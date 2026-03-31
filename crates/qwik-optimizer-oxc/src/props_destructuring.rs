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
//!   nodes matching local aliases with `_rawProps.originalKey` member expressions
//!   (or `_rawProps.originalKey ?? defaultValue` when a default exists).

use std::collections::{HashMap, HashSet};

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

    /// Default value expressions for props that have const defaults.
    /// Maps local_alias -> default expression source code.
    /// E.g., `{ some = 1 + 2 }` -> { "some": "1 + 2" }
    /// E.g., `{ stuffDefault: hey2 = 123 }` -> { "hey2": "123" }
    pub prop_defaults: HashMap<String, String>,

    /// When the first param is a plain BindingIdentifier (e.g., `props`),
    /// this stores the identifier name. Used for signal wrapping of
    /// `props.class` -> `_wrapProp(props, "class")` patterns.
    pub props_param_name: Option<String>,
}

impl Default for PropsDestructuringInfo {
    fn default() -> Self {
        Self {
            needs_transform: false,
            prop_keys: Vec::new(),
            rest_name: None,
            raw_props_name: "_rawProps".to_string(),
            prop_defaults: HashMap::new(),
            props_param_name: None,
        }
    }
}

/// Information extracted from detecting manual body destructuring of the props parameter.
///
/// E.g., `const { "bind:value": bindValue, test, ...rest } = props;`
#[derive(Debug, Clone)]
pub(crate) struct BodyDestructuringInfo {
    /// Pairs of (original_key, local_alias) from the ObjectPattern.
    pub prop_keys: Vec<(String, String)>,
    /// The rest variable name if a rest pattern exists.
    pub rest_name: Option<String>,
    /// Index of the destructuring statement in the arrow body.
    pub stmt_index: usize,
}

/// Analyze the first parameter of a component$ arrow function to detect
/// destructured props that need transformation.
///
/// Returns a `PropsDestructuringInfo` with `needs_transform = true` if the
/// first parameter is an ObjectPattern (destructured props).
///
/// If the first parameter is a plain BindingIdentifier (e.g., `(props) =>`),
/// or there are no parameters, returns `needs_transform = false`.
///
/// The `import_names` parameter provides the set of known import identifiers
/// in the module, used to determine if a default value expression is const
/// (imports are considered const, matching SWC's behavior).
pub(crate) fn analyze_props_destructuring(
    params: &FormalParameters<'_>,
    import_names: &HashSet<String>,
) -> PropsDestructuringInfo {
    let mut info = PropsDestructuringInfo::default();

    if params.items.is_empty() {
        return info;
    }

    let first_param = &params.items[0];

    match &first_param.pattern {
        BindingPattern::ObjectPattern(obj_pat) => {
            let mut skip = false;

            for prop in &obj_pat.properties {
                let key_name = extract_property_key_name(&prop.key);

                match &prop.value {
                    // Simple identifier: { count } or { count: c }
                    BindingPattern::BindingIdentifier(ident) => {
                        if let Some(key) = key_name {
                            let local = ident.name.to_string();
                            info.prop_keys.push((key, local));
                        }
                    }

                    // Assignment pattern: { some = 3 } or { stuffDefault: hey2 = 123 }
                    BindingPattern::AssignmentPattern(assign) => {
                        if let Some(key) = key_name {
                            // Extract the local name from the left side
                            if let BindingPattern::BindingIdentifier(left_ident) = &assign.left {
                                let local = left_ident.name.to_string();
                                // Check if the default value is a const expression
                                if is_const_default_value(&assign.right, import_names) {
                                    // Serialize the default expression
                                    let default_code = serialize_expression(&assign.right);
                                    info.prop_defaults.insert(local.clone(), default_code);
                                    info.prop_keys.push((key, local));
                                } else {
                                    // Non-const default -> can't transform
                                    skip = true;
                                }
                            } else {
                                // Left side is not a simple identifier (e.g., nested pattern)
                                skip = true;
                            }
                        }
                    }

                    // Nested destructuring: { stuff: { hey } } or { stuff: [a, b] }
                    BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_) => {
                        skip = true;
                    }
                }
            }

            if skip {
                // Can't transform -- leave destructuring unchanged
                return info;
            }

            info.needs_transform = true;

            if let Some(rest) = &obj_pat.rest {
                if let BindingPattern::BindingIdentifier(ident) = &rest.argument {
                    info.rest_name = Some(ident.name.to_string());
                } else {
                    // Non-identifier rest pattern -> can't transform
                    info.needs_transform = false;
                }
            }
        }
        BindingPattern::BindingIdentifier(ident) => {
            let name = ident.name.to_string();
            info.props_param_name = Some(name.clone());
            info.raw_props_name = name;
        }
        _ => {}
    }

    info
}

/// Check if a default value expression is a compile-time constant.
///
/// An expression is considered const if it consists only of:
/// - Literals (string, number, boolean, null, bigint, regexp)
/// - Identifiers that are known imports (they resolve to module-level constants)
/// - Unary operators applied to const sub-expressions
/// - Binary operators applied to const sub-expressions
/// - Parenthesized const expressions
/// - Template literals with only const expressions
///
/// This matches SWC's `is_const_expr` behavior for the props destructuring context.
fn is_const_default_value(expr: &Expression<'_>, import_names: &HashSet<String>) -> bool {
    match expr {
        // Literals are always const
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,

        // Identifiers: const only if they're imports
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            // undefined, NaN, Infinity are const globals
            name == "undefined" || name == "NaN" || name == "Infinity"
                || import_names.contains(name)
        }

        // Unary: const if argument is const
        Expression::UnaryExpression(unary) => {
            is_const_default_value(&unary.argument, import_names)
        }

        // Binary: const if both sides are const
        Expression::BinaryExpression(bin) => {
            is_const_default_value(&bin.left, import_names)
                && is_const_default_value(&bin.right, import_names)
        }

        // Parenthesized: const if inner is const
        Expression::ParenthesizedExpression(paren) => {
            is_const_default_value(&paren.expression, import_names)
        }

        // Template literal: const if all expressions are const
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions.iter().all(|e| is_const_default_value(e, import_names))
        }

        // Everything else (calls, member exprs, arrows, etc.) is NOT const
        _ => false,
    }
}

/// Serialize an expression to source code using OXC Codegen.
fn serialize_expression(expr: &Expression<'_>) -> String {
    let mut codegen = oxc::codegen::Codegen::new();
    codegen.print_expression(expr);
    codegen.into_source_text()
}

/// Build an expression AST node from source code, allocated in the traversal context.
fn build_expression_from_code<'a>(
    code: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Option<Expression<'a>> {
    // For simple cases, build directly without parsing
    // Try to parse as a number
    if let Ok(num) = code.parse::<f64>() {
        return Some(ctx.ast.expression_numeric_literal(
            SPAN,
            num,
            None,
            oxc::syntax::number::NumberBase::Decimal,
        ));
    }

    // Try string literal (starts and ends with quotes)
    if (code.starts_with('"') && code.ends_with('"'))
        || (code.starts_with('\'') && code.ends_with('\''))
    {
        let inner = &code[1..code.len() - 1];
        return Some(
            ctx.ast
                .expression_string_literal(SPAN, ctx.ast.atom(inner), None),
        );
    }

    // For boolean/null/undefined/NaN/Infinity
    match code {
        "true" => return Some(ctx.ast.expression_boolean_literal(SPAN, true)),
        "false" => return Some(ctx.ast.expression_boolean_literal(SPAN, false)),
        "null" => return Some(ctx.ast.expression_null_literal(SPAN)),
        "undefined" | "NaN" | "Infinity" => {
            return Some(ctx.ast.expression_identifier(SPAN, ctx.ast.atom(code)));
        }
        _ => {}
    }

    // For identifiers (single word, no operators)
    if code.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') && !code.is_empty() {
        return Some(ctx.ast.expression_identifier(SPAN, ctx.ast.atom(code)));
    }

    // For complex expressions (binary, unary, etc.), parse in a temporary
    // allocator and rebuild in ctx's allocator.
    build_complex_expression(code, ctx)
}

/// Build a complex expression (binary, unary, etc.) in the traversal context.
/// This handles expressions that can't be represented as simple literals.
fn build_complex_expression<'a>(
    code: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Option<Expression<'a>> {
    // For now, handle the most common patterns:
    // 1. Binary expressions: "1 + 2", "a - b", etc.
    // 2. Unary: "-1", "!true", etc.
    // 3. Identifiers (handled above)

    // Strategy: parse in a temporary allocator, then use Codegen to get the
    // expression string, and re-create in ctx. Since we can't transfer AST
    // nodes between allocators, we'll use a recursive builder.

    let alloc = oxc::allocator::Allocator::default();
    let parse_source_str = format!("var _={code}");
    let source_ref = alloc.alloc_str(&parse_source_str);
    let parser = oxc::parser::Parser::new(&alloc, source_ref, oxc::span::SourceType::mjs());
    let parse_result = parser.parse();

    if !parse_result.errors.is_empty() || parse_result.program.body.is_empty() {
        return None;
    }

    if let Some(Statement::VariableDeclaration(decl)) = parse_result.program.body.first() {
        if let Some(declarator) = decl.declarations.first() {
            if let Some(ref init) = declarator.init {
                return clone_expression_to_ctx(init, ctx);
            }
        }
    }
    None
}

/// Clone an expression from one allocator context to another by rebuilding it.
/// This handles the common expression types needed for default values.
fn clone_expression_to_ctx<'a>(
    expr: &Expression<'_>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Option<Expression<'a>> {
    match expr {
        Expression::NumericLiteral(lit) => Some(ctx.ast.expression_numeric_literal(
            SPAN,
            lit.value,
            None,
            lit.base,
        )),

        Expression::StringLiteral(lit) => Some(ctx.ast.expression_string_literal(
            SPAN,
            ctx.ast.atom(lit.value.as_str()),
            None,
        )),

        Expression::BooleanLiteral(lit) => {
            Some(ctx.ast.expression_boolean_literal(SPAN, lit.value))
        }

        Expression::NullLiteral(_) => Some(ctx.ast.expression_null_literal(SPAN)),

        Expression::Identifier(ident) => Some(
            ctx.ast
                .expression_identifier(SPAN, ctx.ast.atom(ident.name.as_str())),
        ),

        Expression::BinaryExpression(bin) => {
            let left = clone_expression_to_ctx(&bin.left, ctx)?;
            let right = clone_expression_to_ctx(&bin.right, ctx)?;
            Some(ctx.ast.expression_binary(SPAN, left, bin.operator, right))
        }

        Expression::UnaryExpression(unary) => {
            let arg = clone_expression_to_ctx(&unary.argument, ctx)?;
            Some(ctx.ast.expression_unary(SPAN, unary.operator, arg))
        }

        Expression::ParenthesizedExpression(paren) => {
            let inner = clone_expression_to_ctx(&paren.expression, ctx)?;
            Some(ctx.ast.expression_parenthesized(SPAN, inner))
        }

        _ => {
            // For unsupported patterns, fall back to serializing and using an identifier
            // (this shouldn't happen for valid const default values)
            let mut codegen = oxc::codegen::Codegen::new();
            codegen.print_expression(expr);
            let code = codegen.into_source_text();
            Some(ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&code)))
        }
    }
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

/// Build a `_rawProps.key ?? defaultValue` expression.
fn build_nullish_coalesce<'a>(
    raw_props_name: &str,
    original_key: &str,
    default_code: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Option<Expression<'a>> {
    // Build the left side: _rawProps.key
    let obj = ctx
        .ast
        .expression_identifier(SPAN, ctx.ast.atom(raw_props_name));
    let prop_name = ctx
        .ast
        .identifier_name(SPAN, ctx.ast.atom(original_key));
    let member = ctx
        .ast
        .static_member_expression(SPAN, obj, prop_name, false);
    let left = Expression::StaticMemberExpression(ctx.ast.alloc(member));

    // Build the right side: default value
    let right = build_expression_from_code(default_code, ctx)?;

    // Build: left ?? right
    Some(ctx.ast.expression_logical(
        SPAN,
        left,
        oxc::syntax::operator::LogicalOperator::Coalesce,
        right,
    ))
}

/// Rewrite identifier references in an expression, replacing those that match
/// local aliases from destructured props with `_rawProps.originalKey` member
/// expressions, or `_rawProps.originalKey ?? defaultValue` when a default exists.
///
/// The `prop_map` contains (local_alias -> original_key) mappings.
/// The `prop_defaults` contains (local_alias -> default_expression_code) for props with defaults.
pub(crate) fn rewrite_props_references<'a>(
    expr: &mut Expression<'a>,
    prop_map: &[(String, String)], // (local_alias, original_key)
    raw_props_name: &str,
    prop_defaults: &HashMap<String, String>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match expr {
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            for (local_alias, original_key) in prop_map {
                if name == local_alias {
                    // Check if this prop has a default value
                    if let Some(default_code) = prop_defaults.get(local_alias) {
                        if let Some(coalesce) =
                            build_nullish_coalesce(raw_props_name, original_key, default_code, ctx)
                        {
                            *expr = coalesce;
                            return;
                        }
                    }
                    // No default: simple member expression _rawProps.key
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
            for i in 0..arr.elements.len() {
                match &arr.elements[i] {
                    ArrayExpressionElement::SpreadElement(_) => {
                        if let ArrayExpressionElement::SpreadElement(spread) = &mut arr.elements[i] {
                            rewrite_props_references(
                                &mut spread.argument,
                                prop_map,
                                raw_props_name,
                                prop_defaults,
                                ctx,
                            );
                        }
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    ArrayExpressionElement::Identifier(ident) => {
                        // Handle identifier elements in arrays: check if they match a prop alias
                        // and replace with _rawProps.propName member expression.
                        let name = ident.name.as_str().to_string();
                        if let Some((_, original_key)) = prop_map.iter().find(|(local, _)| *local == name) {
                            if let Some(default_code) = prop_defaults.get(&name) {
                                if let Some(coalesce) =
                                    build_nullish_coalesce(raw_props_name, original_key, default_code, ctx)
                                {
                                    arr.elements[i] = ArrayExpressionElement::from(coalesce);
                                    continue;
                                }
                            }
                            let obj = ctx.ast.expression_identifier(SPAN, ctx.ast.atom(raw_props_name));
                            let prop_name_ident = ctx.ast.identifier_name(SPAN, ctx.ast.atom(original_key.as_str()));
                            let member = ctx.ast.static_member_expression(SPAN, obj, prop_name_ident, false);
                            let member_expr = Expression::StaticMemberExpression(ctx.ast.alloc(member));
                            arr.elements[i] = ArrayExpressionElement::from(member_expr);
                        }
                    }
                    _ => {
                        // Other ArrayExpressionElement variants (CallExpression, etc.)
                        // are not common in capture arrays; skip for now.
                    }
                }
            }
        }

        Expression::ObjectExpression(obj) => {
            for prop in obj.properties.iter_mut() {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        rewrite_props_references(&mut p.value, prop_map, raw_props_name, prop_defaults, ctx);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        rewrite_props_references(
                            &mut spread.argument,
                            prop_map,
                            raw_props_name,
                            prop_defaults,
                            ctx,
                        );
                    }
                }
            }
        }

        Expression::CallExpression(call) => {
            rewrite_props_references(&mut call.callee, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_call_arguments(&mut call.arguments, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::BinaryExpression(bin) => {
            rewrite_props_references(&mut bin.left, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_props_references(&mut bin.right, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::LogicalExpression(log) => {
            rewrite_props_references(&mut log.left, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_props_references(&mut log.right, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::ConditionalExpression(cond) => {
            rewrite_props_references(&mut cond.test, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_props_references(&mut cond.consequent, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_props_references(&mut cond.alternate, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::UnaryExpression(unary) => {
            rewrite_props_references(&mut unary.argument, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::UpdateExpression(_update) => {}

        Expression::AssignmentExpression(assign) => {
            rewrite_props_references(&mut assign.right, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::SequenceExpression(seq) => {
            for e in seq.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }

        Expression::TemplateLiteral(tmpl) => {
            for e in tmpl.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }

        Expression::TaggedTemplateExpression(tagged) => {
            rewrite_props_references(&mut tagged.tag, prop_map, raw_props_name, prop_defaults, ctx);
            for e in tagged.quasi.expressions.iter_mut() {
                rewrite_props_references(e, prop_map, raw_props_name, prop_defaults, ctx);
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
                rewrite_body_statements(&mut arrow.body.statements, prop_map, raw_props_name, prop_defaults, ctx);
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
                        prop_defaults,
                        ctx,
                    );
                }
            }
        }

        Expression::ParenthesizedExpression(paren) => {
            rewrite_props_references(&mut paren.expression, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::AwaitExpression(aw) => {
            rewrite_props_references(&mut aw.argument, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::YieldExpression(y) => {
            if let Some(ref mut arg) = y.argument {
                rewrite_props_references(arg, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }

        Expression::NewExpression(ne) => {
            rewrite_props_references(&mut ne.callee, prop_map, raw_props_name, prop_defaults, ctx);
            for arg in ne.arguments.iter_mut() {
                match arg {
                    Argument::SpreadElement(spread) => {
                        rewrite_props_references(
                            &mut spread.argument,
                            prop_map,
                            raw_props_name,
                            prop_defaults,
                            ctx,
                        );
                    }
                    _ => {
                        if let Some(e) = argument_as_expression_mut(arg) {
                            rewrite_props_references(e, prop_map, raw_props_name, prop_defaults, ctx);
                        }
                    }
                }
            }
        }

        Expression::StaticMemberExpression(mem) => {
            rewrite_props_references(&mut mem.object, prop_map, raw_props_name, prop_defaults, ctx);
        }

        Expression::ComputedMemberExpression(mem) => {
            rewrite_props_references(&mut mem.object, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_props_references(&mut mem.expression, prop_map, raw_props_name, prop_defaults, ctx);
        }

        _ => {}
    }
}

/// Rewrite identifier references in a slice of statements.
pub(crate) fn rewrite_body_statements<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    prop_map: &[(String, String)],
    raw_props_name: &str,
    prop_defaults: &HashMap<String, String>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for stmt in stmts.iter_mut() {
        rewrite_statement(stmt, prop_map, raw_props_name, prop_defaults, ctx);
    }
}

/// Rewrite identifier references in a single statement.
fn rewrite_statement<'a>(
    stmt: &mut Statement<'a>,
    prop_map: &[(String, String)],
    raw_props_name: &str,
    prop_defaults: &HashMap<String, String>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            rewrite_props_references(&mut expr_stmt.expression, prop_map, raw_props_name, prop_defaults, ctx);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(ref mut arg) = ret.argument {
                rewrite_props_references(arg, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for declarator in decl.declarations.iter_mut() {
                if let Some(ref mut init) = declarator.init {
                    rewrite_props_references(init, prop_map, raw_props_name, prop_defaults, ctx);
                }
            }
        }
        Statement::IfStatement(if_stmt) => {
            rewrite_props_references(&mut if_stmt.test, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_statement(&mut if_stmt.consequent, prop_map, raw_props_name, prop_defaults, ctx);
            if let Some(ref mut alt) = if_stmt.alternate {
                rewrite_statement(alt, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }
        Statement::BlockStatement(block) => {
            rewrite_body_statements(&mut block.body, prop_map, raw_props_name, prop_defaults, ctx);
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(ref mut test) = for_stmt.test {
                rewrite_props_references(test, prop_map, raw_props_name, prop_defaults, ctx);
            }
            if let Some(ref mut update) = for_stmt.update {
                rewrite_props_references(update, prop_map, raw_props_name, prop_defaults, ctx);
            }
            rewrite_statement(&mut for_stmt.body, prop_map, raw_props_name, prop_defaults, ctx);
        }
        Statement::WhileStatement(while_stmt) => {
            rewrite_props_references(&mut while_stmt.test, prop_map, raw_props_name, prop_defaults, ctx);
            rewrite_statement(&mut while_stmt.body, prop_map, raw_props_name, prop_defaults, ctx);
        }
        Statement::SwitchStatement(switch_stmt) => {
            rewrite_props_references(&mut switch_stmt.discriminant, prop_map, raw_props_name, prop_defaults, ctx);
            for case in switch_stmt.cases.iter_mut() {
                if let Some(ref mut test) = case.test {
                    rewrite_props_references(test, prop_map, raw_props_name, prop_defaults, ctx);
                }
                rewrite_body_statements(&mut case.consequent, prop_map, raw_props_name, prop_defaults, ctx);
            }
        }
        Statement::ThrowStatement(throw_stmt) => {
            rewrite_props_references(&mut throw_stmt.argument, prop_map, raw_props_name, prop_defaults, ctx);
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
    prop_defaults: &HashMap<String, String>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for i in 0..arguments.len() {
        match &arguments[i] {
            Argument::SpreadElement(_spread) => {
                let placeholder = Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                let old = std::mem::replace(&mut arguments[i], placeholder);
                if let Argument::SpreadElement(mut spread) = old {
                    rewrite_props_references(&mut spread.argument, prop_map, raw_props_name, prop_defaults, ctx);
                    arguments[i] = Argument::SpreadElement(spread);
                }
            }
            Argument::Identifier(ident) => {
                let name = ident.name.as_str().to_string();
                if let Some((_, original_key)) = prop_map.iter().find(|(local, _)| *local == name) {
                    // Check if this prop has a default value
                    if let Some(default_code) = prop_defaults.get(&name) {
                        if let Some(coalesce) =
                            build_nullish_coalesce(raw_props_name, original_key, default_code, ctx)
                        {
                            arguments[i] = Argument::from(coalesce);
                            continue;
                        }
                    }
                    // No default: simple member expression
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
                rewrite_props_references(&mut expr, prop_map, raw_props_name, prop_defaults, ctx);
                arguments[i] = Argument::from(expr);
            }
        }
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

/// Scan the arrow body for a destructuring statement of the form:
/// `const { "key1": alias1, key2, ...rest } = <props_param_name>;`
///
/// Returns `Some(BodyDestructuringInfo)` if found, `None` otherwise.
pub(crate) fn detect_body_destructuring(
    statements: &[Statement<'_>],
    props_param_name: &str,
) -> Option<BodyDestructuringInfo> {
    for (i, stmt) in statements.iter().enumerate() {
        if let Statement::VariableDeclaration(decl) = stmt {
            if decl.declarations.len() != 1 {
                continue;
            }
            let declarator = &decl.declarations[0];
            // Check that init is an identifier matching props_param_name
            if let Some(Expression::Identifier(init_ident)) = &declarator.init {
                if init_ident.name.as_str() != props_param_name {
                    continue;
                }
            } else {
                continue;
            }
            // Check that pattern is an ObjectPattern
            if let BindingPattern::ObjectPattern(obj_pat) = &declarator.id {
                let mut prop_keys = Vec::new();
                let mut rest_name = None;

                for prop in &obj_pat.properties {
                    let key_name = extract_property_key_name(&prop.key);
                    match &prop.value {
                        BindingPattern::BindingIdentifier(ident) => {
                            if let Some(key) = key_name {
                                let local = ident.name.to_string();
                                prop_keys.push((key, local));
                            }
                        }
                        BindingPattern::AssignmentPattern(assign) => {
                            if let Some(key) = key_name {
                                if let BindingPattern::BindingIdentifier(left_ident) = &assign.left {
                                    let local = left_ident.name.to_string();
                                    prop_keys.push((key, local));
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if let Some(rest) = &obj_pat.rest {
                    if let BindingPattern::BindingIdentifier(ident) = &rest.argument {
                        rest_name = Some(ident.name.to_string());
                    }
                }

                return Some(BodyDestructuringInfo {
                    prop_keys,
                    rest_name,
                    stmt_index: i,
                });
            }
        }
    }
    None
}
