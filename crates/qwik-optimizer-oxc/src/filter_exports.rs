//! Export stripping — Stage 2.
//!
//! Strip exports from the module based on the `strip_exports` option.
//! Used for server/client mode where certain exports should be removed.
//!
//! When an export name is in the `strip_exports` list AND the export uses a
//! single declarator (not destructuring), its function/arrow body is replaced
//! with a single `throw "Symbol removed ..."` statement. The export declaration
//! itself is preserved so downstream consumers see the binding but get a runtime
//! error if they call it on the wrong platform.
//!
//! SPEC limitation: only single-declarator `var`/`const`/`let` exports and
//! function declaration exports are stripped. Multi-declarator and class exports
//! are left unchanged.

use oxc::ast::AstBuilder;
use oxc::ast::ast::*;
use oxc::span::SPAN;

/// The error message injected into stripped export bodies.
const STRIP_MESSAGE: &str =
    "Symbol removed by Qwik Optimizer, it can not be called from current platform";

/// Filter (strip) exports from the program AST.
///
/// For each export whose binding name appears in `strip_exports`:
///   - `export const/let/var name = fn` → body replaced with throw stub
///     (single-declarator only; multi-declarator and destructuring are skipped)
///   - `export function name() {...}` → body replaced with throw stub
///
/// Note: `_strip_ctx_name` (Stage 10) is intentionally NOT handled here.
pub(crate) fn filter_exports<'a>(
    program: &mut Program<'a>,
    strip_exports: &[String],
    allocator: &'a oxc::allocator::Allocator,
) {
    if strip_exports.is_empty() {
        return;
    }

    let ast = AstBuilder::new(allocator);

    for stmt in program.body.iter_mut() {
        if let Statement::ExportNamedDeclaration(export_decl) = stmt {
            if let Some(ref mut decl) = export_decl.declaration {
                match decl {
                    Declaration::VariableDeclaration(var_decl) => {
                        // SPEC: single-declarator only. Multi-declarator / destructuring skipped.
                        if var_decl.declarations.len() != 1 {
                            continue;
                        }
                        let declarator = &mut var_decl.declarations[0];
                        if let Some(name) = binding_pattern_name(&declarator.id) {
                            if strip_exports.iter().any(|s| s == name) {
                                replace_init_body(&mut declarator.init, &ast);
                            }
                        }
                    }
                    Declaration::FunctionDeclaration(func_decl) => {
                        if let Some(ref id) = func_decl.id {
                            let name = id.name.as_str();
                            if strip_exports.iter().any(|s| s == name) {
                                replace_function_body(&mut func_decl.body, &ast);
                            }
                        }
                    }
                    _ => {
                        // Class and other declaration types: skip per SPEC.
                    }
                }
            }
        }
    }
}

/// Replace the initializer with a synchronous zero-arg arrow throw stub.
///
/// Per SPEC lines 1025-1028, the stub shape is always:
/// `() => { throw "Symbol removed ..." }`
///
/// This ensures async functions throw synchronously (not reject a promise)
/// and the original arity/constructibility is not preserved.
fn replace_init_body<'a>(init: &mut Option<Expression<'a>>, ast: &AstBuilder<'a>) {
    if init.is_some() {
        *init = Some(build_arrow_throw_stub(ast));
    }
}

/// Build `() => { throw "Symbol removed ..." }` arrow expression.
fn build_arrow_throw_stub<'a>(ast: &AstBuilder<'a>) -> Expression<'a> {
    let throw_body = build_throw_body(ast);
    let params = ast.formal_parameters(
        SPAN,
        FormalParameterKind::ArrowFormalParameters,
        ast.vec(),
        Option::<FormalParameterRest<'a>>::None,
    );
    ast.expression_arrow_function(
        SPAN,
        false, // expression
        false, // async
        Option::<TSTypeParameterDeclaration<'a>>::None,
        params,
        Option::<TSTypeAnnotation<'a>>::None,
        throw_body,
    )
}

/// Replace a function declaration's body with the throw stub.
fn replace_function_body<'a>(
    body: &mut Option<oxc::allocator::Box<'a, FunctionBody<'a>>>,
    ast: &AstBuilder<'a>,
) {
    let throw_body = build_throw_body(ast);
    *body = Some(ast.alloc(throw_body));
}

/// Build a `FunctionBody` containing a single: `throw "Symbol removed ..."`.
fn build_throw_body<'a>(ast: &AstBuilder<'a>) -> FunctionBody<'a> {
    let message = ast.expression_string_literal(SPAN, STRIP_MESSAGE, None);
    let throw_stmt = ast.statement_throw(SPAN, message);
    ast.function_body(SPAN, ast.vec(), ast.vec1(throw_stmt))
}

/// Extract the binding name from a `BindingPattern`, if it is a simple identifier.
///
/// Returns `None` for destructuring patterns (object/array) so they are not
/// accidentally stripped.
fn binding_pattern_name<'a>(pattern: &BindingPattern<'a>) -> Option<&'a str> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str()),
        _ => None,
    }
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

    fn transform(src: &str, names: &[&str]) -> String {
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let ret = Parser::new(&allocator, src, source_type).parse();
        let mut program = ret.program;
        let strip: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        filter_exports(&mut program, &strip, &allocator);
        Codegen::new().build(&program).code
    }

    #[test]
    fn strips_arrow_function_export() {
        let src = "export const onGet = () => { return 42; };";
        let out = transform(src, &["onGet"]);
        assert!(
            out.contains("throw"),
            "Expected throw in output, got: {out}"
        );
        assert!(
            out.contains(STRIP_MESSAGE),
            "Expected STRIP_MESSAGE in output, got: {out}"
        );
        assert!(
            !out.contains("return 42"),
            "Original body should be gone, got: {out}"
        );
    }

    #[test]
    fn strips_function_declaration_export() {
        let src = "export function onGet() { return 42; }";
        let out = transform(src, &["onGet"]);
        assert!(
            out.contains("throw"),
            "Expected throw in output, got: {out}"
        );
        assert!(
            out.contains(STRIP_MESSAGE),
            "Expected STRIP_MESSAGE in output, got: {out}"
        );
        assert!(
            !out.contains("return 42"),
            "Original body should be gone, got: {out}"
        );
    }

    #[test]
    fn does_not_strip_multi_declarator() {
        // Multi-declarator destructuring must NOT be stripped, even if "a" is in list.
        let src = "export const { a, b } = obj;";
        let out = transform(src, &["a"]);
        assert!(
            !out.contains("throw"),
            "Multi-declarator should not be stripped, got: {out}"
        );
        assert!(
            out.contains("obj"),
            "Original initializer should be intact, got: {out}"
        );
    }

    #[test]
    fn does_not_strip_name_not_in_list() {
        let src = "export const keep = () => 1;";
        let out = transform(src, &["other"]);
        assert!(
            !out.contains("throw"),
            "Unrelated export should not be stripped, got: {out}"
        );
        assert!(
            out.contains("keep"),
            "Export name should be preserved, got: {out}"
        );
    }

    #[test]
    fn strips_non_function_init() {
        // `export const onGet = someFn;` should be replaced with arrow throw stub.
        let src = "export const onGet = someFn;";
        let out = transform(src, &["onGet"]);
        assert!(
            out.contains("throw"),
            "Non-function init should be stripped, got: {out}"
        );
        assert!(
            out.contains(STRIP_MESSAGE),
            "Expected STRIP_MESSAGE in output, got: {out}"
        );
        assert!(
            !out.contains("someFn"),
            "Original init should be gone, got: {out}"
        );
    }

    #[test]
    fn empty_strip_exports_is_noop() {
        let src = "export const onGet = () => { return 42; };";
        let out = transform(src, &[]);
        assert!(
            !out.contains("throw"),
            "Empty strip_exports must be a no-op, got: {out}"
        );
        assert!(
            out.contains("return 42"),
            "Body should be intact, got: {out}"
        );
    }

    #[test]
    fn preserves_export_declaration_structure() {
        // After stripping, the export keyword and binding name are preserved.
        let src = "export const onGet = () => { return 42; };";
        let out = transform(src, &["onGet"]);
        assert!(
            out.contains("export"),
            "export keyword must be preserved, got: {out}"
        );
        assert!(
            out.contains("onGet"),
            "Binding name must be preserved, got: {out}"
        );
    }
}
