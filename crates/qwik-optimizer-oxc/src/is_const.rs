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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_const_expression_literals() {
        let allocator = oxc::allocator::Allocator::default();
        let source = "42";
        let source_type = oxc::span::SourceType::mjs();
        let parser = oxc::parser::Parser::new(&allocator, source, source_type);
        let result = parser.parse_expression().unwrap();
        assert!(is_const_expression(&result));
    }
}
