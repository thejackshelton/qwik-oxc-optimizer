//! Build constant replacement and dead branch elimination.
//!
//! Replace references to compile-time constants (`isServer`, `isBrowser`, `isDev`)
//! with boolean literals based on build configuration, then eliminate dead branches.
//! This runs as a pre-pass before the main traverse so that segment body serialization
//! sees the replaced values.
//!
//! The `VisitMut` walker recursively visits all AST nodes, including arrow function
//! bodies inside `inlinedQrl(...)` callback arguments. This means dead branch
//! elimination reaches Inline strategy entry code where `if (isBrowser)` / `if (isServer)`
//! guards appear inside component bodies that stay in the entry module.

use std::collections::HashMap;

use oxc::ast::AstBuilder;
use oxc::ast::ast::*;
use oxc::ast_visit::{VisitMut, walk_mut::*};
use oxc::span::SPAN;

use crate::types::{EmitMode, TransformOptions};

/// Sources that can export build constants.
const BUILD_CONSTANT_SOURCES: &[&str] = &["@qwik.dev/core", "@qwik.dev/core/build"];

/// Known build constant names and what they represent.
const BUILD_CONSTANTS: &[&str] = &["isServer", "isBrowser", "isDev"];

/// Replace build constants and eliminate dead branches in the program AST.
pub(crate) fn replace_build_constants<'a>(
    program: &mut Program<'a>,
    options: &TransformOptions,
    allocator: &'a oxc::allocator::Allocator,
) {
    let replacements = build_replacement_map(program, options);
    if replacements.is_empty() {
        return;
    }

    let ast = AstBuilder::new(allocator);

    // Pass 1: Replace identifiers with boolean literals
    let mut replacer = ConstReplacer {
        replacements: &replacements,
        ast: &ast,
    };
    replacer.visit_program(program);

    // Pass 2: Simplify logical expressions and eliminate dead branches
    let mut eliminator = DeadBranchEliminator { ast: &ast };
    eliminator.visit_program(program);

    strip_build_constant_imports(&mut program.body, &replacements);
}

/// Scan import declarations for build constant imports and build a
/// local_name -> replacement boolean value map.
fn build_replacement_map(
    program: &Program<'_>,
    options: &TransformOptions,
) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(import) = stmt else {
            continue;
        };
        let source = import.source.value.as_str();
        if !BUILD_CONSTANT_SOURCES.iter().any(|s| *s == source) {
            continue;
        }
        let Some(specifiers) = &import.specifiers else {
            continue;
        };
        for spec in specifiers {
            let ImportDeclarationSpecifier::ImportSpecifier(s) = spec else {
                continue;
            };
            let imported_name = match &s.imported {
                ModuleExportName::IdentifierName(id) => id.name.as_str(),
                ModuleExportName::IdentifierReference(id) => id.name.as_str(),
                ModuleExportName::StringLiteral(sl) => sl.value.as_str(),
            };
            if !BUILD_CONSTANTS.contains(&imported_name) {
                continue;
            }
            let local_name = s.local.name.as_str().to_string();
            let value = match imported_name {
                "isServer" => options.is_server,
                "isBrowser" => !options.is_server,
                "isDev" => matches!(options.mode, EmitMode::Dev),
                _ => continue,
            };
            map.insert(local_name, value);
        }
    }
    map
}

/// Replaces build constant identifiers with boolean literals throughout the AST.
///
/// The VisitMut walker automatically recurses into every AST node type
/// (statements, declarations, arguments, array elements, JSX expressions)
/// and calls `visit_expression` for all expression contexts via `to_expression_mut()`.
struct ConstReplacer<'a, 'b> {
    replacements: &'b HashMap<String, bool>,
    ast: &'b AstBuilder<'a>,
}

impl<'a> VisitMut<'a> for ConstReplacer<'a, '_> {
    fn visit_expression(&mut self, expr: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expr {
            if let Some(&value) = self.replacements.get(ident.name.as_str()) {
                *expr = self.ast.expression_boolean_literal(SPAN, value);
                return; // No children to walk after replacement
            }
        }
        walk_expression(self, expr);
    }
}

/// Simplifies logical expressions and eliminates dead if-branches.
///
/// Uses bottom-up (post-order) traversal: children are simplified before
/// parents, so inner expressions are reduced before outer ones are evaluated.
struct DeadBranchEliminator<'a, 'b> {
    ast: &'b AstBuilder<'a>,
}

impl<'a> VisitMut<'a> for DeadBranchEliminator<'a, '_> {
    fn visit_expression(&mut self, expr: &mut Expression<'a>) {
        walk_expression(self, expr); // Walk children FIRST (bottom-up)
        simplify_logical_expression(expr, self.ast);
    }

    fn visit_statements(&mut self, stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>) {
        walk_statements(self, stmts); // Walk children first
        eliminate_dead_if_statements(stmts, self.ast);
    }
}

/// Returns `Some(bool)` if the expression can be reduced to a boolean literal.
fn eval_boolean_value(expr: &Expression<'_>) -> Option<bool> {
    match expr {
        Expression::BooleanLiteral(lit) => Some(lit.value),
        Expression::UnaryExpression(unary)
            if matches!(
                unary.operator,
                oxc::syntax::operator::UnaryOperator::LogicalNot
            ) =>
        {
            eval_boolean_value(&unary.argument).map(|v| !v)
        }
        _ => None,
    }
}

/// Simplify a logical expression or unary-not in-place (non-recursive).
/// The VisitMut walker handles recursion via bottom-up traversal.
fn simplify_logical_expression<'a>(expr: &mut Expression<'a>, ast: &AstBuilder<'a>) {
    if let Expression::LogicalExpression(logical) = expr {
        if let Some(left_val) = eval_boolean_value(&logical.left) {
            match logical.operator {
                oxc::syntax::operator::LogicalOperator::And => {
                    if !left_val {
                        *expr = ast.expression_boolean_literal(SPAN, false);
                    } else {
                        let right = std::mem::replace(
                            &mut logical.right,
                            ast.expression_boolean_literal(SPAN, false),
                        );
                        *expr = right;
                    }
                }
                oxc::syntax::operator::LogicalOperator::Or => {
                    if left_val {
                        *expr = ast.expression_boolean_literal(SPAN, true);
                    } else {
                        let right = std::mem::replace(
                            &mut logical.right,
                            ast.expression_boolean_literal(SPAN, false),
                        );
                        *expr = right;
                    }
                }
                _ => {}
            }
        }
    }

    if let Expression::UnaryExpression(unary) = expr {
        if matches!(
            unary.operator,
            oxc::syntax::operator::UnaryOperator::LogicalNot
        ) {
            if let Some(val) = eval_boolean_value(&unary.argument) {
                *expr = ast.expression_boolean_literal(SPAN, !val);
            }
        }
    }
}

/// Action to take for a statement during dead branch elimination.
enum StmtAction<'a> {
    Remove,
    ReplaceWith(Vec<Statement<'a>>),
}

/// Eliminate dead if-statements in a single statement list (non-recursive).
/// The VisitMut walker handles recursion into nested scopes automatically.
fn eliminate_dead_if_statements<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    ast: &AstBuilder<'a>,
) {
    let mut actions: Vec<(usize, StmtAction<'a>)> = Vec::new();

    for (i, stmt) in stmts.iter_mut().enumerate() {
        let Statement::IfStatement(if_stmt) = stmt else {
            continue;
        };
        let Some(test_val) = eval_boolean_value(&if_stmt.test) else {
            continue;
        };
        if test_val {
            let placeholder = Statement::EmptyStatement(ast.alloc_empty_statement(SPAN));
            let consequent = std::mem::replace(&mut if_stmt.consequent, placeholder);
            if let Statement::BlockStatement(block) = consequent {
                let body_stmts: Vec<Statement<'a>> = block.unbox().body.into_iter().collect();
                actions.push((i, StmtAction::ReplaceWith(body_stmts)));
            } else {
                actions.push((i, StmtAction::ReplaceWith(vec![consequent])));
            }
        } else if if_stmt.alternate.is_some() {
            let alternate = if_stmt.alternate.take().unwrap();
            if let Statement::BlockStatement(block) = alternate {
                let body_stmts: Vec<Statement<'a>> = block.unbox().body.into_iter().collect();
                actions.push((i, StmtAction::ReplaceWith(body_stmts)));
            } else {
                actions.push((i, StmtAction::ReplaceWith(vec![alternate])));
            }
        } else {
            actions.push((i, StmtAction::Remove));
        }
    }

    if !actions.is_empty() {
        let mut action_map: HashMap<usize, StmtAction<'a>> = actions.into_iter().collect();
        let all_stmts: Vec<Statement<'a>> = stmts.drain(..).collect();
        let mut result: Vec<Statement<'a>> = Vec::new();
        for (i, stmt) in all_stmts.into_iter().enumerate() {
            if let Some(action) = action_map.remove(&i) {
                match action {
                    StmtAction::Remove => {}
                    StmtAction::ReplaceWith(replacement) => result.extend(replacement),
                }
            } else {
                result.push(stmt);
            }
        }
        for stmt in result {
            stmts.push(stmt);
        }
    }
}

/// Remove build constant specifiers from import declarations.
/// If all specifiers are removed, remove the entire import statement.
fn strip_build_constant_imports<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    replacements: &HashMap<String, bool>,
) {
    let mut remove_indices: Vec<usize> = Vec::new();
    for (i, stmt) in stmts.iter_mut().enumerate() {
        if let Statement::ImportDeclaration(import) = stmt {
            let source = import.source.value.as_str();
            if !BUILD_CONSTANT_SOURCES.iter().any(|s| *s == source) {
                continue;
            }
            if let Some(ref mut specifiers) = import.specifiers {
                specifiers.retain(|spec| {
                    if let ImportDeclarationSpecifier::ImportSpecifier(s) = spec {
                        !replacements.contains_key(s.local.name.as_str())
                    } else {
                        true
                    }
                });
                if specifiers.is_empty() {
                    remove_indices.push(i);
                }
            }
        }
    }
    if !remove_indices.is_empty() {
        let all_stmts: Vec<Statement<'a>> = stmts.drain(..).collect();
        for (i, stmt) in all_stmts.into_iter().enumerate() {
            if !remove_indices.contains(&i) {
                stmts.push(stmt);
            }
        }
    }
}
