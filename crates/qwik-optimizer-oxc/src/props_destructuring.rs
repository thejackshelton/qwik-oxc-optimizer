//! Props destructuring transformation — Stage 8.
//!
//! Rewrites destructured arrow function parameters to `_rawProps.propName` form.
//! When a component-style arrow function has a single ObjectPattern parameter,
//! this stage rewrites it to `(_rawProps) =>` with `const propName = _rawProps.propName;`
//! declarations prepended to the body.
//!
//! This runs for ALL modes including `EmitMode::Lib`.
//!
//! SPEC lines 1132–1191.

use oxc::allocator::{Allocator, CloneIn, Vec as ArenaVec};
use oxc::ast::AstBuilder;
use oxc::ast::ast::*;
use oxc::ast_visit::VisitMut;
use oxc::span::SPAN;

use crate::collector::{GlobalCollect, Import, ImportKind};
use crate::is_const::is_const_expression;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Transform destructured arrow function props to `_rawProps` form.
///
/// No mode gate — runs for ALL modes including Lib.
pub(crate) fn transform_props_destructuring<'a>(
    program: &mut Program<'a>,
    collect: &mut GlobalCollect,
    core_module: &str,
    allocator: &'a Allocator,
) {
    let ast = AstBuilder::new(allocator);
    let mut visitor = PropsDestructurer {
        ast,
        allocator,
        core_module: core_module.to_string(),
        needs_rest_props_import: false,
    };
    visitor.visit_program(program);

    if visitor.needs_rest_props_import {
        collect.add_synthetic_import(
            "_restProps".to_string(),
            Import {
                source: core_module.to_string(),
                specifier: "_restProps".to_string(),
                kind: ImportKind::Named,
                synthetic: true,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// PropsDestructurer visitor
// ---------------------------------------------------------------------------

struct PropsDestructurer<'a> {
    ast: AstBuilder<'a>,
    allocator: &'a Allocator,
    core_module: String,
    needs_rest_props_import: bool,
}

impl<'a> VisitMut<'a> for PropsDestructurer<'a> {
    fn visit_arrow_function_expression(&mut self, node: &mut ArrowFunctionExpression<'a>) {
        // Walk children FIRST: transform nested arrows before outer ones.
        oxc::ast_visit::walk_mut::walk_arrow_function_expression(self, node);

        // Must have exactly 1 parameter.
        if node.params.items.len() != 1 {
            return;
        }

        // Body must qualify (expression body, or block body with return).
        if !qualifies(&node.body, node.expression) {
            return;
        }

        // Skip pre-compiled lib bodies: first stmt is `const x = _captures[N]`.
        if is_precompiled_lib_body(&node.body) {
            return;
        }

        // Extract the single param's ObjectPattern.
        let obj_pattern = match &node.params.items[0].pattern {
            BindingPattern::ObjectPattern(obj) => obj,
            _ => return,
        };

        // Collect prop info: (key_name, local_name, default_expr_is_const).
        // We also need to detect non-const defaults early to abort.
        struct PropInfo {
            key_name: String,
            local_name: String,
            has_default: bool,
        }

        let mut props: Vec<PropInfo> = Vec::new();

        for prop in &obj_pattern.properties {
            // Extract key name.
            let key_name = match &prop.key {
                PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(),
                PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
                _ => return, // Computed keys not supported.
            };

            // Extract local name and optional default.
            match &prop.value {
                BindingPattern::BindingIdentifier(id) => {
                    props.push(PropInfo {
                        key_name,
                        local_name: id.name.as_str().to_string(),
                        has_default: false,
                    });
                }
                BindingPattern::AssignmentPattern(assign) => {
                    // Check default value is const.
                    if !is_const_expression(&assign.right) {
                        return; // Non-const default — skip entire arrow.
                    }
                    // Local name from left side.
                    let local_name = match &assign.left {
                        BindingPattern::BindingIdentifier(id) => id.name.as_str().to_string(),
                        _ => return,
                    };
                    props.push(PropInfo {
                        key_name,
                        local_name,
                        has_default: true,
                    });
                }
                _ => return, // Nested destructuring not supported.
            }
        }

        // Extract rest element name if present.
        let rest_name: Option<String> = obj_pattern.rest.as_ref().and_then(|rest| {
            match &rest.argument {
                BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_string()),
                _ => None,
            }
        });

        // --- Build const declarations ---
        // We need to clone default expressions from the existing params.
        // Collect defaults upfront before we mutate params.
        // We'll reconstruct the expressions using the same allocator.
        // Since we can't easily clone arena-allocated AST, we parse the defaults
        // by collecting them before mutating. We use TakeIn to take ownership.
        // Actually, we'll rebuild the expressions inline after replacing params.

        // Strategy: collect the key/local/default data from params, then replace
        // the parameter, then build the const declarations.
        //
        // For defaults, we need to take the expression out of the AssignmentPattern.
        // OXC 0.113 doesn't have a simple Clone, but we can take the value using
        // std::mem::replace since we're rewriting the entire param anyway.

        // Build the statements to prepend.
        let mut new_stmts: ArenaVec<'a, Statement<'a>> = ArenaVec::new_in(self.allocator);

        // Step 1: If there's a rest element, build `const rest = _restProps(_rawProps, [key1, key2, ...])`
        if let Some(ref rest_local) = rest_name {
            let rest_stmt = self.build_rest_props_stmt(rest_local, &props.iter().map(|p| p.key_name.clone()).collect::<Vec<_>>());
            new_stmts.push(rest_stmt);
            self.needs_rest_props_import = true;
        }

        // Step 2: Build const declarations for each property.
        // We need the default expressions. Since we can't easily clone them,
        // we'll take them from the param before we overwrite it.
        // Collect defaults from the param list (take the expression).
        let defaults: Vec<Option<Expression<'a>>> = node.params.items.iter_mut()
            .next()
            .map(|param| {
                if let BindingPattern::ObjectPattern(obj) = &mut param.pattern {
                    let mut defs = Vec::new();
                    for p in obj.properties.iter_mut() {
                        match &mut p.value {
                            BindingPattern::AssignmentPattern(assign) => {
                                // Take the right-hand default expression.
                                let dummy = self.ast.expression_boolean_literal(SPAN, false);
                                let default_expr = std::mem::replace(&mut assign.right, dummy);
                                defs.push(Some(default_expr));
                            }
                            _ => defs.push(None),
                        }
                    }
                    defs
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();

        // Build const declarations for each prop.
        for (i, prop_info) in props.iter().enumerate() {
            let default = defaults.get(i).and_then(|d| d.as_ref());
            let stmt = self.build_prop_const_stmt(&prop_info.key_name, &prop_info.local_name, prop_info.has_default, default);
            new_stmts.push(stmt);
        }

        // Step 3: Replace the parameter with `_rawProps`.
        // Replace params.items[0] with a FormalParameter wrapping a BindingIdentifier.
        let raw_props_atom = self.ast.atom("_rawProps");
        let raw_props_binding = self.ast.binding_pattern_binding_identifier(SPAN, raw_props_atom);
        let decorators: ArenaVec<'a, Decorator<'a>> = ArenaVec::new_in(self.allocator);
        let new_param = self.ast.formal_parameter(
            SPAN,
            decorators,
            raw_props_binding,
            None::<TSTypeAnnotation<'a>>,
            None::<Expression<'a>>,
            false,
            None,
            false,
            false,
        );
        node.params.items[0] = new_param;

        // Step 4: Prepend new_stmts to body.statements.
        // We want: new_stmts followed by existing body statements.
        let existing_stmts = std::mem::replace(
            &mut node.body.statements,
            ArenaVec::new_in(self.allocator),
        );

        // If expression body, convert to block body.
        if node.expression {
            // The existing body.statements has exactly 1 ExpressionStatement (the return expr).
            // We need to turn it into a ReturnStatement.
            // The expression body's single statement is already an ExpressionStatement
            // wrapping the return value. We convert it to a ReturnStatement.
            node.expression = false;
            for stmt in new_stmts {
                node.body.statements.push(stmt);
            }
            // Convert the expression statement to a return statement.
            for stmt in existing_stmts {
                match stmt {
                    Statement::ExpressionStatement(expr_stmt) => {
                        let ret_stmt = self.ast.statement_return(
                            expr_stmt.span,
                            Some(expr_stmt.unbox().expression),
                        );
                        node.body.statements.push(ret_stmt);
                    }
                    other => node.body.statements.push(other),
                }
            }
        } else {
            for stmt in new_stmts {
                node.body.statements.push(stmt);
            }
            for stmt in existing_stmts {
                node.body.statements.push(stmt);
            }
        }
    }
}

impl<'a> PropsDestructurer<'a> {
    /// Build `const local = _rawProps.key;` or `const local = _rawProps.key ?? default;`
    fn build_prop_const_stmt(
        &self,
        key_name: &str,
        local_name: &str,
        has_default: bool,
        default_expr: Option<&Expression<'a>>,
    ) -> Statement<'a> {
        // Allocate strings in the arena so they get lifetime 'a.
        let key_atom = self.ast.atom(key_name);
        let local_atom = self.ast.atom(local_name);
        let raw_props_atom = self.ast.atom("_rawProps");

        // Build `_rawProps.key` or `_rawProps["key"]` for non-identifier keys.
        let raw_props = self.ast.expression_identifier(SPAN, raw_props_atom.clone());
        let member = if is_valid_identifier(key_name) {
            let key_ident = self.ast.identifier_name(SPAN, key_atom);
            Expression::StaticMemberExpression(
                self.ast.alloc_static_member_expression(SPAN, raw_props, key_ident, false)
            )
        } else {
            let key_str = self.ast.expression_string_literal(SPAN, key_atom, None);
            Expression::ComputedMemberExpression(
                self.ast.alloc_computed_member_expression(SPAN, raw_props, key_str, false)
            )
        };

        // Build init expression: either member access or member ?? default.
        let init = if has_default {
            if let Some(default) = default_expr {
                // Clone the default expression using CloneIn.
                let cloned_default = default.clone_in(self.allocator);
                Expression::LogicalExpression(
                    self.ast.alloc(self.ast.logical_expression(
                        SPAN,
                        member,
                        LogicalOperator::Coalesce,
                        cloned_default,
                    ))
                )
            } else {
                member
            }
        } else {
            member
        };

        // Build `const local = init;`
        let binding = self.ast.binding_pattern_binding_identifier(SPAN, local_atom);
        let mut declarators: ArenaVec<'a, VariableDeclarator<'a>> = ArenaVec::new_in(self.allocator);
        declarators.push(self.ast.variable_declarator(
            SPAN,
            VariableDeclarationKind::Const,
            binding,
            None::<TSTypeAnnotation<'a>>,
            Some(init),
            false,
        ));
        Statement::VariableDeclaration(
            self.ast.alloc_variable_declaration(SPAN, VariableDeclarationKind::Const, declarators, false)
        )
    }

    /// Build `const rest = _restProps(_rawProps, ["key1", "key2", ...])` or
    /// `const rest = _restProps(_rawProps)` when no regular props.
    fn build_rest_props_stmt(&self, rest_name: &str, excluded_keys: &[String]) -> Statement<'a> {
        let rest_name_atom = self.ast.atom(rest_name);
        let callee = self.ast.expression_identifier(SPAN, self.ast.atom("_restProps"));

        let mut args: ArenaVec<'a, Argument<'a>> = ArenaVec::new_in(self.allocator);

        // First arg: _rawProps
        // Argument inherits from Expression variants via macro.
        args.push(Argument::Identifier(
            self.ast.alloc_identifier_reference(SPAN, self.ast.atom("_rawProps"))
        ));

        // Second arg (only if there are excluded keys): array of string literals.
        if !excluded_keys.is_empty() {
            let mut elements: ArenaVec<'a, ArrayExpressionElement<'a>> = ArenaVec::new_in(self.allocator);
            for key in excluded_keys {
                let key_atom = self.ast.atom(key.as_str());
                elements.push(ArrayExpressionElement::StringLiteral(
                    self.ast.alloc_string_literal(SPAN, key_atom, None)
                ));
            }
            let arr = self.ast.expression_array(SPAN, elements);
            // Since Argument inherits Expression variants, ArrayExpression is Argument::ArrayExpression.
            args.push(Argument::ArrayExpression(
                match arr {
                    Expression::ArrayExpression(boxed) => boxed,
                    _ => unreachable!(),
                }
            ));
        }

        let call_expr = self.ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false);

        // Build `const rest = <call_expr>;`
        let binding = self.ast.binding_pattern_binding_identifier(SPAN, rest_name_atom);
        let mut declarators: ArenaVec<'a, VariableDeclarator<'a>> = ArenaVec::new_in(self.allocator);
        declarators.push(self.ast.variable_declarator(
            SPAN,
            VariableDeclarationKind::Const,
            binding,
            None::<TSTypeAnnotation<'a>>,
            Some(call_expr),
            false,
        ));
        Statement::VariableDeclaration(
            self.ast.alloc_variable_declaration(SPAN, VariableDeclarationKind::Const, declarators, false)
        )
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Returns true if the string is a valid JavaScript identifier (safe for static member access).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Returns true if the arrow body qualifies for props destructuring.
///
/// Triggers per SPEC lines 1140-1164:
/// - Expression body (`node.expression == true`): always qualifies (the expression is the return value).
/// - Block body: must contain at least 1 ReturnStatement.
fn qualifies(body: &FunctionBody<'_>, is_expression: bool) -> bool {
    if is_expression {
        // Expression bodies always qualify — the expression IS the return value.
        return true;
    }
    // Block body: must have at least one return statement.
    body.statements.iter().any(|stmt| matches!(stmt, Statement::ReturnStatement(_)))
}

/// Returns true if the first statement is `const x = _captures[N]`.
///
/// Used to skip pre-compiled lib bodies.
fn is_precompiled_lib_body(body: &FunctionBody<'_>) -> bool {
    let Some(first) = body.statements.first() else {
        return false;
    };
    let Statement::VariableDeclaration(var_decl) = first else {
        return false;
    };
    if var_decl.kind != VariableDeclarationKind::Const {
        return false;
    }
    let Some(declarator) = var_decl.declarations.first() else {
        return false;
    };
    let Some(init) = &declarator.init else {
        return false;
    };
    // Check if init is `_captures[N]`
    if let Expression::ComputedMemberExpression(member) = init {
        if let Expression::Identifier(obj) = &member.object {
            return obj.name.as_str() == "_captures";
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::global_collect;
    use oxc::allocator::Allocator;
    use oxc::codegen::Codegen;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    fn transform(src: &str) -> (String, GlobalCollect) {
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let ret = Parser::new(&allocator, src, source_type).parse();
        let mut program = ret.program;
        let mut collect = global_collect(&program);
        transform_props_destructuring(&mut program, &mut collect, "@qwik.dev/core", &allocator);
        let code = Codegen::new().build(&program).code;
        // SAFETY: We can't return both allocator-owned program and collect without unsafe.
        // For tests we just return the code and a freshly-built collect snapshot.
        // But collect IS modified in-place so we return it.
        (code, collect)
    }

    #[test]
    fn basic_destructuring() {
        let (code, _) = transform(r#"({ foo, bar }) => { return foo; }"#);
        assert!(code.contains("_rawProps"), "Expected _rawProps in: {code}");
        assert!(code.contains("_rawProps.foo"), "Expected _rawProps.foo in: {code}");
        assert!(code.contains("_rawProps.bar"), "Expected _rawProps.bar in: {code}");
        assert!(code.contains("const foo = _rawProps.foo"), "Expected const foo in: {code}");
        assert!(code.contains("const bar = _rawProps.bar"), "Expected const bar in: {code}");
    }

    #[test]
    fn rest_pattern() {
        let (code, collect) = transform(r#"({ foo, ...rest }) => { return rest; }"#);
        assert!(code.contains("_rawProps"), "Expected _rawProps in: {code}");
        assert!(code.contains("_restProps"), "Expected _restProps call in: {code}");
        assert!(code.contains("\"foo\""), "Expected key 'foo' in exclusion array in: {code}");
        assert!(
            collect.synthetic.iter().any(|(name, _)| name == "_restProps"),
            "Expected _restProps in synthetic imports"
        );
    }

    #[test]
    fn rest_only() {
        let (code, collect) = transform(r#"({ ...rest }) => { return rest; }"#);
        assert!(code.contains("_restProps(_rawProps)"), "Expected _restProps single arg in: {code}");
        assert!(
            collect.synthetic.iter().any(|(name, _)| name == "_restProps"),
            "Expected _restProps in synthetic imports"
        );
    }

    #[test]
    fn non_const_default_skip() {
        let (code, _) = transform(r#"({ foo = someFunc() }) => { return foo; }"#);
        assert!(
            !code.contains("_rawProps"),
            "Non-const default should skip transform, got: {code}"
        );
    }

    #[test]
    fn const_default_inlined() {
        let (code, _) = transform(r#"({ foo = 42 }) => { return foo; }"#);
        assert!(code.contains("_rawProps"), "Expected _rawProps in: {code}");
        assert!(code.contains("??"), "Expected ?? (nullish coalesce) in: {code}");
        assert!(code.contains("42"), "Expected default value 42 in: {code}");
    }

    #[test]
    fn precompiled_skip() {
        let src = r#"({ foo }) => { const x = _captures[0]; return foo; }"#;
        let (code, _) = transform(src);
        assert!(
            !code.contains("_rawProps"),
            "Pre-compiled body should skip transform, got: {code}"
        );
    }

    #[test]
    fn renamed_prop() {
        let (code, _) = transform(r#"({ count: c }) => { return c; }"#);
        assert!(code.contains("_rawProps"), "Expected _rawProps in: {code}");
        assert!(code.contains("_rawProps.count"), "Expected _rawProps.count (key) in: {code}");
        assert!(code.contains("const c = _rawProps.count"), "Expected const c in: {code}");
    }

    #[test]
    fn string_literal_key_computed_access() {
        let (code, _) = transform(r#"({ "foo-bar": x }) => { return x; }"#);
        assert!(code.contains("_rawProps"), "Expected _rawProps in: {code}");
        assert!(code.contains(r#"_rawProps["foo-bar"]"#), "Expected computed access _rawProps[\"foo-bar\"] in: {code}");
        assert!(code.contains("const x = _rawProps[\"foo-bar\"]"), "Expected const x in: {code}");
    }

    #[test]
    fn non_object_pattern_skip() {
        let (code, _) = transform(r#"(props) => { return props; }"#);
        assert!(
            !code.contains("_rawProps"),
            "Plain identifier param should not be rewritten, got: {code}"
        );
    }

    #[test]
    fn two_params_skip() {
        let (code, _) = transform(r#"({ a }, { b }) => { return a; }"#);
        assert!(
            !code.contains("_rawProps"),
            "Two params should skip transform, got: {code}"
        );
    }

    #[test]
    fn no_return_skip() {
        let (code, _) = transform(r#"({ foo }) => { foo; }"#);
        assert!(
            !code.contains("_rawProps"),
            "No return/call should skip transform, got: {code}"
        );
    }

    #[test]
    fn expression_call_body() {
        // Expression body with call expression triggers transform.
        let (code, _) = transform(r#"({ foo }) => someCall(foo)"#);
        assert!(code.contains("_rawProps"), "Expression call body should trigger transform, got: {code}");
        assert!(code.contains("const foo = _rawProps.foo"), "Expected const foo in: {code}");
    }
}
