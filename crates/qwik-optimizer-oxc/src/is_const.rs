//! Const evaluation utilities.
//!
//! Determine whether an expression is a compile-time constant for JSX prop
//! classification. Used by the JSX transform to decide whether a prop value
//! goes into const props or var props.

use std::collections::HashSet;

use crate::collector::GlobalCollect;

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

/// Determine whether a JSX prop value expression is a compile-time constant,
/// with additional identifier resolution against global imports, exports, and
/// the caller-provided `const_idents` set.
///
/// Extends [`is_const_expression`] with identifier resolution:
/// - `Identifier` is const if the name is in `const_idents` OR is a globally
///   imported/exported binding in `global_collect`.
/// - `ArrowFunctionExpression` is always const (does not descend into body).
/// - `CallExpression` is never const.
/// - `StaticMemberExpression`, `ComputedMemberExpression`, `PrivateFieldExpression`
///   are never const.
/// - All other cases delegate to the same recursive logic as `is_const_expression`.
///
/// # Safety
/// `global_collect` must be a valid pointer for the duration of this call.
pub(crate) fn is_const_expr_with_context(
    expr: &oxc::ast::ast::Expression<'_>,
    const_idents: &HashSet<String>,
    global_collect: *const GlobalCollect,
) -> bool {
    use oxc::ast::ast::*;

    // SAFETY: pointer is valid for the duration of the traversal (same as transform.rs pattern)
    let collect = unsafe { &*global_collect };

    match expr {
        // Identifier: resolve against const_idents and global_collect
        Expression::Identifier(id) => {
            let name = id.name.as_str();
            if const_idents.contains(name) {
                return true;
            }
            if collect.imports.contains_key(name) {
                return true;
            }
            if collect.exports.contains_key(name) {
                return true;
            }
            false
        }

        // Arrow functions are always const — do NOT descend into body
        Expression::ArrowFunctionExpression(_) => true,

        // Call expressions are never const
        Expression::CallExpression(_) => false,

        // Member expressions are never const
        Expression::StaticMemberExpression(_)
        | Expression::ComputedMemberExpression(_)
        | Expression::PrivateFieldExpression(_) => false,

        // Literals are always const
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,

        // Template literals are const only if all expressions are const
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions.is_empty()
                || tpl.expressions.iter().all(|e| is_const_expr_with_context(e, const_idents, global_collect))
        }

        // typeof is always a string
        Expression::UnaryExpression(unary) => {
            matches!(unary.operator, UnaryOperator::Typeof)
                || is_const_expr_with_context(&unary.argument, const_idents, global_collect)
        }

        // Ternary: const if all three parts are const
        Expression::ConditionalExpression(cond) => {
            is_const_expr_with_context(&cond.test, const_idents, global_collect)
                && is_const_expr_with_context(&cond.consequent, const_idents, global_collect)
                && is_const_expr_with_context(&cond.alternate, const_idents, global_collect)
        }

        // Binary expressions: const if both sides are const
        Expression::BinaryExpression(bin) => {
            is_const_expr_with_context(&bin.left, const_idents, global_collect)
                && is_const_expr_with_context(&bin.right, const_idents, global_collect)
        }

        // Object expressions: const if all property values are const
        Expression::ObjectExpression(obj) => obj.properties.iter().all(|prop| match prop {
            ObjectPropertyKind::ObjectProperty(p) => {
                is_const_expr_with_context(&p.value, const_idents, global_collect)
            }
            ObjectPropertyKind::SpreadProperty(_) => false,
        }),

        // Array expressions: const if all elements are const
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

        // Parenthesized expressions: const if inner is const
        Expression::ParenthesizedExpression(paren) => {
            is_const_expr_with_context(&paren.expression, const_idents, global_collect)
        }

        // Everything else is NOT const
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

    // ---------------------------------------------------------------------------
    // is_const_expr_with_context tests
    // ---------------------------------------------------------------------------

    fn make_collect_with_import(local: &str, specifier: &str, source: &str) -> crate::collector::GlobalCollect {
        use crate::collector::{Import, ImportKind};
        let alloc = oxc::allocator::Allocator::default();
        let src_type = oxc::span::SourceType::tsx();
        let dummy = format!("import {{ {} }} from \"{}\"; export const {} = {};", local, source, local, local);
        let parser = oxc::parser::Parser::new(&alloc, alloc.alloc_str(&dummy), src_type);
        let ret = parser.parse();
        crate::collector::global_collect(&ret.program)
    }

    fn make_collect_with_export(name: &str) -> crate::collector::GlobalCollect {
        let alloc = oxc::allocator::Allocator::default();
        let src_type = oxc::span::SourceType::tsx();
        let dummy = format!("export const {} = 1;", name);
        let parser = oxc::parser::Parser::new(&alloc, alloc.alloc_str(&dummy), src_type);
        let ret = parser.parse();
        crate::collector::global_collect(&ret.program)
    }

    fn make_empty_collect() -> crate::collector::GlobalCollect {
        let alloc = oxc::allocator::Allocator::default();
        let src_type = oxc::span::SourceType::tsx();
        let dummy = "const x = 1;";
        let parser = oxc::parser::Parser::new(&alloc, alloc.alloc_str(dummy), src_type);
        let ret = parser.parse();
        crate::collector::global_collect(&ret.program)
    }

    fn parse_expr<'a>(allocator: &'a oxc::allocator::Allocator, src: &'a str) -> oxc::ast::ast::Expression<'a> {
        let source_type = oxc::span::SourceType::tsx();
        let parser = oxc::parser::Parser::new(allocator, src, source_type);
        parser.parse_expression().unwrap()
    }

    #[test]
    fn test_context_imported_name_is_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_collect_with_import("importedName", "importedName", "@mod");
        let expr = parse_expr(&allocator, "importedName");
        assert!(is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }

    #[test]
    fn test_context_exported_name_is_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_collect_with_export("exportedName");
        let expr = parse_expr(&allocator, "exportedName");
        assert!(is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }

    #[test]
    fn test_context_const_ident_is_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_empty_collect();
        let mut const_idents = std::collections::HashSet::new();
        const_idents.insert("constVar".to_string());
        let expr = parse_expr(&allocator, "constVar");
        assert!(is_const_expr_with_context(&expr, &const_idents, &collect as *const _));
    }

    #[test]
    fn test_context_dynamic_var_is_not_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_empty_collect();
        let expr = parse_expr(&allocator, "dynamicVar");
        assert!(!is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }

    #[test]
    fn test_context_arrow_fn_is_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_empty_collect();
        let expr = parse_expr(&allocator, "() => 1");
        assert!(is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }

    #[test]
    fn test_context_call_expr_is_not_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_empty_collect();
        let expr = parse_expr(&allocator, "foo()");
        assert!(!is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }

    #[test]
    fn test_context_member_expr_is_not_const() {
        let allocator = oxc::allocator::Allocator::default();
        let collect = make_empty_collect();
        let expr = parse_expr(&allocator, "a.b");
        assert!(!is_const_expr_with_context(&expr, &std::collections::HashSet::new(), &collect as *const _));
    }
}
