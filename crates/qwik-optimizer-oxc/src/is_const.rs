//! Const evaluation utilities.
//!
//! Determine whether an expression is a compile-time constant for JSX prop
//! classification. Used by the JSX transform to decide whether a prop value
//! goes into const props or var props.

/// Determine whether a JSX prop value expression is a compile-time constant.
///
/// Returns `true` for literals, template literals with no expressions,
/// typeof expressions, and other statically-known values. Returns `false`
/// for identifiers, member expressions, calls, and other dynamic values.
///
/// This is used for the var/const prop split in JSX transformation.
/// Signal wrapping logic (_wrapProp, _fnSignal) handles reactive cases
/// independently and may promote expressions from var to const after wrapping.
#[allow(dead_code)]
pub(crate) fn is_const_expression(expr: &oxc::ast::ast::Expression<'_>) -> bool {
    use oxc::ast::ast::*;
    match expr {
        // Literals are always const
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,

        // Template literals are const only if they have no expressions
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions.is_empty() || tpl.expressions.iter().all(|e| is_const_expression(e))
        }

        // typeof is always a string
        Expression::UnaryExpression(unary) => {
            matches!(unary.operator, UnaryOperator::Typeof) || is_const_expression(&unary.argument)
        }

        // Ternary: const if all three parts are const
        Expression::ConditionalExpression(cond) => {
            is_const_expression(&cond.test)
                && is_const_expression(&cond.consequent)
                && is_const_expression(&cond.alternate)
        }

        // Binary expressions: const if both sides are const
        Expression::BinaryExpression(bin) => {
            is_const_expression(&bin.left) && is_const_expression(&bin.right)
        }

        // Object expressions: const if all property values are const
        Expression::ObjectExpression(obj) => obj.properties.iter().all(|prop| match prop {
            ObjectPropertyKind::ObjectProperty(p) => is_const_expression(&p.value),
            ObjectPropertyKind::SpreadProperty(_) => false,
        }),

        // Array expressions: const if all elements are const
        Expression::ArrayExpression(arr) => arr.elements.iter().all(|elem| match elem {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            // Literal elements are const
            ArrayExpressionElement::BooleanLiteral(_)
            | ArrayExpressionElement::NullLiteral(_)
            | ArrayExpressionElement::NumericLiteral(_)
            | ArrayExpressionElement::BigIntLiteral(_)
            | ArrayExpressionElement::RegExpLiteral(_)
            | ArrayExpressionElement::StringLiteral(_) => true,
            // Everything else in arrays -> not const for simplicity
            _ => false,
        }),

        // Parenthesized expressions: const if inner is const
        Expression::ParenthesizedExpression(paren) => is_const_expression(&paren.expression),

        // Everything else (identifiers, member exprs, calls, arrows, etc.) is NOT const
        _ => false,
    }
}

/// Scope-aware variant of `is_const_expression` that classifies identifiers
/// based on whether they appear in the `const_bindings` set.
///
/// This mirrors SWC's `ConstCollector` which tracks imports and `const` declarations
/// and uses that information during JSX prop classification. Identifiers that are
/// imports or const-declared are treated as const; others (let/var, function params)
/// are treated as non-const.
///
/// Compound expressions (binary, conditional, template literal, etc.) recurse
/// with scope awareness so that `dep.thing + "stuff"` is correctly classified
/// as const when `dep` is an import.
pub(crate) fn is_const_expression_with_scope(
    expr: &oxc::ast::ast::Expression<'_>,
    const_bindings: &std::collections::HashSet<String>,
) -> bool {
    use oxc::ast::ast::*;
    match expr {
        // Literals are always const
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,

        // Identifiers: const if they're in the const_bindings set (imports or const declarations)
        Expression::Identifier(ident) => const_bindings.contains(ident.name.as_str()),

        // Member expressions are NEVER const (matches SWC's ConstCollector::visit_member_expr).
        // Even if the object is a const binding, the member access itself is dynamic.
        Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_) => false,

        // Call expressions are NEVER const (matches SWC's ConstCollector::visit_call_expr).
        Expression::CallExpression(_) => false,

        // Template literals: const if no expressions or all expressions are scope-const
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions.is_empty()
                || tpl
                    .expressions
                    .iter()
                    .all(|e| is_const_expression_with_scope(e, const_bindings))
        }

        // typeof is always a string; other unary ops check inner with scope
        Expression::UnaryExpression(unary) => {
            matches!(unary.operator, UnaryOperator::Typeof)
                || is_const_expression_with_scope(&unary.argument, const_bindings)
        }

        // Ternary: const if all three parts are scope-const
        Expression::ConditionalExpression(cond) => {
            is_const_expression_with_scope(&cond.test, const_bindings)
                && is_const_expression_with_scope(&cond.consequent, const_bindings)
                && is_const_expression_with_scope(&cond.alternate, const_bindings)
        }

        // Binary expressions: const if both sides are scope-const
        Expression::BinaryExpression(bin) => {
            is_const_expression_with_scope(&bin.left, const_bindings)
                && is_const_expression_with_scope(&bin.right, const_bindings)
        }

        // Object expressions: const if all property values are scope-const
        Expression::ObjectExpression(obj) => obj.properties.iter().all(|prop| match prop {
            ObjectPropertyKind::ObjectProperty(p) => {
                is_const_expression_with_scope(&p.value, const_bindings)
            }
            ObjectPropertyKind::SpreadProperty(_) => false,
        }),

        // Array expressions: const if all elements are scope-const
        Expression::ArrayExpression(arr) => arr.elements.iter().all(|elem| match elem {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            ArrayExpressionElement::BooleanLiteral(_)
            | ArrayExpressionElement::NullLiteral(_)
            | ArrayExpressionElement::NumericLiteral(_)
            | ArrayExpressionElement::BigIntLiteral(_)
            | ArrayExpressionElement::RegExpLiteral(_)
            | ArrayExpressionElement::StringLiteral(_) => true,
            _ => false,
        }),

        // Parenthesized expressions: const if inner is scope-const
        Expression::ParenthesizedExpression(paren) => {
            is_const_expression_with_scope(&paren.expression, const_bindings)
        }

        // Everything else (calls, arrows, etc.) is NOT const
        _ => false,
    }
}
