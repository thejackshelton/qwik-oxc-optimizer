//! Import renaming — Stage 5.
//!
//! Renames `@builder.io/*` import sources to their `@qwik.dev/*` equivalents.
//! This runs as a `VisitMut` pass over the parsed AST before GlobalCollect.
//!
//! # Rename table
//! | Old prefix (length)          | New prefix              |
//! |------------------------------|-------------------------|
//! | `@builder.io/qwik-city` (21) | `@qwik.dev/router`      |
//! | `@builder.io/qwik-react` (22)| `@qwik.dev/react`       |
//! | `@builder.io/qwik` (16)      | `@qwik.dev/core`        |
//!
//! Order is load-bearing: `qwik-city` must be checked BEFORE `qwik` to avoid
//! the shorter prefix matching the longer string.
//!
//! Only `ImportDeclaration` sources are rewritten. Export-from (`export { x }
//! from "..."`) is intentionally left unchanged.

use oxc::allocator::Allocator;
use oxc::ast::ast::*;
use oxc::ast_visit::VisitMut;
use oxc::span::Atom;

/// Visitor that renames `@builder.io/*` import sources to `@qwik.dev/*`.
///
/// The allocator is required to intern the new source strings into the AST
/// arena (OXC 0.113: `Atom<'a>` is a borrowed reference into the arena).
pub(crate) struct RenameTransform<'alloc> {
    allocator: &'alloc Allocator,
}

impl<'a> VisitMut<'a> for RenameTransform<'a> {
    fn visit_import_declaration(&mut self, node: &mut ImportDeclaration<'a>) {
        let src = node.source.value.as_str();
        // Order is load-bearing: qwik-city (len 21) BEFORE qwik (len 16).
        let new_val: Option<String> = if src.starts_with("@builder.io/qwik-city") {
            Some(format!("@qwik.dev/router{}", &src[21..]))
        } else if src.starts_with("@builder.io/qwik-react") {
            Some(format!("@qwik.dev/react{}", &src[22..]))
        } else if src.starts_with("@builder.io/qwik") {
            Some(format!("@qwik.dev/core{}", &src[16..]))
        } else {
            None
        };
        if let Some(new_src) = new_val {
            // Allocate the new string into the arena so `Atom<'a>` is valid.
            let interned: &'a str = self.allocator.alloc_str(&new_src);
            node.source.value = Atom::from(interned);
        }
        // Do NOT walk children — only source value is rewritten.
    }
}

/// Run the import rename pass over the program.
///
/// Only `import` declarations are affected. `export ... from "..."` sources
/// are left unchanged (Stage 5 scope is imports only).
pub(crate) fn rename_imports<'a>(program: &mut Program<'a>, allocator: &'a Allocator) {
    let mut renamer = RenameTransform { allocator };
    renamer.visit_program(program);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oxc::allocator::Allocator;
    use oxc::codegen::Codegen;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    fn transform(src: &str) -> String {
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let ret = Parser::new(&allocator, src, source_type).parse();
        let mut program = ret.program;
        rename_imports(&mut program, &allocator);
        Codegen::new().build(&program).code
    }

    #[test]
    fn renames_qwik_city() {
        let src = r#"import { x } from "@builder.io/qwik-city";"#;
        let out = transform(src);
        assert!(
            out.contains("@qwik.dev/router"),
            "Expected @qwik.dev/router, got: {out}"
        );
        assert!(
            !out.contains("@builder.io/qwik-city"),
            "Old import should be gone, got: {out}"
        );
    }

    #[test]
    fn renames_qwik_city_with_suffix() {
        let src = r#"import { x } from "@builder.io/qwik-city/middleware/node";"#;
        let out = transform(src);
        assert!(
            out.contains("@qwik.dev/router/middleware/node"),
            "Expected suffix preserved, got: {out}"
        );
    }

    #[test]
    fn renames_qwik_react() {
        let src = r#"import { x } from "@builder.io/qwik-react";"#;
        let out = transform(src);
        assert!(
            out.contains("@qwik.dev/react"),
            "Expected @qwik.dev/react, got: {out}"
        );
        assert!(
            !out.contains("@builder.io/qwik-react"),
            "Old import should be gone, got: {out}"
        );
    }

    #[test]
    fn renames_qwik_core() {
        let src = r#"import { x } from "@builder.io/qwik";"#;
        let out = transform(src);
        assert!(
            out.contains("@qwik.dev/core"),
            "Expected @qwik.dev/core, got: {out}"
        );
        assert!(
            !out.contains("@builder.io/qwik\""),
            "Old import should be gone, got: {out}"
        );
    }

    #[test]
    fn renames_qwik_core_with_suffix() {
        let src = r#"import { x } from "@builder.io/qwik/build";"#;
        let out = transform(src);
        assert!(
            out.contains("@qwik.dev/core/build"),
            "Expected suffix preserved, got: {out}"
        );
    }

    #[test]
    fn does_not_rename_export_from() {
        // export-from sources are intentionally untouched (Stage 5 scope)
        let src = r#"export { x } from "@builder.io/qwik";"#;
        let out = transform(src);
        assert!(
            out.contains("@builder.io/qwik"),
            "Export-from source should be unchanged, got: {out}"
        );
        assert!(
            !out.contains("@qwik.dev"),
            "Export-from should NOT be renamed, got: {out}"
        );
    }

    #[test]
    fn leaves_unknown_packages_unchanged() {
        let src = r#"import { x } from "other-package";"#;
        let out = transform(src);
        assert!(
            out.contains("other-package"),
            "Unknown package should be unchanged, got: {out}"
        );
        assert!(
            !out.contains("@qwik.dev"),
            "Unknown package should not gain qwik.dev prefix, got: {out}"
        );
    }

    #[test]
    fn prefix_order_safety_qwik_city_not_matched_as_qwik() {
        // "@builder.io/qwik-city" must map to "@qwik.dev/router", NOT "@qwik.dev/core-city"
        let src = r#"import { x } from "@builder.io/qwik-city";"#;
        let out = transform(src);
        assert!(
            !out.contains("@qwik.dev/core-city"),
            "qwik-city must not match qwik prefix, got: {out}"
        );
        assert!(
            out.contains("@qwik.dev/router"),
            "qwik-city must map to router, got: {out}"
        );
    }
}
