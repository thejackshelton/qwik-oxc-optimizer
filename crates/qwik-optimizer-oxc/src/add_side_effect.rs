//! Stage 11 post-transform DCE — SideEffectVisitor.
//!
//! For `Inline` and `Hoist` entry strategies, relative imports that were
//! collected in `GlobalCollect` should be preserved as bare side-effect
//! imports in the root module so bundlers see the dependency edge.
//!
//! ## Algorithm
//!
//! For each entry in `global_collect.imports` whose `source` starts with `'.'`:
//! 1. Resolve the canonical path: `abs_dir.join(source)`.
//! 2. If the resolved path starts_with `src_dir`, the import is within the
//!    project source tree.
//! 3. If the source is not already present as a bare import declaration in
//!    `program.body`, prepend `import './source';` at position 0.
//!
//! SPEC §8 (post-transform DCE, SideEffectVisitor branch).

use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::ast::ast::{Statement};

use crate::collector::GlobalCollect;
use crate::transform::parse_single_statement;

// ---------------------------------------------------------------------------
// normalize_path — resolve ".." components without filesystem access
// ---------------------------------------------------------------------------

/// Normalize a path by resolving `.` and `..` components lexically,
/// without touching the filesystem (unlike `canonicalize`).
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {} // skip "."
            Component::ParentDir => {
                // Pop last normal component if possible
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            other => components.push(other),
        }
    }
    components.iter().collect()
}

// ---------------------------------------------------------------------------
// add_side_effect_imports
// ---------------------------------------------------------------------------

/// Inject bare `import './source';` statements at position 0 of `program.body`
/// for each relative import in `global_collect` whose resolved path is inside
/// `src_dir`.
///
/// Bare imports that are already present in the module are not duplicated.
///
/// # Parameters
/// - `program`        — mutable post-transform AST root
/// - `global_collect` — import index from Stage 7
/// - `abs_dir`        — absolute directory of the file being transformed
/// - `src_dir`        — project source root (filter: only inject within src_dir)
/// - `allocator`      — OXC arena (required for AST insertion)
pub(crate) fn add_side_effect_imports<'a>(
    program: &mut oxc::ast::ast::Program<'a>,
    global_collect: &GlobalCollect,
    abs_dir: &Path,
    src_dir: &Path,
    allocator: &'a Allocator,
) {
    // Step 1: collect existing import specifier sources from program.body.
    let mut existing: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for stmt in program.body.iter() {
        if let Statement::ImportDeclaration(import_decl) = stmt {
            existing.insert(import_decl.source.value.as_str().to_string());
        }
    }

    // Step 2: for each relative import in global_collect, inject if needed.
    // Collect sources to inject (preserve insertion order).
    let mut to_inject: Vec<String> = Vec::new();
    for (_local, import_info) in global_collect.imports.iter() {
        let src = &import_info.source;
        if !src.starts_with('.') {
            continue;
        }
        // Resolve path: abs_dir / source, then normalize to remove ".." components.
        let raw = abs_dir.join(src);
        let resolved = normalize_path(&raw);
        // Check if within src_dir (use normalized src_dir too)
        let normalized_src_dir = normalize_path(src_dir);
        if !resolved.starts_with(&normalized_src_dir) {
            continue;
        }
        // Skip if already present
        if existing.contains(src.as_str()) {
            continue;
        }
        // Avoid duplicate injections in the to_inject list
        if !to_inject.contains(src) {
            to_inject.push(src.clone());
        }
    }

    // Step 3: parse and insert each bare import at position 0 (in reverse
    // order so first item ends up at position 0 after all insertions).
    for source in to_inject.iter().rev() {
        let import_str = format!("import \"{source}\";");
        if let Some(stmt) = parse_single_statement(&import_str, allocator) {
            program.body.insert(0, stmt);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{Import, ImportKind};
    use indexmap::IndexMap;
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::span::SourceType;
    use std::path::PathBuf;

    fn parse_program<'a>(allocator: &'a Allocator, src: &str) -> oxc::ast::ast::Program<'a> {
        let src = allocator.alloc_str(src);
        let ret = Parser::new(allocator, src, SourceType::default()).parse();
        assert!(!ret.panicked, "parse failed");
        ret.program
    }

    fn make_collect(sources: &[&str]) -> GlobalCollect {
        let mut collect = GlobalCollect::new_empty();
        for (i, src) in sources.iter().enumerate() {
            collect.imports.insert(
                format!("local_{i}"),
                Import {
                    source: src.to_string(),
                    specifier: "default".to_string(),
                    kind: ImportKind::Default,
                    synthetic: false,
                },
            );
        }
        collect
    }

    #[test]
    fn test_inject_relative_import_within_src_dir() {
        let alloc = Allocator::default();
        let mut program = parse_program(&alloc, r#"export const x = 1;"#);

        let collect = make_collect(&["./utils"]);
        let abs_dir = PathBuf::from("/project/src");
        let src_dir = PathBuf::from("/project/src");

        add_side_effect_imports(&mut program, &collect, &abs_dir, &src_dir, &alloc);

        // Should have prepended an import statement
        assert_eq!(
            program.body.len(),
            2,
            "Expected original stmt + injected import, got {}",
            program.body.len()
        );
        if let Statement::ImportDeclaration(import_decl) = &program.body[0] {
            assert_eq!(import_decl.source.value.as_str(), "./utils");
        } else {
            panic!("First statement should be an ImportDeclaration");
        }
    }

    #[test]
    fn test_no_injection_for_non_relative_import() {
        let alloc = Allocator::default();
        let mut program = parse_program(&alloc, r#"export const x = 1;"#);
        let original_len = program.body.len();

        let collect = make_collect(&["@qwik.dev/core"]);
        let abs_dir = PathBuf::from("/project/src");
        let src_dir = PathBuf::from("/project/src");

        add_side_effect_imports(&mut program, &collect, &abs_dir, &src_dir, &alloc);

        assert_eq!(
            program.body.len(),
            original_len,
            "Non-relative imports should not be injected"
        );
    }

    #[test]
    fn test_no_injection_outside_src_dir() {
        let alloc = Allocator::default();
        let mut program = parse_program(&alloc, r#"export const x = 1;"#);
        let original_len = program.body.len();

        let collect = make_collect(&["../outside"]);
        let abs_dir = PathBuf::from("/project/src");
        let src_dir = PathBuf::from("/project/src");

        add_side_effect_imports(&mut program, &collect, &abs_dir, &src_dir, &alloc);

        assert_eq!(
            program.body.len(),
            original_len,
            "Imports outside src_dir should not be injected"
        );
    }

    #[test]
    fn test_no_duplicate_injection() {
        let alloc = Allocator::default();
        // Program already has import "./utils"
        let mut program = parse_program(&alloc, r#"import "./utils"; export const x = 1;"#);
        let original_len = program.body.len();

        let collect = make_collect(&["./utils"]);
        let abs_dir = PathBuf::from("/project/src");
        let src_dir = PathBuf::from("/project/src");

        add_side_effect_imports(&mut program, &collect, &abs_dir, &src_dir, &alloc);

        assert_eq!(
            program.body.len(),
            original_len,
            "Existing import should not be duplicated"
        );
    }

    #[test]
    fn test_empty_collect_no_changes() {
        let alloc = Allocator::default();
        let mut program = parse_program(&alloc, r#"export const x = 1;"#);
        let original_len = program.body.len();

        let collect = make_collect(&[]);
        let abs_dir = PathBuf::from("/project/src");
        let src_dir = PathBuf::from("/project/src");

        add_side_effect_imports(&mut program, &collect, &abs_dir, &src_dir, &alloc);

        assert_eq!(
            program.body.len(),
            original_len,
            "Empty collect should not modify program"
        );
    }
}
