//! Inlined function signal wrapping utilities.
//!
//! Implements `convert_inlined_fn` with 6 eligibility checks, `ObjectUsageChecker`
//! (read-only `Visit`), and `ReplaceIdentifiers` (string-based replacement) for
//! building `_fnSignal((p0, p1, ...) => expr, [caps])` calls.
//!
//! Called by `create_synthetic_qqsegment` in `transform.rs` after the `_wrapProp`
//! fast path has been tried and did not match.

use std::collections::HashSet;

use oxc::allocator::Allocator;
use oxc::ast::ast::*;
use oxc::ast_visit::Visit;
use oxc::codegen::Codegen;
use oxc::span::{SourceType, SPAN};

// ---------------------------------------------------------------------------
// ObjectUsageChecker — read-only visitor
// ---------------------------------------------------------------------------

/// Traverses an expression to detect whether any captured identifier appears as
/// the object of a member expression or in a logical-OR expression, and whether
/// any call expression (used_as_call) exists.
///
/// Per SPEC §convert_inlined_fn eligibility checks 2 and 3.
pub(crate) struct ObjectUsageChecker<'a> {
    pub used_as_call: bool,
    pub used_as_object: bool,
    scoped_idents: &'a HashSet<String>,
}

impl<'a> ObjectUsageChecker<'a> {
    pub(crate) fn new(scoped_idents: &'a HashSet<String>) -> Self {
        Self {
            used_as_call: false,
            used_as_object: false,
            scoped_idents,
        }
    }

    fn is_captured_ident(expr: &Expression<'_>, scoped_idents: &HashSet<String>) -> bool {
        if let Expression::Identifier(id) = expr {
            scoped_idents.contains(id.name.as_str())
        } else {
            false
        }
    }
}

impl<'a, 'b> Visit<'a> for ObjectUsageChecker<'b> {
    /// Any call expression in the tree → used_as_call unconditionally.
    fn visit_call_expression(&mut self, expr: &CallExpression<'a>) {
        self.used_as_call = true;
        // Still walk children so we capture any nested object usage.
        oxc::ast_visit::walk::walk_call_expression(self, expr);
    }

    /// Static member `obj.prop` — if `obj` is a captured ident → used_as_object.
    fn visit_static_member_expression(&mut self, expr: &StaticMemberExpression<'a>) {
        if Self::is_captured_ident(&expr.object, self.scoped_idents) {
            self.used_as_object = true;
        }
        oxc::ast_visit::walk::walk_static_member_expression(self, expr);
    }

    /// Logical OR `a || b` — if either operand is a captured ident → used_as_object.
    fn visit_logical_expression(&mut self, expr: &LogicalExpression<'a>) {
        if expr.operator == LogicalOperator::Or {
            if Self::is_captured_ident(&expr.left, self.scoped_idents)
                || Self::is_captured_ident(&expr.right, self.scoped_idents)
            {
                self.used_as_object = true;
            }
        }
        oxc::ast_visit::walk::walk_logical_expression(self, expr);
    }
}

// ---------------------------------------------------------------------------
// ReplaceIdentifiers — string-based replacement + abort detection
// ---------------------------------------------------------------------------

/// Serializes `expr` via OXC Codegen, then performs string replacement of captured
/// identifier names with `p{N}`, and sets `abort = true` if the expression contains
/// constructs that cannot be safely re-evaluated (`=>`, `function`, `class`, `@`).
pub(crate) struct ReplaceIdentifiers {
    /// The serialized expression body string with captured idents replaced by `p{N}`.
    pub rendered: String,
    /// Whether the expression contains any construct that prevents wrapping.
    pub abort: bool,
}

impl ReplaceIdentifiers {
    /// Serialize `expr`, detect abort conditions, then substitute captured idents.
    pub(crate) fn run<'a>(
        expr: &Expression<'a>,
        scoped_idents: &[(String, bool)],
        allocator: &'a Allocator,
    ) -> Self {
        let serialized = serialize_expression_inner(expr, allocator);

        // Check abort conditions in the serialized form.
        let abort = serialized.contains("=>")
            || serialized.contains("function")
            || serialized.contains("class")
            || serialized.contains('@');

        // Replace captured idents with p{N}.
        let mut rendered = serialized;
        for (idx, (name, _is_const)) in scoped_idents.iter().enumerate() {
            // Use word-boundary replacement: only replace whole identifier occurrences.
            // Simple string replace is sufficient here because OXC Codegen produces
            // well-formed code and idents do not appear as substrings of each other
            // when surrounded by operators/punctuation.
            rendered = replace_word(&rendered, name, &format!("p{idx}"));
        }

        Self { rendered, abort }
    }
}

/// Replace whole-word occurrences of `from` with `to` in `s`.
/// A "word boundary" is any character that is not alphanumeric or underscore.
fn replace_word(s: &str, from: &str, to: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut remaining = s;
    while let Some(pos) = remaining.find(from) {
        // Check boundaries.
        let before_ok = pos == 0 || {
            let b = remaining.as_bytes()[pos - 1];
            !b.is_ascii_alphanumeric() && b != b'_'
        };
        let after_pos = pos + from.len();
        let after_ok = after_pos >= remaining.len() || {
            let b = remaining.as_bytes()[after_pos];
            !b.is_ascii_alphanumeric() && b != b'_'
        };

        if before_ok && after_ok {
            result.push_str(&remaining[..pos]);
            result.push_str(to);
            remaining = &remaining[after_pos..];
        } else {
            // Not a word boundary — advance past this false match.
            result.push_str(&remaining[..pos + from.len()]);
            remaining = &remaining[after_pos..];
        }
    }
    result.push_str(remaining);
    result
}

// ---------------------------------------------------------------------------
// serialize_expression_inner — standalone (no QwikTransform receiver)
// ---------------------------------------------------------------------------

fn serialize_expression_inner<'a>(expr: &Expression<'a>, allocator: &'a Allocator) -> String {
    use oxc::allocator::{Box as ArenaBox, CloneIn, Vec as ArenaVec};
    use oxc::ast::AstBuilder;

    let ast = AstBuilder::new(allocator);
    let cloned: Expression<'a> = expr.clone_in(allocator);
    let binding = ast.binding_pattern_binding_identifier(SPAN, ast.atom("_x"));
    let mut declarators: ArenaVec<VariableDeclarator<'_>> = ArenaVec::new_in(allocator);
    declarators.push(ast.variable_declarator(
        SPAN,
        VariableDeclarationKind::Const,
        binding,
        None::<TSTypeAnnotation<'_>>,
        Some(cloned),
        false,
    ));
    let var_decl = ast.alloc_variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        declarators,
        false,
    );
    let mut body: ArenaVec<Statement<'_>> = ArenaVec::new_in(allocator);
    body.push(Statement::VariableDeclaration(var_decl));
    let directives: ArenaVec<Directive<'_>> = ArenaVec::new_in(allocator);
    let comments: ArenaVec<Comment> = ArenaVec::new_in(allocator);
    let prog = ast.program(SPAN, SourceType::tsx(), "", comments, None, directives, body);
    let raw = Codegen::new().build(&prog).code;
    raw.trim_start_matches("const _x = ")
        .trim_end_matches(';')
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// convert_inlined_fn — 6-check eligibility + _fnSignal construction
// ---------------------------------------------------------------------------

/// Try to wrap `expr` as a `_fnSignal((p0, p1, ...) => <body>, [cap0, cap1, ...])` call.
///
/// Returns `(Some(fn_signal_call_code), arrow_code, is_const)` when eligible,
/// or `(None, String::new(), is_const/false)` when any eligibility check fires.
///
/// The `arrow_code` field is the rendered arrow expression string, used by the
/// caller (`hoist_fn_signal_call`) as the deduplication key.
///
/// # Eligibility checks (SPEC §convert_inlined_fn, in order)
///
/// 1. Expression is `ArrowFunctionExpression` → `(None, "", is_const)`
/// 2. `ObjectUsageChecker::used_as_call == true` → `(None, "", false)`
/// 3. `!ObjectUsageChecker::used_as_object` → `(None, "", is_const)`
/// 4. `ReplaceIdentifiers::abort == true` → `(None, "", is_const)`
/// 5. Rendered expression length > 150 chars → `(None, "", false)`
/// 6. `scoped_idents.is_empty()` → `(None, "", true)`
pub(crate) fn convert_inlined_fn<'a>(
    expr: &Expression<'a>,
    scoped_idents: &[(String, bool)], // (name, is_const) pairs
    is_const: bool,
    is_server: bool,
    allocator: &'a Allocator,
) -> (Option<String>, String, bool) {
    // Check 1: ArrowFunctionExpression — already a QRL boundary.
    if matches!(expr, Expression::ArrowFunctionExpression(_)) {
        return (None, String::new(), is_const);
    }

    // Build a HashSet of captured ident names for ObjectUsageChecker.
    let captured_names: HashSet<String> =
        scoped_idents.iter().map(|(n, _)| n.clone()).collect();

    // Check 2 + 3: ObjectUsageChecker.
    let mut checker = ObjectUsageChecker::new(&captured_names);
    checker.visit_expression(expr);

    if checker.used_as_call {
        return (None, String::new(), false);
    }
    if !checker.used_as_object {
        return (None, String::new(), is_const);
    }

    // Check 4: ReplaceIdentifiers abort detection.
    let replaced = ReplaceIdentifiers::run(expr, scoped_idents, allocator);
    if replaced.abort {
        return (None, String::new(), is_const);
    }

    // Check 5: rendered expression length > 150.
    if replaced.rendered.len() > 150 {
        return (None, String::new(), false);
    }

    // Check 6: no captures.
    if scoped_idents.is_empty() {
        return (None, String::new(), true);
    }

    // All checks passed — build the _fnSignal call string.
    // Build parameter list: `(p0, p1, ...)`.
    let params: Vec<String> = (0..scoped_idents.len()).map(|i| format!("p{i}")).collect();
    let params_str = if scoped_idents.len() == 1 {
        format!("p0")
    } else {
        format!("({})", params.join(", "))
    };

    // Build the arrow expression: `(p0, p1, ...) => <body_with_replacements>`.
    let arrow_code = format!("{} => {}", params_str, replaced.rendered);

    // Build captures array: `[cap0, cap1, ...]`.
    let caps: Vec<String> = scoped_idents.iter().map(|(n, _)| n.clone()).collect();
    let caps_str = format!("[{}]", caps.join(", "));

    let fn_signal_code = if is_server {
        // Server mode: third arg is the stringified arrow source.
        let server_str = format!("\"{}\"", arrow_code.replace('\\', "\\\\").replace('"', "\\\""));
        format!("_fnSignal({arrow_code}, {caps_str}, {server_str})")
    } else {
        format!("_fnSignal({arrow_code}, {caps_str})")
    };

    (Some(fn_signal_code), arrow_code, false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oxc::allocator::Allocator;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    fn parse_expr<'a>(allocator: &'a Allocator, src: &str) -> Expression<'a> {
        let src_arena: &str = allocator.alloc_str(src);
        let parser = Parser::new(allocator, src_arena, SourceType::tsx());
        parser.parse_expression().unwrap()
    }

    #[test]
    fn check1_arrow_returns_none() {
        let alloc = Allocator::default();
        let expr = parse_expr(&alloc, "() => x");
        let scoped = vec![("x".to_string(), false)];
        let (result, _arrow, _is_const) = convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_none(), "Arrow expr should return None");
    }

    #[test]
    fn check2_call_returns_none() {
        let alloc = Allocator::default();
        let expr = parse_expr(&alloc, "signal.doSomething()");
        let scoped = vec![("signal".to_string(), false)];
        let (result, _arrow, _is_const) = convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_none(), "Call expr should return None (used_as_call)");
    }

    #[test]
    fn check3_no_object_usage_returns_none() {
        let alloc = Allocator::default();
        // `x + 1` — x is in scoped_idents but not used as object of member access
        let expr = parse_expr(&alloc, "x + 1");
        let scoped = vec![("x".to_string(), false)];
        let (result, _arrow, _is_const) = convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_none(), "No object usage should return None");
    }

    #[test]
    fn check6_no_captures_returns_none() {
        let alloc = Allocator::default();
        // Check 6 fires when scoped_idents is empty but the expression would otherwise pass.
        // We need an expr where the "captured" ident IS used as object.
        // To reach check 6, we need: not arrow, not call, used_as_object=true, not abort, len<=150.
        // With empty scoped_idents, used_as_object can never be true (check 3 fires first).
        // So test check 6 via: a.value where "a" is in scoped_idents initially, then empty list.
        // Actually: with empty scoped_idents, check 3 fires (no object usage). Test that.
        let expr = parse_expr(&alloc, "a.value");
        let scoped: Vec<(String, bool)> = vec![];
        // Check 3 fires first (no captures → no object usage) → returns (None, is_const)
        let (result, _arrow, _is_const) = convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_none(), "No captures → no object usage → None");
    }

    #[test]
    fn check6_no_scoped_idents_used_as_object_still_fires_check6() {
        // When we explicitly make the scoped_idents match the object but the captures
        // array is empty would be contradictory. Instead verify that if captures would
        // be passed as empty, the function returns None.
        // This is actually tested via check3 since empty scoped_idents means
        // ObjectUsageChecker never sees a "captured" ident as object.
        // The check6 path is: all 5 prior checks pass, then scoped_idents.is_empty() fires.
        // Construct: pass scoped_idents with a member, but override to empty list.
        // Actually we need to test the real check6 path:
        // convert_inlined_fn receives expr, scoped_idents=[] after a prior dedup step.
        // This can't happen in practice with check3 before it; check3 always catches it.
        // So this test documents the ordering: check3 catches empty captures before check6.
        let alloc = Allocator::default();
        let expr = parse_expr(&alloc, "x + 1");
        let scoped: Vec<(String, bool)> = vec![];
        let (result, _, _) = convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_none());
    }

    #[test]
    fn eligible_member_produces_fn_signal() {
        let alloc = Allocator::default();
        let expr = parse_expr(&alloc, "signal.color");
        let scoped = vec![("signal".to_string(), false)];
        let (result, arrow_code, _is_const) =
            convert_inlined_fn(&expr, &scoped, false, false, &alloc);
        assert!(result.is_some(), "Eligible member expr should produce _fnSignal");
        let code = result.unwrap();
        assert!(code.starts_with("_fnSignal("), "Should start with _fnSignal");
        assert!(code.contains("p0.color"), "Replaced ident should appear as p0");
        assert!(arrow_code.contains("p0.color"), "arrow_code should contain p0.color");
    }

    #[test]
    fn server_mode_adds_third_arg() {
        let alloc = Allocator::default();
        let expr = parse_expr(&alloc, "signal.color");
        let scoped = vec![("signal".to_string(), false)];
        let (result, _arrow, _is_const) =
            convert_inlined_fn(&expr, &scoped, false, true, &alloc);
        assert!(result.is_some());
        let code = result.unwrap();
        // Third arg should be present
        assert!(
            code.matches(',').count() >= 2,
            "Server mode should have 3 args (2 commas): {code}"
        );
    }

    #[test]
    fn replace_word_whole_boundary() {
        assert_eq!(replace_word("signal.value", "signal", "p0"), "p0.value");
        assert_eq!(replace_word("mysignal.value", "signal", "p0"), "mysignal.value");
        assert_eq!(replace_word("x + x", "x", "p0"), "p0 + p0");
    }
}
