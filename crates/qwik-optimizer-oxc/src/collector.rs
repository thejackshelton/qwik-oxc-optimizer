//! Global Collector — Stage 7.
//!
//! Performs a single read-only pass over the parsed AST to build an index of:
//! - `imports`: all import declarations (keyed by local name)
//! - `exports`: all export declarations (keyed by exported name)
//! - `root`: all top-level var/fn/class declarations that are NOT import or
//!   export statements (keyed by binding name)
//!
//! The resulting `GlobalCollect` is consumed by Stage 9 (const replacement) and
//! later stages.

use indexmap::IndexMap;
use oxc::ast::ast::*;
use oxc::span::Span;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The kind of a JavaScript import binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ImportKind {
    /// `import { foo } from "bar"`
    Named,
    /// `import Foo from "bar"` (default binding)
    Default,
    /// `import * as ns from "bar"` (namespace binding)
    Namespace,
}

/// Information about a single import binding.
#[derive(Debug, Clone)]
pub(crate) struct Import {
    /// The module specifier string (the `"bar"` in `import { foo } from "bar"`).
    pub source: String,
    /// The imported name from the source module (e.g. `"foo"` in `import { foo }`).
    /// For default imports this is `"default"`.
    /// For namespace imports this is `"*"`.
    pub specifier: String,
    /// How the binding was imported.
    pub kind: ImportKind,
    /// True if this import was synthetically injected (not from the original source).
    pub synthetic: bool,
}

/// Placeholder for export metadata. Extended in future phases.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExportInfo {
    // Phase 10+ will add re-export source, export kind, etc.
}

/// Centralized index of all imports, exports, and top-level declarations in a module.
///
/// Built by [`global_collect`] via a single read-only pass.
pub(crate) struct GlobalCollect {
    /// Synthetically-inserted imports (not from parsed source). Phase 10+ populates this.
    pub synthetic: Vec<(String, Import)>,
    /// All import bindings, keyed by local binding name.
    pub imports: IndexMap<String, Import>,
    /// All exported names, keyed by exported name.
    pub exports: IndexMap<String, ExportInfo>,
    /// Top-level declarations that are NOT imports or exports,
    /// keyed by binding name and valued by the declaration span.
    pub root: IndexMap<String, Span>,
    /// Reverse lookup: (specifier, source) -> local name.
    rev_imports: HashMap<(String, String), String>,
}

impl GlobalCollect {
    fn new() -> Self {
        Self {
            synthetic: Vec::new(),
            imports: IndexMap::with_capacity(16),
            exports: IndexMap::with_capacity(16),
            root: IndexMap::with_capacity(16),
            rev_imports: HashMap::with_capacity(16),
        }
    }

    /// Insert an import binding, updating both `imports` and `rev_imports`.
    fn insert_import(&mut self, local: String, import: Import) {
        let key = (import.specifier.clone(), import.source.clone());
        self.rev_imports.insert(key, local.clone());
        self.imports.insert(local, import);
    }

    // -----------------------------------------------------------------------
    // Query methods
    // -----------------------------------------------------------------------

    /// Resolve `(specifier, source)` → local binding name.
    ///
    /// Returns `Some(local)` when an `import { specifier } from "source"` exists.
    pub(crate) fn get_imported_local(&self, specifier: &str, source: &str) -> Option<&str> {
        self.rev_imports
            .get(&(specifier.to_string(), source.to_string()))
            .map(|s| s.as_str())
    }

    /// Returns `true` if `name` appears in imports, exports, or root.
    pub(crate) fn is_global(&self, name: &str) -> bool {
        self.imports.contains_key(name)
            || self.exports.contains_key(name)
            || self.root.contains_key(name)
    }
}

// ---------------------------------------------------------------------------
// Collector pass (iterates program.body directly — top-level only)
// ---------------------------------------------------------------------------

/// Build a `GlobalCollect` for `program` by scanning all top-level statements.
pub(crate) fn global_collect(program: &Program<'_>) -> GlobalCollect {
    let mut collect = GlobalCollect::new();

    for stmt in &program.body {
        match stmt {
            // ----------------------------------------------------------------
            // Import declarations
            // ----------------------------------------------------------------
            Statement::ImportDeclaration(import_decl) => {
                let source = import_decl.source.value.as_str().to_string();
                if let Some(specifiers) = &import_decl.specifiers {
                    for spec in specifiers {
                        match spec {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                let local = s.local.name.as_str().to_string();
                                let imported = match &s.imported {
                                    ModuleExportName::IdentifierName(id) => {
                                        id.name.as_str().to_string()
                                    }
                                    ModuleExportName::IdentifierReference(id) => {
                                        id.name.as_str().to_string()
                                    }
                                    ModuleExportName::StringLiteral(s) => {
                                        s.value.as_str().to_string()
                                    }
                                };
                                collect.insert_import(
                                    local,
                                    Import {
                                        source: source.clone(),
                                        specifier: imported,
                                        kind: ImportKind::Named,
                                        synthetic: false,
                                    },
                                );
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                let local = s.local.name.as_str().to_string();
                                collect.insert_import(
                                    local,
                                    Import {
                                        source: source.clone(),
                                        specifier: "default".to_string(),
                                        kind: ImportKind::Default,
                                        synthetic: false,
                                    },
                                );
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                let local = s.local.name.as_str().to_string();
                                collect.insert_import(
                                    local,
                                    Import {
                                        source: source.clone(),
                                        specifier: "*".to_string(),
                                        kind: ImportKind::Namespace,
                                        synthetic: false,
                                    },
                                );
                            }
                        }
                    }
                }
            }

            // ----------------------------------------------------------------
            // Export named declarations: `export { x }` or `export const x = ...`
            // ----------------------------------------------------------------
            Statement::ExportNamedDeclaration(export_decl) => {
                // Re-exports from another module: `export { x } from "mod"` — only track exports.
                // Local re-exports: track exported names.
                for spec in &export_decl.specifiers {
                    let exported_name = match &spec.exported {
                        ModuleExportName::IdentifierName(id) => id.name.as_str().to_string(),
                        ModuleExportName::IdentifierReference(id) => {
                            id.name.as_str().to_string()
                        }
                        ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
                    };
                    collect.exports.insert(exported_name, ExportInfo::default());
                }

                // Inline declaration: `export const x = 1;` or `export function f() {}`
                if let Some(decl) = &export_decl.declaration {
                    collect_decl_names(decl, &mut collect.exports, Some(&mut collect.root));
                }
            }

            // ----------------------------------------------------------------
            // Default exports: `export default function Foo() {}` or `export default expr`
            // ----------------------------------------------------------------
            Statement::ExportDefaultDeclaration(export_default) => {
                collect.exports.insert("default".to_string(), ExportInfo::default());
                // If a named function/class is default-exported, also add to root.
                match &export_default.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        if let Some(id) = &f.id {
                            collect
                                .root
                                .insert(id.name.as_str().to_string(), id.span);
                        }
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        if let Some(id) = &c.id {
                            collect
                                .root
                                .insert(id.name.as_str().to_string(), id.span);
                        }
                    }
                    _ => {}
                }
            }

            // ----------------------------------------------------------------
            // Export all: `export * from "mod"` — no local bindings to index.
            // ----------------------------------------------------------------
            Statement::ExportAllDeclaration(_) => {}

            // ----------------------------------------------------------------
            // Top-level declarations (NOT exported, NOT imported)
            // ----------------------------------------------------------------
            other => {
                collect_stmt_root(other, &mut collect.root);
            }
        }
    }

    collect
}

/// Collect binding names from a declaration, inserting into `exports` and
/// optionally into `root`.
fn collect_decl_names(
    decl: &Declaration<'_>,
    exports: &mut IndexMap<String, ExportInfo>,
    mut root: Option<&mut IndexMap<String, Span>>,
) {
    match decl {
        Declaration::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                collect_binding_pattern_names(&declarator.id, exports, root.as_deref_mut());
            }
        }
        Declaration::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                let name = id.name.as_str().to_string();
                exports.insert(name.clone(), ExportInfo::default());
                if let Some(r) = root {
                    r.insert(name, id.span);
                }
            }
        }
        Declaration::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                let name = id.name.as_str().to_string();
                exports.insert(name.clone(), ExportInfo::default());
                if let Some(r) = root {
                    r.insert(name, id.span);
                }
            }
        }
        _ => {}
    }
}

/// Collect binding names from a `BindingPattern`, inserting into exports and/or root.
fn collect_binding_pattern_names(
    pattern: &BindingPattern<'_>,
    exports: &mut IndexMap<String, ExportInfo>,
    mut root: Option<&mut IndexMap<String, Span>>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            let name = id.name.as_str().to_string();
            exports.insert(name.clone(), ExportInfo::default());
            if let Some(r) = root {
                r.insert(name, id.span);
            }
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_pattern_names(&prop.value, exports, root.as_deref_mut());
            }
            if let Some(rest) = &obj.rest {
                collect_binding_pattern_names(&rest.argument, exports, root.as_deref_mut());
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for element in arr.elements.iter().flatten() {
                collect_binding_pattern_names(element, exports, root.as_deref_mut());
            }
            if let Some(rest) = &arr.rest {
                collect_binding_pattern_names(&rest.argument, exports, root.as_deref_mut());
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_pattern_names(&assign.left, exports, root.as_deref_mut());
        }
    }
}

/// Collect top-level binding names from a statement into `root`.
/// Only handles var/fn/class declarations — other statement kinds are skipped.
fn collect_stmt_root(stmt: &Statement<'_>, root: &mut IndexMap<String, Span>) {
    match stmt {
        Statement::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                collect_binding_into_root(&declarator.id, root);
            }
        }
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                root.insert(id.name.as_str().to_string(), id.span);
            }
        }
        Statement::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                root.insert(id.name.as_str().to_string(), id.span);
            }
        }
        _ => {}
    }
}

/// Recursively collect binding names from a `BindingPattern` into `root`.
fn collect_binding_into_root(pattern: &BindingPattern<'_>, root: &mut IndexMap<String, Span>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            root.insert(id.name.as_str().to_string(), id.span);
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_into_root(&prop.value, root);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_into_root(&rest.argument, root);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for element in arr.elements.iter().flatten() {
                collect_binding_into_root(element, root);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_into_root(&rest.argument, root);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_into_root(&assign.left, root);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    fn parse(src: &str) -> (Allocator, oxc::ast::ast::Program<'static>) {
        // SAFETY: We extend the lifetime so the program can be returned alongside
        // the allocator. The allocator owns all data; both are dropped together.
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let ret = Parser::new(&allocator, src, source_type).parse();
        // SAFETY: program borrows from allocator; both live as long as the tuple.
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        (allocator, program)
    }

    // -----------------------------------------------------------------------
    // Import tests
    // -----------------------------------------------------------------------

    #[test]
    fn named_import_populates_imports() {
        let (_alloc, program) = parse(r#"import { foo } from "bar";"#);
        let collect = global_collect(&program);
        let imp = collect.imports.get("foo").expect("foo not found in imports");
        assert_eq!(imp.source, "bar");
        assert_eq!(imp.specifier, "foo");
        assert_eq!(imp.kind, ImportKind::Named);
        assert!(!imp.synthetic);
    }

    #[test]
    fn default_import_populates_imports() {
        let (_alloc, program) = parse(r#"import Def from "bar";"#);
        let collect = global_collect(&program);
        let imp = collect.imports.get("Def").expect("Def not found in imports");
        assert_eq!(imp.source, "bar");
        assert_eq!(imp.specifier, "default");
        assert_eq!(imp.kind, ImportKind::Default);
    }

    #[test]
    fn namespace_import_populates_imports() {
        let (_alloc, program) = parse(r#"import * as ns from "bar";"#);
        let collect = global_collect(&program);
        let imp = collect.imports.get("ns").expect("ns not found in imports");
        assert_eq!(imp.source, "bar");
        assert_eq!(imp.specifier, "*");
        assert_eq!(imp.kind, ImportKind::Namespace);
    }

    #[test]
    fn import_does_not_appear_in_root() {
        let (_alloc, program) = parse(r#"import { foo } from "bar";"#);
        let collect = global_collect(&program);
        assert!(
            !collect.root.contains_key("foo"),
            "Import bindings must NOT appear in root"
        );
    }

    // -----------------------------------------------------------------------
    // Export tests
    // -----------------------------------------------------------------------

    #[test]
    fn named_export_specifier_populates_exports() {
        let (_alloc, program) = parse(r#"const foo = 1; export { foo };"#);
        let collect = global_collect(&program);
        assert!(
            collect.exports.contains_key("foo"),
            "export {{ foo }} must populate exports"
        );
    }

    #[test]
    fn export_const_populates_exports_and_root() {
        let (_alloc, program) = parse(r#"export const x = 1;"#);
        let collect = global_collect(&program);
        assert!(
            collect.exports.contains_key("x"),
            "export const x must populate exports"
        );
        assert!(
            collect.root.contains_key("x"),
            "export const x must also populate root"
        );
    }

    #[test]
    fn export_function_populates_exports() {
        let (_alloc, program) = parse(r#"export function f() {}"#);
        let collect = global_collect(&program);
        assert!(
            collect.exports.contains_key("f"),
            "export function f must populate exports"
        );
    }

    #[test]
    fn export_default_function_populates_exports() {
        let (_alloc, program) = parse(r#"export default function Foo() {}"#);
        let collect = global_collect(&program);
        assert!(
            collect.exports.contains_key("default"),
            "export default must insert 'default' key into exports"
        );
    }

    // -----------------------------------------------------------------------
    // Root tests
    // -----------------------------------------------------------------------

    #[test]
    fn plain_const_and_function_populate_root() {
        let (_alloc, program) = parse(r#"const y = 2; function g() {}"#);
        let collect = global_collect(&program);
        assert!(collect.root.contains_key("y"), "const y must appear in root");
        assert!(collect.root.contains_key("g"), "function g must appear in root");
    }

    #[test]
    fn import_declaration_does_not_appear_in_root() {
        let (_alloc, program) =
            parse(r#"import { foo } from "bar"; const x = 1;"#);
        let collect = global_collect(&program);
        assert!(
            !collect.root.contains_key("foo"),
            "Import 'foo' must NOT appear in root"
        );
        assert!(
            collect.root.contains_key("x"),
            "Plain const 'x' must appear in root"
        );
    }

    // -----------------------------------------------------------------------
    // get_imported_local tests
    // -----------------------------------------------------------------------

    #[test]
    fn get_imported_local_returns_local_name() {
        let (_alloc, program) = parse(r#"import { foo } from "bar";"#);
        let collect = global_collect(&program);
        let local = collect.get_imported_local("foo", "bar");
        assert_eq!(local, Some("foo"), "get_imported_local should return 'foo'");
    }

    #[test]
    fn get_imported_local_returns_none_for_missing() {
        let (_alloc, program) = parse(r#"import { foo } from "bar";"#);
        let collect = global_collect(&program);
        let local = collect.get_imported_local("missing", "bar");
        assert_eq!(local, None, "get_imported_local should return None for unknown specifier");
    }

    #[test]
    fn get_imported_local_wrong_source_returns_none() {
        let (_alloc, program) = parse(r#"import { foo } from "bar";"#);
        let collect = global_collect(&program);
        let local = collect.get_imported_local("foo", "wrong-source");
        assert_eq!(
            local, None,
            "get_imported_local should return None for wrong source"
        );
    }
}
