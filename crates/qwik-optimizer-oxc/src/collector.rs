//! First-pass AST analysis.
//!
//! Walk the parsed AST once (before transformation) to collect information
//! needed by the transform pass: which imports come from `@qwik.dev/core`,
//! which of those are `$`-suffixed, what the module exports, and which
//! identifiers are declared at module scope. This is a read-only pass --
//! it does not mutate the AST.
//!
//! Display names and segment naming are handled entirely by the transform
//! pass via `stack_ctxt` (see `transform.rs::register_context_name`).
//!
//! Also provides `compute_captures()` for capture analysis: given a set of
//! identifier names referenced inside a $()-body and a set of names declared
//! locally in that body, classify each outer reference as LocalCapture,
//! ImportReemit, or skipped (global / framework import).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use oxc::ast::ast::*;
use oxc::semantic::Scoping;

use crate::types::{CollectResult, ExportInfo, ImportInfo, ImportKind};

// ---------------------------------------------------------------------------
// Capture Analysis
// ---------------------------------------------------------------------------

/// A single import binding that needs to be re-emitted in a segment module.
#[derive(Debug, Clone)]
pub(crate) struct ReemittedImport {
    /// The local binding name (e.g., "dep3", "bbar", "dep2").
    pub local_name: String,
    /// The module source path (e.g., "dep3/something", "../state").
    pub source: String,
    /// The kind of import (default, namespace, or named).
    pub kind: ImportKind,
    /// For aliased named imports, the original imported name (e.g., "bar" for `import { bar as bbar }`).
    /// None if the local name matches the imported name.
    pub imported_name: Option<String>,
    /// Import assertion/attribute clause (e.g., `with { type: "json" }`).
    pub assertion: Vec<(String, String)>,
}

/// Result of capture analysis for a single $()-body.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CaptureAnalysisResult {
    /// Variable names that will be passed via _captures[] at runtime.
    /// Order matches encounter order from the body traversal.
    pub capture_names: Vec<String>,

    /// Import bindings referenced in the body that should be re-emitted
    /// in the segment module (NOT captured).
    pub reemitted_imports: Vec<ReemittedImport>,

    /// Diagnostic messages for invalid captures (function/class declarations).
    pub diagnostics: Vec<String>,
}

/// A well-known global identifier that should never be treated as a capture.
pub(crate) static KNOWN_GLOBALS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "console",
        "undefined",
        "NaN",
        "Infinity",
        "window",
        "document",
        "globalThis",
        "self",
        "navigator",
        "location",
        "history",
        "localStorage",
        "sessionStorage",
        "fetch",
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "requestAnimationFrame",
        "cancelAnimationFrame",
        "queueMicrotask",
        "Promise",
        "Array",
        "Object",
        "String",
        "Number",
        "Boolean",
        "Symbol",
        "Map",
        "Set",
        "WeakMap",
        "WeakSet",
        "Date",
        "RegExp",
        "Error",
        "TypeError",
        "RangeError",
        "JSON",
        "Math",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "encodeURIComponent",
        "decodeURIComponent",
        "encodeURI",
        "decodeURI",
        "atob",
        "btoa",
        "structuredClone",
        "URL",
        "URLSearchParams",
        "Headers",
        "Request",
        "Response",
        "FormData",
        "Blob",
        "File",
        "TextEncoder",
        "TextDecoder",
        "AbortController",
        "AbortSignal",
        "Event",
        "CustomEvent",
        "EventTarget",
        "crypto",
        "performance",
        "alert",
        "confirm",
        "prompt",
        "import",
        "require",
        "module",
        "exports",
        "__dirname",
        "__filename",
        "process",
        "Buffer",
        "global",
        "arguments",
        "this",
        "super",
        "new",
        "true",
        "false",
        "null",
    ])
});

/// Determine which variables are captured by a $()-body.
///
/// This is the simplified/pragmatic approach for Phase 9: rather than using the
/// full OXC Scoping API (which is consumed by `traverse_mut`), we receive:
///
/// - `body_ident_refs`: All IdentifierReference names seen inside the $()-body
///   (collected during Traverse via `enter_identifier_reference`)
/// - `body_local_decls`: Names declared locally inside the $()-body (parameters,
///   let/const/var declarations) -- these are NOT captures
/// - `collect_result`: The collector result with import data
///
/// For each unique name in `body_ident_refs` that is NOT in `body_local_decls`:
/// - If it's a known global -> skip
/// - If it's in the module's imports -> classify as ImportReemit (NOT captured)
/// - If it's a $-suffixed import from @qwik.dev/core -> skip (framework, handled by QRL rewriting)
/// - Otherwise -> LocalCapture (add to capture_names)
///
/// Encounter order from the body traversal is preserved in `body_ident_refs`.
pub(crate) fn compute_captures(
    body_ident_refs: &[String],
    body_local_decls: &HashSet<String>,
    collect_result: &CollectResult,
) -> CaptureAnalysisResult {
    let mut capture_names = Vec::new();
    let mut reemitted_imports: Vec<ReemittedImport> = Vec::new();
    let diagnostics = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for name in body_ident_refs {
        if !seen.insert(name.clone()) {
            continue;
        }

        if body_local_decls.contains(name) {
            continue;
        }

        if KNOWN_GLOBALS.contains(name.as_str()) {
            continue;
        }

        if collect_result.dollar_imports.contains(name) {
            continue;
        }

        // NOTE: module_level_decls are NOT skipped here. They need to be captured
        // for nested segments because the segment module can't access the parent
        // module's top-level scope. Top-level segments already zero out their
        // captures in transform.rs (is_top_level_dollar_call).

        let mut is_import = false;
        for import_info in &collect_result.module_imports {
            if let Some(idx) = import_info.specifiers.iter().position(|s| s == name) {
                let kind = import_info
                    .specifier_kinds
                    .get(idx)
                    .cloned()
                    .unwrap_or(ImportKind::Named);
                let imported_name = import_info.specifier_aliases.get(name).cloned();
                reemitted_imports.push(ReemittedImport {
                    local_name: name.clone(),
                    source: import_info.source.clone(),
                    kind,
                    imported_name,
                    assertion: import_info.assertion.clone(),
                });
                is_import = true;
                break;
            }
        }
        if is_import {
            continue;
        }

        capture_names.push(name.clone());
    }

    // Sort capture names alphabetically to match SWC's ordering.
    // SWC uses a HashSet->Vec->sort() pattern in compute_scoped_idents(),
    // which produces alphabetical order regardless of encounter order.
    capture_names.sort();

    CaptureAnalysisResult {
        capture_names,
        reemitted_imports,
        diagnostics,
    }
}

/// Context for the first-pass AST walk.
struct CollectContext {
    /// Set of dollar-suffixed imports from @qwik.dev/core (or custom core_module).
    /// Contains LOCAL names (which may be aliases).
    dollar_imports: HashSet<String>,
    /// Alias map: local_name -> original_imported_name for $-suffixed imports.
    /// Only populated when the local name differs from the imported name.
    alias_map: HashMap<String, String>,
    /// All import declarations.
    module_imports: Vec<ImportInfo>,
    /// All export declarations.
    module_exports: Vec<ExportInfo>,
    /// Names declared at module (top-level) scope.
    module_level_decls: HashSet<String>,
    /// The core module import path(s) to recognize as Qwik imports.
    /// Always includes "@qwik.dev/core"; may also include a custom core_module.
    core_modules: Vec<String>,
    /// Local binding names that are user-exported (via export const/function/class
    /// or export { X }). Used to determine which module-level decls need _auto_ prefix
    /// when re-exported for segment self-imports. Does NOT include `export default`.
    exported_local_names: HashSet<String>,
}

impl CollectContext {
    fn new(core_module: Option<&str>) -> Self {
        let mut core_modules = vec!["@qwik.dev/core".to_string()];
        if let Some(cm) = core_module {
            if cm != "@qwik.dev/core" {
                core_modules.push(cm.to_string());
            }
        }
        if !core_modules.iter().any(|m| m == "@builder.io/qwik") {
            core_modules.push("@builder.io/qwik".to_string());
        }
        Self {
            dollar_imports: HashSet::new(),
            alias_map: HashMap::new(),
            module_imports: Vec::new(),
            module_exports: Vec::new(),
            module_level_decls: HashSet::new(),
            core_modules,
            exported_local_names: HashSet::new(),
        }
    }

    /// Check if an import source is a recognized Qwik module.
    ///
    /// Returns true for:
    /// - `@qwik.dev/core` (default)
    /// - `@qwik.dev/*` packages (e.g., `@qwik.dev/react`)
    /// - `@builder.io/qwik` and `@builder.io/qwik-*` (legacy)
    /// - Any custom core_module specified in config
    ///
    /// Sub-paths like `@qwik.dev/core/build` or `@qwik.dev/core/jsx-runtime`
    /// are NOT treated as core imports (they don't re-export $-APIs).
    fn is_qwik_core_import(&self, source: &str) -> bool {
        if self.core_modules.iter().any(|m| m == source) {
            return true;
        }
        if source.starts_with("@qwik.dev/") {
            let after_scope = &source["@qwik.dev/".len()..];
            return !after_scope.contains('/');
        }
        if source.starts_with("@builder.io/qwik-") {
            return true;
        }
        false
    }
}

/// Collect binding names from a Declaration into a set.
/// Handles variable declarations, function declarations, and class declarations.
fn collect_declaration_names(names: &mut HashSet<String>, decl: &oxc::ast::ast::Declaration<'_>) {
    use oxc::ast::ast::Declaration;
    match decl {
        Declaration::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                collect_binding_pattern_names_into(names, &declarator.id);
            }
        }
        Declaration::FunctionDeclaration(fn_decl) => {
            if let Some(ident) = &fn_decl.id {
                names.insert(ident.name.as_str().to_string());
            }
        }
        Declaration::ClassDeclaration(class_decl) => {
            if let Some(ident) = &class_decl.id {
                names.insert(ident.name.as_str().to_string());
            }
        }
        Declaration::TSEnumDeclaration(enum_decl) => {
            names.insert(enum_decl.id.name.as_str().to_string());
        }
        _ => {}
    }
}

/// Collect binding names from a BindingPattern into a set (for module-level tracking).
fn collect_binding_pattern_names_into(
    names: &mut HashSet<String>,
    pattern: &oxc::ast::ast::BindingPattern<'_>,
) {
    use oxc::ast::ast::BindingPattern;
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            names.insert(ident.name.as_str().to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_pattern_names_into(names, &prop.value);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_pattern_names_into(names, &rest.argument);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_binding_pattern_names_into(names, elem);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_pattern_names_into(names, &rest.argument);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_pattern_names_into(names, &assign.left);
        }
    }
}

/// Collect declaration names from a top-level statement.
///
/// Delegates to `collect_declaration_names` for statements that carry a
/// Declaration (variable, function, class). Non-declaration statements are
/// ignored.
fn collect_statement_decl_names(names: &mut HashSet<String>, stmt: &oxc::ast::ast::Statement<'_>) {
    if let Some(decl) = stmt.as_declaration() {
        collect_declaration_names(names, decl);
    }
}

/// Perform first-pass analysis of the parsed module.
///
/// Walks the program body to collect:
/// - Dollar-suffixed imports from `@qwik.dev/core` (or custom core_module)
/// - All import and export declarations
/// - Alias mappings for renamed $-suffixed imports
/// - Module-level declaration names (for capture analysis)
/// - Local `$`-suffixed definitions via `wrap()`/`implicit$FirstArg()`
///
/// Display names and segment naming are handled entirely by the transform
/// pass via `stack_ctxt` (see `transform.rs::register_context_name`).
///
/// The `core_module` parameter allows recognizing imports from a custom
/// module (e.g., `@qwik.dev/react`, `@builder.io/qwik`) as Qwik core
/// imports in addition to the default `@qwik.dev/core`.
pub(crate) fn collect<'a>(
    program: &Program<'a>,
    _scoping: &Scoping,
    core_module: Option<&str>,
) -> CollectResult {
    let mut ctx = CollectContext::new(core_module);

    for stmt in &program.body {
        if let Statement::ImportDeclaration(import) = stmt {
            collect_import(&mut ctx, import);
        }
    }

    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(_) => {}
            Statement::ExportNamedDeclaration(export) => {
                if let Some(decl) = &export.declaration {
                    collect_declaration_names(&mut ctx.module_level_decls, decl);
                }
                collect_named_export(&mut ctx, export);
            }
            Statement::ExportDefaultDeclaration(export) => {
                collect_default_export(&mut ctx, export);
            }
            _ => {
                collect_statement_decl_names(&mut ctx.module_level_decls, stmt);
                walk_statement_for_calls(&mut ctx, stmt);
            }
        }
    }

    CollectResult {
        dollar_imports: ctx.dollar_imports,
        alias_map: ctx.alias_map,
        module_imports: ctx.module_imports,
        module_exports: ctx.module_exports,
        module_level_decls: ctx.module_level_decls,
        exported_local_names: ctx.exported_local_names,
    }
}

/// Collect information from an import declaration.
fn collect_import(ctx: &mut CollectContext, import: &ImportDeclaration<'_>) {
    let source = import.source.value.as_str();
    let is_qwik_core = ctx.is_qwik_core_import(source);

    let mut specifiers_vec = Vec::new();
    let mut specifier_kinds_vec = Vec::new();
    let mut specifier_aliases = HashMap::new();

    if let Some(specifiers) = &import.specifiers {
        for spec in specifiers {
            match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let imported_name = match &s.imported {
                        ModuleExportName::IdentifierName(id) => id.name.as_str(),
                        ModuleExportName::IdentifierReference(id) => id.name.as_str(),
                        ModuleExportName::StringLiteral(s) => s.value.as_str(),
                    };
                    let local_name = s.local.name.as_str();
                    specifiers_vec.push(local_name.to_string());
                    specifier_kinds_vec.push(ImportKind::Named);

                    // Track alias mapping for all aliased specifiers (not just $-suffixed)
                    if local_name != imported_name {
                        specifier_aliases.insert(local_name.to_string(), imported_name.to_string());
                    }

                    if imported_name == "$" || imported_name.ends_with('$') {
                        ctx.dollar_imports.insert(local_name.to_string());

                        if local_name != imported_name {
                            ctx.alias_map
                                .insert(local_name.to_string(), imported_name.to_string());
                        }
                    }
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    specifiers_vec.push(s.local.name.as_str().to_string());
                    specifier_kinds_vec.push(ImportKind::Default);
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    specifiers_vec.push(s.local.name.as_str().to_string());
                    specifier_kinds_vec.push(ImportKind::Namespace);
                }
            }
        }
    }

    // Collect import assertion/attribute clause (e.g., `with { type: "json" }`)
    let assertion = if let Some(ref with_clause) = import.with_clause {
        with_clause
            .with_entries
            .iter()
            .filter_map(|entry| {
                let key = match &entry.key {
                    ImportAttributeKey::Identifier(id) => id.name.as_str().to_string(),
                    ImportAttributeKey::StringLiteral(s) => s.value.to_string(),
                };
                Some((key, entry.value.value.to_string()))
            })
            .collect()
    } else {
        Vec::new()
    };

    ctx.module_imports.push(ImportInfo {
        source: source.to_string(),
        specifiers: specifiers_vec,
        specifier_kinds: specifier_kinds_vec,
        specifier_aliases,
        is_qwik_core,
        span: (import.span.start, import.span.end),
        assertion,
    });
}

/// Collect information from a named export declaration.
fn collect_named_export(ctx: &mut CollectContext, export: &ExportNamedDeclaration<'_>) {
    if let Some(decl) = &export.declaration {
        match decl {
            Declaration::VariableDeclaration(var_decl) => {
                for declarator in &var_decl.declarations {
                    // Collect ALL binding names from destructured patterns for exported_local_names
                    let mut decl_names = HashSet::new();
                    collect_binding_pattern_names_into(&mut decl_names, &declarator.id);
                    for dn in &decl_names {
                        ctx.exported_local_names.insert(dn.clone());
                    }

                    let Some(name) = binding_pattern_name(&declarator.id) else {
                        continue;
                    };
                    ctx.module_exports.push(ExportInfo {
                        name: name.clone(),
                        is_reexport: false,
                        span: (export.span.start, export.span.end),
                    });

                    if name.ends_with('$')
                        && !ctx.dollar_imports.contains(&name)
                        && declarator
                            .init
                            .as_ref()
                            .is_some_and(|init| is_wrap_call(init))
                    {
                        ctx.dollar_imports.insert(name.clone());
                    }

                    if let Some(init) = &declarator.init {
                        walk_expression_for_calls(ctx, init);
                    }
                }
            }
            Declaration::FunctionDeclaration(func) => {
                if let Some(id) = &func.id {
                    let func_name = id.name.as_str().to_string();
                    ctx.module_exports.push(ExportInfo {
                        name: func_name.clone(),
                        is_reexport: false,
                        span: (export.span.start, export.span.end),
                    });
                    // Track as user-exported for _auto_ prefix determination
                    ctx.exported_local_names.insert(func_name);

                    // Walk function body to find wrap() definitions
                    if let Some(body) = &func.body {
                        for s in &body.statements {
                            walk_statement_for_calls(ctx, s);
                        }
                    }
                }
            }
            Declaration::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    let class_name = id.name.as_str().to_string();
                    ctx.module_exports.push(ExportInfo {
                        name: class_name.clone(),
                        is_reexport: false,
                        span: (export.span.start, export.span.end),
                    });
                    // Track as user-exported for _auto_ prefix determination
                    ctx.exported_local_names.insert(class_name);
                }
            }
            _ => {}
        }
    }

    if export.source.is_some() {
        // Re-exports from another module (export { X } from '...')
        for spec in &export.specifiers {
            let name = match &spec.exported {
                ModuleExportName::IdentifierName(id) => id.name.as_str().to_string(),
                ModuleExportName::IdentifierReference(id) => id.name.as_str().to_string(),
                ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
            };
            ctx.module_exports.push(ExportInfo {
                name,
                is_reexport: true,
                span: (export.span.start, export.span.end),
            });
        }
    } else if export.declaration.is_none() {
        // Specifier-only exports without source (export { X, Y })
        // These are local re-exports: the local names are user-exported.
        for spec in &export.specifiers {
            let local_name = match &spec.local {
                ModuleExportName::IdentifierName(id) => id.name.as_str().to_string(),
                ModuleExportName::IdentifierReference(id) => id.name.as_str().to_string(),
                ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
            };
            ctx.exported_local_names.insert(local_name);
        }
    }
}

/// Collect information from a default export declaration.
fn collect_default_export(ctx: &mut CollectContext, export: &ExportDefaultDeclaration<'_>) {
    ctx.module_exports.push(ExportInfo {
        name: "default".to_string(),
        is_reexport: false,
        span: (export.span.start, export.span.end),
    });

    match &export.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(fn_decl) => {
            if let Some(ident) = &fn_decl.id {
                let name = ident.name.as_str().to_string();
                ctx.module_level_decls.insert(name.clone());
                // SWC treats `export default function X` as making X exported
                // for _auto_ prefix purposes (X won't get _auto_ prefix).
                ctx.exported_local_names.insert(name);
            }
        }
        ExportDefaultDeclarationKind::ClassDeclaration(class_decl) => {
            if let Some(ident) = &class_decl.id {
                let name = ident.name.as_str().to_string();
                ctx.module_level_decls.insert(name.clone());
                // SWC treats `export default class X` as making X exported
                // for _auto_ prefix purposes.
                ctx.exported_local_names.insert(name);
            }
        }
        _ => {
            if let Some(expr) = export.declaration.as_expression() {
                walk_expression_for_calls(ctx, expr);
            }
        }
    }
}

/// Check if an expression is a `wrap(...)` or `implicit$FirstArg(...)` call.
/// These are the Qwik conventions for creating custom $-APIs from Qrl variants.
fn is_wrap_call(expr: &Expression<'_>) -> bool {
    if let Expression::CallExpression(call) = expr {
        if let Expression::Identifier(ident) = &call.callee {
            let name = ident.name.as_str();
            return name == "wrap" || name == "implicit$FirstArg";
        }
    }
    false
}

/// Extract the first identifier name from a binding pattern.
fn binding_pattern_name(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_string()),
        _ => None,
    }
}

/// Walk a statement to find dollar call sites.
fn walk_statement_for_calls(ctx: &mut CollectContext, stmt: &Statement<'_>) {
    match stmt {
        Statement::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                let var_name = binding_pattern_name(&declarator.id);
                if let Some(ref name) = var_name {
                    if name.ends_with('$')
                        && !ctx.dollar_imports.contains(name)
                        && declarator
                            .init
                            .as_ref()
                            .is_some_and(|init| is_wrap_call(init))
                    {
                        ctx.dollar_imports.insert(name.clone());
                    }
                }
                if let Some(init) = &declarator.init {
                    walk_expression_for_calls(ctx, init);
                }
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            walk_expression_for_calls(ctx, &expr_stmt.expression);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression_for_calls(ctx, arg);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                walk_statement_for_calls(ctx, s);
            }
        }
        Statement::IfStatement(if_stmt) => {
            walk_expression_for_calls(ctx, &if_stmt.test);
            walk_statement_for_calls(ctx, &if_stmt.consequent);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement_for_calls(ctx, alt);
            }
        }
        Statement::ExportNamedDeclaration(export) => {
            collect_named_export(ctx, export);
        }
        Statement::ExportDefaultDeclaration(export) => {
            collect_default_export(ctx, export);
        }
        Statement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                for s in &body.statements {
                    walk_statement_for_calls(ctx, s);
                }
            }
        }
        _ => {}
    }
}

/// Walk an expression to find dollar call sites.
fn walk_expression_for_calls(ctx: &mut CollectContext, expr: &Expression<'_>) {
    match expr {
        Expression::CallExpression(call) => {
            walk_expression_for_calls(ctx, &call.callee);
            for arg in &call.arguments {
                walk_argument_for_calls(ctx, arg);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            for stmt in &arrow.body.statements {
                walk_statement_for_calls(ctx, stmt);
            }
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                for stmt in &body.statements {
                    walk_statement_for_calls(ctx, stmt);
                }
            }
        }
        Expression::ParenthesizedExpression(paren) => {
            walk_expression_for_calls(ctx, &paren.expression);
        }
        Expression::SequenceExpression(seq) => {
            for expr in &seq.expressions {
                walk_expression_for_calls(ctx, expr);
            }
        }
        Expression::ConditionalExpression(cond) => {
            walk_expression_for_calls(ctx, &cond.test);
            walk_expression_for_calls(ctx, &cond.consequent);
            walk_expression_for_calls(ctx, &cond.alternate);
        }
        Expression::AssignmentExpression(assign) => {
            walk_expression_for_calls(ctx, &assign.right);
        }
        Expression::LogicalExpression(logical) => {
            walk_expression_for_calls(ctx, &logical.left);
            walk_expression_for_calls(ctx, &logical.right);
        }
        Expression::BinaryExpression(binary) => {
            walk_expression_for_calls(ctx, &binary.left);
            walk_expression_for_calls(ctx, &binary.right);
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        walk_expression_for_calls(ctx, &spread.argument);
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(expr) = elem.as_expression() {
                            walk_expression_for_calls(ctx, expr);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        walk_expression_for_calls(ctx, &p.value);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        walk_expression_for_calls(ctx, &spread.argument);
                    }
                }
            }
        }
        Expression::TemplateLiteral(tmpl) => {
            for expr in &tmpl.expressions {
                walk_expression_for_calls(ctx, expr);
            }
        }
        Expression::TaggedTemplateExpression(tagged) => {
            walk_expression_for_calls(ctx, &tagged.tag);
        }
        Expression::JSXElement(jsx) => {
            walk_jsx_element_for_calls(ctx, jsx);
        }
        Expression::JSXFragment(jsx) => {
            walk_jsx_children_for_calls(ctx, &jsx.children);
        }
        _ => {}
    }
}

/// Walk a call argument for dollar call sites.
fn walk_argument_for_calls(ctx: &mut CollectContext, arg: &Argument<'_>) {
    match arg {
        Argument::SpreadElement(spread) => {
            walk_expression_for_calls(ctx, &spread.argument);
        }
        _ => {
            if let Some(expr) = arg.as_expression() {
                walk_expression_for_calls(ctx, expr);
            }
        }
    }
}

/// Walk a JSXExpression to find dollar calls.
///
/// JSXExpression uses `inherit_variants!` from Expression in OXC 0.113,
/// meaning all Expression variants are directly on JSXExpression. We handle
/// call expressions specifically and delegate the rest.
fn walk_jsx_expression_for_calls(ctx: &mut CollectContext, jsx_expr: &JSXExpression<'_>) {
    match jsx_expr {
        JSXExpression::EmptyExpression(_) => {}
        // JSXExpression inherits all Expression variants via inherit_variants! macro.
        JSXExpression::CallExpression(call) => {
            walk_expression_for_calls(ctx, &call.callee);
            for arg in &call.arguments {
                walk_argument_for_calls(ctx, arg);
            }
        }
        JSXExpression::ArrowFunctionExpression(arrow) => {
            for stmt in &arrow.body.statements {
                walk_statement_for_calls(ctx, stmt);
            }
        }
        JSXExpression::JSXElement(el) => {
            walk_jsx_element_for_calls(ctx, el);
        }
        JSXExpression::JSXFragment(frag) => {
            walk_jsx_children_for_calls(ctx, &frag.children);
        }
        _ => {}
    }
}

/// Walk JSX element and its children for wrap() definitions.
fn walk_jsx_element_for_calls(ctx: &mut CollectContext, element: &JSXElement<'_>) {
    for attr in &element.opening_element.attributes {
        if let JSXAttributeItem::Attribute(attr) = attr {
            if let Some(value) = &attr.value {
                if let JSXAttributeValue::ExpressionContainer(container) = value {
                    walk_jsx_expression_for_calls(ctx, &container.expression);
                }
            }
        }
    }
    walk_jsx_children_for_calls(ctx, &element.children);
}


/// Walk JSX children for dollar calls.
fn walk_jsx_children_for_calls<'a>(
    ctx: &mut CollectContext,
    children: &oxc::allocator::Vec<'a, JSXChild<'a>>,
) {
    for child in children {
        match child {
            JSXChild::Element(el) => {
                walk_jsx_element_for_calls(ctx, el);
            }
            JSXChild::Fragment(frag) => {
                walk_jsx_children_for_calls(ctx, &frag.children);
            }
            JSXChild::ExpressionContainer(container) => {
                walk_jsx_expression_for_calls(ctx, &container.expression);
            }
            _ => {}
        }
    }
}

