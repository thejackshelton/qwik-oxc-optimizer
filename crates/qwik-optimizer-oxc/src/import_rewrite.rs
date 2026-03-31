//! Import mutation logic.
//!
//! Transform import declarations after the main traversal. Removes `$`-suffixed
//! imports that were consumed by the transform, adds new Qrl-suffixed imports,
//! and adds `qrl` or `inlinedQrl` imports as needed.
//!
//! Also provides AST builder functions for constructing QRL call expressions,
//! named imports, and lazy import declarations.

use oxc::ast::ast::*;
use oxc::span::SPAN;
use oxc_traverse::TraverseCtx;

/// Dev mode metadata for QRL calls.
/// When present, the QRL function name gets a DEV suffix and an extra
/// metadata object argument is added: `{ file, lo, hi, displayName }`.
pub(crate) struct QrlDevMetadata {
    /// Absolute file path (e.g., "/user/qwik/src/test.tsx")
    pub file: String,
    /// Byte offset of the $()-call body start
    pub lo: u32,
    /// Byte offset of the $()-call body end
    pub hi: u32,
    /// Display name (e.g., "test.tsx_App_component")
    pub display_name: String,
}

/// Build a dev metadata object expression: `{ file: "...", lo: N, hi: N, displayName: "..." }`
fn build_dev_metadata_object<'a>(
    meta: &QrlDevMetadata,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let mut props = ctx.ast.vec_with_capacity(4);

    // file
    let file_atom = ctx.ast.atom(&meta.file);
    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("file")),
        ctx.ast.expression_string_literal(SPAN, file_atom, None),
        false, false, false,
    ));

    // lo
    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("lo")),
        ctx.ast.expression_numeric_literal(SPAN, meta.lo as f64, None, oxc::syntax::number::NumberBase::Decimal),
        false, false, false,
    ));

    // hi
    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("hi")),
        ctx.ast.expression_numeric_literal(SPAN, meta.hi as f64, None, oxc::syntax::number::NumberBase::Decimal),
        false, false, false,
    ));

    // displayName
    let dn_atom = ctx.ast.atom(&meta.display_name);
    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("displayName")),
        ctx.ast.expression_string_literal(SPAN, dn_atom, None),
        false, false, false,
    ));

    ctx.ast.expression_object(SPAN, props)
}

/// Dev mode location metadata for JSX elements.
/// Added as an extra argument to `_jsxSorted` calls: `{ fileName, lineNumber, columnNumber }`.
pub(crate) struct JsxDevLocation {
    /// File name (relative path, e.g., "test.tsx" or "project/index.tsx")
    pub file_name: String,
    /// 1-based line number
    pub line_number: u32,
    /// 1-based column number
    pub column_number: u32,
}

/// Build a JSX dev location object: `{ fileName: "...", lineNumber: N, columnNumber: N }`
pub(crate) fn build_jsx_dev_location<'a>(
    loc: &JsxDevLocation,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let mut props = ctx.ast.vec_with_capacity(3);

    let fn_atom = ctx.ast.atom(&loc.file_name);
    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("fileName")),
        ctx.ast.expression_string_literal(SPAN, fn_atom, None),
        false, false, false,
    ));

    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("lineNumber")),
        ctx.ast.expression_numeric_literal(SPAN, loc.line_number as f64, None, oxc::syntax::number::NumberBase::Decimal),
        false, false, false,
    ));

    props.push(ctx.ast.object_property_kind_object_property(
        SPAN,
        PropertyKind::Init,
        ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("columnNumber")),
        ctx.ast.expression_numeric_literal(SPAN, loc.column_number as f64, None, oxc::syntax::number::NumberBase::Decimal),
        false, false, false,
    ));

    ctx.ast.expression_object(SPAN, props)
}

/// Build a segment-strategy QRL call expression:
///   `qrl(i_hashValue, "SegmentName_hash")`                        -- no captures
///   `qrl(i_hashValue, "SegmentName_hash", [captured_vars])`       -- with captures
///   `qrlDEV(i_hashValue, "SegmentName_hash", {dev_meta})`         -- dev mode, no captures
///   `qrlDEV(i_hashValue, "SegmentName_hash", {dev_meta}, [caps])` -- dev mode, with captures
///
/// The string parameters are allocated into the arena via `ctx.ast.atom()`.
pub(crate) fn build_qrl_call<'a>(
    import_ident_name: &str,
    segment_export_name: &str,
    captures: &[String],
    dev_meta: Option<&QrlDevMetadata>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let import_atom = ctx.ast.atom(import_ident_name);
    let name_atom = ctx.ast.atom(segment_export_name);

    let import_ref = ctx.ast.expression_identifier(SPAN, import_atom);

    let name_literal = ctx.ast.expression_string_literal(SPAN, name_atom, None);

    let capacity = 2 + dev_meta.is_some() as usize + (!captures.is_empty()) as usize;
    let mut arguments = ctx.ast.vec_with_capacity(capacity);
    arguments.push(Argument::from(import_ref));
    arguments.push(Argument::from(name_literal));

    // In dev mode, insert metadata object before captures
    if let Some(meta) = dev_meta {
        arguments.push(Argument::from(build_dev_metadata_object(meta, ctx)));
    }

    if !captures.is_empty() {
        let mut elements = ctx.ast.vec_with_capacity(captures.len());
        for capture_name in captures {
            let cap_atom = ctx.ast.atom(capture_name.as_str());
            elements.push(ArrayExpressionElement::from(
                ctx.ast.expression_identifier(SPAN, cap_atom),
            ));
        }
        arguments.push(Argument::from(ctx.ast.expression_array(SPAN, elements)));
    }

    let callee_name = if dev_meta.is_some() { "qrlDEV" } else { "qrl" };
    let callee = ctx.ast.expression_identifier(SPAN, callee_name);

    ctx.ast.expression_call_with_pure(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
        true, // pure annotation
    )
}

/// Build an inline-strategy QRL call expression:
///   `inlinedQrl(() => { body }, "Name_hash")`                           -- no captures
///   `inlinedQrl(() => { body }, "Name_hash", [captured_vars])`          -- with captures
///   `inlinedQrlDEV(() => { body }, "Name_hash", {dev_meta})`            -- dev mode
///   `inlinedQrlDEV(() => { body }, "Name_hash", {dev_meta}, [caps])`    -- dev mode + captures
pub(crate) fn build_inlined_qrl_call<'a>(
    body_expr: Expression<'a>,
    segment_name: &str,
    captures: &[String],
    dev_meta: Option<&QrlDevMetadata>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let name_atom = ctx.ast.atom(segment_name);

    let capacity = 2 + dev_meta.is_some() as usize + (!captures.is_empty()) as usize;
    let mut arguments = ctx.ast.vec_with_capacity(capacity);

    arguments.push(Argument::from(body_expr));

    arguments.push(Argument::from(
        ctx.ast.expression_string_literal(SPAN, name_atom, None),
    ));

    // In dev mode, insert metadata object before captures
    if let Some(meta) = dev_meta {
        arguments.push(Argument::from(build_dev_metadata_object(meta, ctx)));
    }

    if !captures.is_empty() {
        let mut elements = ctx.ast.vec_with_capacity(captures.len());
        for capture_name in captures {
            let cap_atom = ctx.ast.atom(capture_name.as_str());
            elements.push(ArrayExpressionElement::from(
                ctx.ast.expression_identifier(SPAN, cap_atom),
            ));
        }
        arguments.push(Argument::from(ctx.ast.expression_array(SPAN, elements)));
    }

    let callee_name = if dev_meta.is_some() { "inlinedQrlDEV" } else { "inlinedQrl" };
    let callee = ctx.ast.expression_identifier(SPAN, callee_name);

    ctx.ast.expression_call_with_pure(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
        true, // pure annotation
    )
}

/// Build a named import declaration:
///   `import { name } from "source"`
pub(crate) fn build_named_import<'a>(
    name: &str,
    source: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let name_atom = ctx.ast.atom(name);
    let source_atom = ctx.ast.atom(source);

    let local = ctx.ast.binding_identifier(SPAN, name_atom.clone());

    let imported = ctx.ast.module_export_name_identifier_name(SPAN, name_atom);

    let specifier = ctx
        .ast
        .import_specifier(SPAN, imported, local, ImportOrExportKind::Value);

    let specifiers = ctx.ast.vec1(ImportDeclarationSpecifier::ImportSpecifier(
        ctx.ast.alloc(specifier),
    ));

    let source_lit = ctx.ast.string_literal(SPAN, source_atom, None);

    let import_decl = ctx.ast.module_declaration_import_declaration(
        SPAN,
        Some(specifiers),
        source_lit,
        None,
        None::<oxc::allocator::Box<'a, WithClause<'a>>>,
        ImportOrExportKind::Value,
    );

    Statement::from(import_decl)
}

/// Build an aliased import declaration:
///   `import { imported_name as local_name } from "source"`
///
/// Used for `import { Fragment as _Fragment } from "@qwik.dev/core/jsx-runtime"`.
pub(crate) fn build_aliased_import<'a>(
    imported_name: &str,
    local_name: &str,
    source: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let imported_atom = ctx.ast.atom(imported_name);
    let local_atom = ctx.ast.atom(local_name);
    let source_atom = ctx.ast.atom(source);

    let local = ctx.ast.binding_identifier(SPAN, local_atom);

    let imported = ctx
        .ast
        .module_export_name_identifier_name(SPAN, imported_atom);

    let specifier = ctx
        .ast
        .import_specifier(SPAN, imported, local, ImportOrExportKind::Value);

    let specifiers = ctx.ast.vec1(ImportDeclarationSpecifier::ImportSpecifier(
        ctx.ast.alloc(specifier),
    ));

    let source_lit = ctx.ast.string_literal(SPAN, source_atom, None);

    let import_decl = ctx.ast.module_declaration_import_declaration(
        SPAN,
        Some(specifiers),
        source_lit,
        None,
        None::<oxc::allocator::Box<'a, WithClause<'a>>>,
        ImportOrExportKind::Value,
    );

    Statement::from(import_decl)
}

/// Build a multi-specifier import declaration:
///   `import { spec1, spec2, imported3 as local3 } from "source"`
///
/// Each specifier is a `(imported_name, local_name)` pair. When imported == local,
/// it's a simple specifier; otherwise an aliased specifier.
pub(crate) fn build_multi_specifier_import<'a>(
    specifier_pairs: &[(String, String)],
    source: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let source_atom = ctx.ast.atom(source);
    let mut specifiers = ctx.ast.vec_with_capacity(specifier_pairs.len());

    for (imported_name, local_name) in specifier_pairs {
        let imported_atom = ctx.ast.atom(imported_name.as_str());
        let local_atom = ctx.ast.atom(local_name.as_str());
        let local = ctx.ast.binding_identifier(SPAN, local_atom);
        let imported = ctx
            .ast
            .module_export_name_identifier_name(SPAN, imported_atom);
        let specifier = ctx
            .ast
            .import_specifier(SPAN, imported, local, ImportOrExportKind::Value);
        specifiers.push(ImportDeclarationSpecifier::ImportSpecifier(
            ctx.ast.alloc(specifier),
        ));
    }

    let source_lit = ctx.ast.string_literal(SPAN, source_atom, None);
    let import_decl = ctx.ast.module_declaration_import_declaration(
        SPAN,
        Some(specifiers),
        source_lit,
        None,
        None::<oxc::allocator::Box<'a, WithClause<'a>>>,
        ImportOrExportKind::Value,
    );

    Statement::from(import_decl)
}

/// Build a lazy import declaration:
///   `const i_hash = () => import("./path_segment_hash")`
pub(crate) fn build_lazy_import_declaration<'a>(
    hash: &str,
    import_path: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let ident_name = format!("i_{}", hash);
    let ident_atom = ctx.ast.atom(&ident_name);
    let path_atom = ctx.ast.atom(import_path);

    let import_source = ctx.ast.expression_string_literal(SPAN, path_atom, None);
    let import_expr = ctx.ast.expression_import(
        SPAN,
        import_source,
        None, // no options
        None, // no phase
    );

    let params = ctx.ast.formal_parameters(
        SPAN,
        FormalParameterKind::ArrowFormalParameters,
        ctx.ast.vec(), // no parameters
        None::<oxc::allocator::Box<'a, FormalParameterRest<'a>>>,
    );

    let expr_stmt = ctx.ast.statement_expression(SPAN, import_expr);
    let body = ctx.ast.function_body(
        SPAN,
        ctx.ast.vec(),           // no directives
        ctx.ast.vec1(expr_stmt), // single expression statement
    );

    let arrow = ctx.ast.expression_arrow_function(
        SPAN,
        true,  // expression body
        false, // not async
        None::<oxc::allocator::Box<'a, TSTypeParameterDeclaration<'a>>>,
        params,
        None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
        body,
    );

    let binding = ctx.ast.binding_pattern_binding_identifier(SPAN, ident_atom);
    let declarator = ctx.ast.variable_declarator(
        SPAN,
        VariableDeclarationKind::Const,
        binding,
        None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
        Some(arrow),
        false,
    );
    let declaration = ctx.ast.variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        ctx.ast.vec1(declarator),
        false,
    );

    Statement::from(Declaration::VariableDeclaration(ctx.ast.alloc(declaration)))
}

/// Build a _wrapProp(signal) call expression (Form 1: signal.value access).
///
/// Strips the `.value` access and passes just the signal identifier.
/// Used when a JSX prop value is `signal.value`.
pub(crate) fn build_wrap_prop_call<'a>(
    signal_expr: Expression<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let callee = ctx.ast.expression_identifier(SPAN, "_wrapProp");
    let mut args = ctx.ast.vec_with_capacity(1);
    args.push(Argument::from(signal_expr));

    ctx.ast.expression_call(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        args,
        false,
    )
}

/// Build a _wrapProp(source, "propName") call expression (Form 2: named property wrapping).
///
/// Used when a JSX prop value is `_rawProps.propName` or `store.propName`.
pub(crate) fn build_wrap_prop_call_named<'a>(
    source_expr: Expression<'a>,
    prop_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let callee = ctx.ast.expression_identifier(SPAN, "_wrapProp");
    let prop_atom = ctx.ast.atom(prop_name);
    let mut args = ctx.ast.vec_with_capacity(2);
    args.push(Argument::from(source_expr));
    args.push(Argument::from(
        ctx.ast.expression_string_literal(SPAN, prop_atom, None),
    ));

    ctx.ast.expression_call(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        args,
        false,
    )
}

/// Build a _noopQrl call expression for stripped segments:
///   `/*#__PURE__*/ _noopQrl("s_HASH")`                               -- no captures
///   `/*#__PURE__*/ _noopQrl("s_HASH", [captured_vars])`              -- with captures
///   `/*#__PURE__*/ _noopQrlDEV("s_HASH", {dev_meta})`                -- dev mode
///   `/*#__PURE__*/ _noopQrlDEV("s_HASH", {dev_meta}, [caps])`        -- dev mode + captures
pub(crate) fn build_noop_qrl_call<'a>(
    segment_name: &str,
    captures: &[String],
    dev_meta: Option<&QrlDevMetadata>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let name_atom = ctx.ast.atom(segment_name);
    let name_literal = ctx.ast.expression_string_literal(SPAN, name_atom, None);

    let capacity = 1 + dev_meta.is_some() as usize + (!captures.is_empty()) as usize;
    let mut arguments = ctx.ast.vec_with_capacity(capacity);
    arguments.push(Argument::from(name_literal));

    // In dev mode, insert metadata object before captures
    if let Some(meta) = dev_meta {
        arguments.push(Argument::from(build_dev_metadata_object(meta, ctx)));
    }

    if !captures.is_empty() {
        let mut elements = ctx.ast.vec_with_capacity(captures.len());
        for cap in captures {
            let cap_atom = ctx.ast.atom(cap);
            let cap_ref = ctx.ast.expression_identifier(SPAN, cap_atom);
            elements.push(ArrayExpressionElement::from(cap_ref));
        }
        let captures_array = ctx.ast.expression_array(SPAN, elements);
        arguments.push(Argument::from(captures_array));
    }

    let callee_name = if dev_meta.is_some() { "_noopQrlDEV" } else { "_noopQrl" };
    let noop_atom = ctx.ast.atom(callee_name);
    let noop_callee = ctx.ast.expression_identifier(SPAN, noop_atom);
    ctx.ast.expression_call_with_pure(
        SPAN,
        noop_callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
        true, // PURE annotation
    )
}

/// Build a _qrlSync call expression for sync$ serialization:
///   `_qrlSync(fn, "stringified_fn")`
///
/// Note: _qrlSync does NOT get a PURE annotation (sync handlers are side-effectful).
pub(crate) fn build_qrl_sync_call<'a>(
    fn_expr: Expression<'a>,
    minified_string: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let str_atom = ctx.ast.atom(minified_string);
    let str_literal = ctx.ast.expression_string_literal(SPAN, str_atom, None);

    let mut arguments = ctx.ast.vec_with_capacity(2);
    arguments.push(Argument::from(fn_expr));
    arguments.push(Argument::from(str_literal));

    let sync_atom = ctx.ast.atom("_qrlSync");
    let sync_callee = ctx.ast.expression_identifier(SPAN, sync_atom);
    ctx.ast.expression_call(
        SPAN,
        sync_callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
    )
}
