//! Component props transformation.
//!
//! Transform component props destructuring patterns. When a component uses
//! destructured props, the optimizer may need to transform the destructuring
//! to preserve reactivity.
//!
//! This module provides:
//! - `analyze_props_destructuring()`: Pure analysis of an arrow function's first
//!   parameter to detect ObjectPattern destructuring and extract prop info.
//! - `rewrite_props_references()`: Recursive walk that replaces IdentifierReference
//!   nodes matching local aliases with `_rawProps.originalKey` member expressions.

use oxc::ast::ast::*;
use oxc::span::SPAN;
use oxc_traverse::TraverseCtx;

/// Information extracted from analyzing a component$'s destructured props parameter.
#[derive(Debug, Clone)]
pub(crate) struct PropsDestructuringInfo {
    /// Whether the parameter needs transformation (is an ObjectPattern).
    pub needs_transform: bool,

    /// Pairs of (original_key, local_alias) from the ObjectPattern properties.
    /// E.g., `{foo}` -> ("foo", "foo"), `{count: c}` -> ("count", "c").
    pub prop_keys: Vec<(String, String)>,

    /// The rest variable name if a rest pattern exists.
    /// E.g., `{...rest}` -> Some("rest").
    pub rest_name: Option<String>,

    /// The name for the raw props parameter (normally "_rawProps").
    pub raw_props_name: String,
}

impl Default for PropsDestructuringInfo {
    fn default() -> Self {
        Self {
            needs_transform: false,
            prop_keys: Vec::new(),
            rest_name: None,
            raw_props_name: "_rawProps".to_string(),
        }
    }
}

/// Analyze the first parameter of a component$ arrow function to detect
/// destructured props that need transformation.
///
/// Returns a `PropsDestructuringInfo` with `needs_transform = true` if the
/// first parameter is an ObjectPattern (destructured props).
///
/// If the first parameter is a plain BindingIdentifier (e.g., `(props) =>`),
/// or there are no parameters, returns `needs_transform = false`.
pub(crate) fn analyze_props_destructuring(params: &FormalParameters<'_>) -> PropsDestructuringInfo {
    let mut info = PropsDestructuringInfo::default();

    if params.items.is_empty() {
        return info;
    }

    let first_param = &params.items[0];

    match &first_param.pattern {
        BindingPattern::ObjectPattern(obj_pat) => {
            info.needs_transform = true;

            for prop in &obj_pat.properties {
                let key_name = extract_property_key_name(&prop.key);
                let local_name = extract_binding_pattern_name(&prop.value);

                if let (Some(key), Some(local)) = (key_name, local_name) {
                    info.prop_keys.push((key, local));
                }
            }

            if let Some(rest) = &obj_pat.rest {
                if let Some(rest_name) = extract_binding_pattern_name(&rest.argument) {
                    info.rest_name = Some(rest_name);
                }
            }
        }
        BindingPattern::BindingIdentifier(_) => {}
        _ => {}
    }

    info
}

/// Extract a string name from a PropertyKey.
///
/// Handles:
/// - `StaticIdentifier` (e.g., `foo` in `{foo}` or `count` in `{count: c}`)
/// - `StringLiteral` expression (e.g., `'bind:value'` in `{'bind:value': bv}`)
fn extract_property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(ident) => Some(ident.name.to_string()),
        PropertyKey::StringLiteral(lit) => Some(lit.value.to_string()),
        _ => None,
    }
}

/// Extract a string name from a BindingPattern (only for BindingIdentifier).
fn extract_binding_pattern_name(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => Some(ident.name.to_string()),
        _ => None,
    }
}

/// Rewrite identifier references in an expression, replacing those that match
/// local aliases from destructured props with `_rawProps.originalKey` member
/// expressions.
///
/// This function takes an `&mut Expression` and recursively walks it, replacing
/// `Expression::Identifier(name)` where `name` matches a local alias with
/// `Expression::StaticMemberExpression(_rawProps, originalKey)`.
///
/// The `prop_map` contains (local_alias -> original_key) mappings.
pub(crate) fn rewrite_props_references<'a>(
    expr: &mut Expression<'a>,
    prop_map: &[(String, String)], // (local_alias, original_key)
    raw_props_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match expr {
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            for (local_alias, original_key) in prop_map {
                if name == local_alias {
                    let obj = ctx
                        .ast
                        .expression_identifier(SPAN, ctx.ast.atom(raw_props_name));
                    let prop_name = ctx
                        .ast
                        .identifier_name(SPAN, ctx.ast.atom(original_key.as_str()));
                    let member = ctx
                        .ast
                        .static_member_expression(SPAN, obj, prop_name, false);
                    *expr = Expression::StaticMemberExpression(ctx.ast.alloc(member));
                    return;
                }
            }
        }

        Expression::ArrayExpression(arr) => {
            for elem in arr.elements.iter_mut() {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        rewrite_props_references(
                            &mut spread.argument,
                            prop_map,
                            raw_props_name,
                            ctx,
                        );
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(e) = array_element_as_expression_mut(elem) {
                            rewrite_props_references(e, prop_map, raw_props_name, ctx);
                        }
                    }
                }
            }
        }

        Expression::ObjectExpression(obj) => {
            for prop in obj.properties.iter_mut() {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        rewrite_props_references(&mut p.value, prop_map, raw_props_name, ctx);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        rewrite_props_references(
                            &mut spread.argument,
                            prop_map,
                            raw_props_name,
                            ctx,
                        );
                    }
                }
            }
        }

        Expression::CallExpression(call) => {
            rewrite_props_references(&mut call.callee, prop_map, raw_props_name, ctx);
            rewrite_call_arguments(&mut call.arguments, prop_map, raw_props_name, ctx);
        }

        Expression::BinaryExpression(bin) => {
            rewrite_props_references(&mut bin.left, prop_map, raw_props_name, ctx);
            rewrite_props_references(&mut bin.right, prop_map, raw_props_name, ctx);
        }

        Expression::LogicalExpression(log) => {
            rewrite_props_references(&mut log.left, prop_map, raw_props_name, ctx);
            rewrite_props_references(&mut log.right, prop_map, raw_props_name, ctx);
        }

        Expression::ConditionalExpression(cond) => {
            rewrite_props_references(&mut cond.test, prop_map, raw_props_name, ctx);
            rewrite_props_references(&mut cond.consequent, prop_map, raw_props_name, ctx);
            rewrite_props_references(&mut cond.alternate, prop_map, raw_props_name, ctx);
        }

        Expression::UnaryExpression(unary) => {
            rewrite_props_references(&mut unary.argument, prop_map, raw_props_name, ctx);
        }

        Expression::UpdateExpression(_update) => {}

        Expression::AssignmentExpression(assign) => {
            rewrite_props_references(&mut assign.right, prop_map, raw_props_name, ctx);
        }

        Expression::SequenceExpression(seq) => {
            for e in seq.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, ctx);
            }
        }

        Expression::TemplateLiteral(tmpl) => {
            for e in tmpl.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, ctx);
            }
        }

        Expression::TaggedTemplateExpression(tagged) => {
            rewrite_props_references(&mut tagged.tag, prop_map, raw_props_name, ctx);
            for e in tagged.quasi.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, ctx);
            }
        }

        Expression::ArrowFunctionExpression(arrow) => {
            let shadowed: Vec<String> = arrow
                .params
                .items
                .iter()
                .filter_map(|p| extract_binding_pattern_name(&p.pattern))
                .filter(|name| prop_map.iter().any(|(local, _)| local == name))
                .collect();

            if shadowed.is_empty() {
                rewrite_body_statements(&mut arrow.body.statements, prop_map, raw_props_name, ctx);
            } else {
                let filtered: Vec<(String, String)> = prop_map
                    .iter()
                    .filter(|(local, _)| !shadowed.contains(local))
                    .cloned()
                    .collect();
                if !filtered.is_empty() {
                    rewrite_body_statements(
                        &mut arrow.body.statements,
                        &filtered,
                        raw_props_name,
                        ctx,
                    );
                }
            }
        }

        Expression::ParenthesizedExpression(paren) => {
            rewrite_props_references(&mut paren.expression, prop_map, raw_props_name, ctx);
        }

        Expression::AwaitExpression(aw) => {
            rewrite_props_references(&mut aw.argument, prop_map, raw_props_name, ctx);
        }

        Expression::YieldExpression(y) => {
            if let Some(ref mut arg) = y.argument {
                rewrite_props_references(arg, prop_map, raw_props_name, ctx);
            }
        }

        Expression::NewExpression(ne) => {
            rewrite_props_references(&mut ne.callee, prop_map, raw_props_name, ctx);
            for arg in ne.arguments.iter_mut() {
                match arg {
                    Argument::SpreadElement(spread) => {
                        rewrite_props_references(
                            &mut spread.argument,
                            prop_map,
                            raw_props_name,
                            ctx,
                        );
                    }
                    _ => {
                        if let Some(e) = argument_as_expression_mut(arg) {
                            rewrite_props_references(e, prop_map, raw_props_name, ctx);
                        }
                    }
                }
            }
        }

        Expression::StaticMemberExpression(mem) => {
            rewrite_props_references(&mut mem.object, prop_map, raw_props_name, ctx);
        }

        Expression::ComputedMemberExpression(mem) => {
            rewrite_props_references(&mut mem.object, prop_map, raw_props_name, ctx);
            rewrite_props_references(&mut mem.expression, prop_map, raw_props_name, ctx);
        }

        _ => {}
    }
}

/// Rewrite identifier references in a slice of statements.
pub(crate) fn rewrite_body_statements<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    prop_map: &[(String, String)],
    raw_props_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for stmt in stmts.iter_mut() {
        rewrite_statement(stmt, prop_map, raw_props_name, ctx);
    }
}

/// Rewrite identifier references in a single statement.
fn rewrite_statement<'a>(
    stmt: &mut Statement<'a>,
    prop_map: &[(String, String)],
    raw_props_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            rewrite_props_references(&mut expr_stmt.expression, prop_map, raw_props_name, ctx);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(ref mut arg) = ret.argument {
                rewrite_props_references(arg, prop_map, raw_props_name, ctx);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for declarator in decl.declarations.iter_mut() {
                if let Some(ref mut init) = declarator.init {
                    rewrite_props_references(init, prop_map, raw_props_name, ctx);
                }
            }
        }
        Statement::IfStatement(if_stmt) => {
            rewrite_props_references(&mut if_stmt.test, prop_map, raw_props_name, ctx);
            rewrite_statement(&mut if_stmt.consequent, prop_map, raw_props_name, ctx);
            if let Some(ref mut alt) = if_stmt.alternate {
                rewrite_statement(alt, prop_map, raw_props_name, ctx);
            }
        }
        Statement::BlockStatement(block) => {
            rewrite_body_statements(&mut block.body, prop_map, raw_props_name, ctx);
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(ref mut test) = for_stmt.test {
                rewrite_props_references(test, prop_map, raw_props_name, ctx);
            }
            if let Some(ref mut update) = for_stmt.update {
                rewrite_props_references(update, prop_map, raw_props_name, ctx);
            }
            rewrite_statement(&mut for_stmt.body, prop_map, raw_props_name, ctx);
        }
        Statement::WhileStatement(while_stmt) => {
            rewrite_props_references(&mut while_stmt.test, prop_map, raw_props_name, ctx);
            rewrite_statement(&mut while_stmt.body, prop_map, raw_props_name, ctx);
        }
        Statement::SwitchStatement(switch_stmt) => {
            rewrite_props_references(&mut switch_stmt.discriminant, prop_map, raw_props_name, ctx);
            for case in switch_stmt.cases.iter_mut() {
                if let Some(ref mut test) = case.test {
                    rewrite_props_references(test, prop_map, raw_props_name, ctx);
                }
                rewrite_body_statements(&mut case.consequent, prop_map, raw_props_name, ctx);
            }
        }
        Statement::ThrowStatement(throw_stmt) => {
            rewrite_props_references(&mut throw_stmt.argument, prop_map, raw_props_name, ctx);
        }
        _ => {}
    }
}

/// Rewrite identifier references in call expression arguments.
///
/// Since Argument uses inherit_variants! from Expression, each Argument variant
/// corresponds to an Expression variant. We match the ones that can contain
/// identifier references that need rewriting.
fn rewrite_call_arguments<'a>(
    arguments: &mut oxc::allocator::Vec<'a, Argument<'a>>,
    prop_map: &[(String, String)],
    raw_props_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for i in 0..arguments.len() {
        match &arguments[i] {
            Argument::SpreadElement(_spread) => {
                let placeholder = Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                let old = std::mem::replace(&mut arguments[i], placeholder);
                if let Argument::SpreadElement(mut spread) = old {
                    rewrite_props_references(&mut spread.argument, prop_map, raw_props_name, ctx);
                    arguments[i] = Argument::SpreadElement(spread);
                }
            }
            Argument::Identifier(ident) => {
                let name = ident.name.as_str().to_string();
                if let Some((_, original_key)) = prop_map.iter().find(|(local, _)| *local == name) {
                    let obj = ctx
                        .ast
                        .expression_identifier(SPAN, ctx.ast.atom(raw_props_name));
                    let prop_name_ident = ctx
                        .ast
                        .identifier_name(SPAN, ctx.ast.atom(original_key.as_str()));
                    let member =
                        ctx.ast
                            .static_member_expression(SPAN, obj, prop_name_ident, false);
                    let member_expr = Expression::StaticMemberExpression(ctx.ast.alloc(member));
                    arguments[i] = Argument::from(member_expr);
                }
            }
            _ => {
                // rewrite, and put back.
                let placeholder = Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                let old = std::mem::replace(&mut arguments[i], placeholder);
                let mut expr = crate::transform::argument_to_expression(old, ctx);
                rewrite_props_references(&mut expr, prop_map, raw_props_name, ctx);
                arguments[i] = Argument::from(expr);
            }
        }
    }
}

/// Helper: Try to get a mutable Expression reference from an ArrayExpressionElement.
/// ArrayExpressionElement inherits Expression variants via inherit_variants!.
fn array_element_as_expression_mut<'b, 'a>(
    elem: &'b mut ArrayExpressionElement<'a>,
) -> Option<&'b mut Expression<'a>> {
    match elem {
        ArrayExpressionElement::Identifier(ident) => {
            let _ = ident;
            None
        }
        ArrayExpressionElement::SpreadElement(_) => None,
        ArrayExpressionElement::Elision(_) => None,
        _ => None,
    }
}

/// Helper: Try to get a mutable Expression reference from an Argument.
fn argument_as_expression_mut<'b, 'a>(
    _arg: &'b mut Argument<'a>,
) -> Option<&'b mut Expression<'a>> {
    None
}

/// Build a `_restProps(_rawProps, ["key1", "key2"])` call expression.
///
/// If `excluded_keys` is empty, builds `_restProps(_rawProps)` (no array argument).
pub(crate) fn build_rest_props_call<'a>(
    raw_props_name: &str,
    excluded_keys: &[String],
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let callee = ctx
        .ast
        .expression_identifier(SPAN, ctx.ast.atom("_restProps"));

    let raw_props_arg = ctx
        .ast
        .expression_identifier(SPAN, ctx.ast.atom(raw_props_name));

    if excluded_keys.is_empty() {
        let mut args = ctx.ast.vec_with_capacity(1);
        args.push(Argument::from(raw_props_arg));
        ctx.ast.expression_call(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            args,
            false,
        )
    } else {
        let mut array_elements = ctx.ast.vec_with_capacity(excluded_keys.len());
        for key in excluded_keys {
            let key_atom = ctx.ast.atom(key.as_str());
            array_elements.push(ArrayExpressionElement::from(
                ctx.ast.expression_string_literal(SPAN, key_atom, None),
            ));
        }
        let array_expr = ctx.ast.expression_array(SPAN, array_elements);

        let mut args = ctx.ast.vec_with_capacity(2);
        args.push(Argument::from(raw_props_arg));
        args.push(Argument::from(array_expr));
        ctx.ast.expression_call(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            args,
            false,
        )
    }
}

/// Build a `const {rest_name} = _restProps(_rawProps, [...keys])` variable declaration statement.
pub(crate) fn build_rest_props_declaration<'a>(
    rest_name: &str,
    raw_props_name: &str,
    excluded_keys: &[String],
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let rest_call = build_rest_props_call(raw_props_name, excluded_keys, ctx);

    let binding = ctx
        .ast
        .binding_pattern_binding_identifier(SPAN, ctx.ast.atom(rest_name));
    let declarator = ctx.ast.variable_declarator(
        SPAN,
        VariableDeclarationKind::Const,
        binding,
        None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
        Some(rest_call),
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
