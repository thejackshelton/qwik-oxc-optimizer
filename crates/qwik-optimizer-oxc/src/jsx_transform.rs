//! JSX transformation: convert JSX elements/fragments into _jsxSorted/_jsxSplit function calls.
//!
//! This module contains all the pure JSX transformation logic extracted from transform.rs.
//! Functions here operate on AST nodes and ImportTracker without needing QwikTransform state.

use oxc::ast::ast::*;
use oxc::span::SPAN;
use oxc_traverse::TraverseCtx;

use crate::import_rewrite;
use crate::transform::ImportTracker;

/// Compute a JSX dev location from a byte offset in source code.
fn compute_jsx_dev_location(
    file_name: &str,
    source_code: &str,
    span_start: u32,
) -> import_rewrite::JsxDevLocation {
    let offset = span_start as usize;
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for (i, ch) in source_code.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    import_rewrite::JsxDevLocation {
        file_name: file_name.to_string(),
        line_number: line,
        column_number: col,
    }
}

/// Get the span of a JSXExpression's inner expression, but ONLY for arrow/function
/// expressions that represent inline lambda bodies needing segment extraction.
///
/// Identifier references (e.g., `onClick$={handler}`) are skipped because the
/// referenced binding is already extracted elsewhere. Call expressions (e.g.,
/// `onClick$={sync$(...)}`) are skipped because they're handled by enter_call_expression.
pub(crate) fn get_jsx_lambda_span(expr: &JSXExpression<'_>) -> Option<(u32, u32)> {
    match expr {
        JSXExpression::ArrowFunctionExpression(arrow) => Some((arrow.span.start, arrow.span.end)),
        JSXExpression::FunctionExpression(func) => Some((func.span.start, func.span.end)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// JSX Transformation Helpers
// ---------------------------------------------------------------------------

/// Normalize JSX text: collapse whitespace, strip leading/trailing newlines.
/// Returns empty string for whitespace-only text.
///
/// Matches Babel/SWC JSX text normalization (`cleanJSXElementLiteralChild`):
/// - First line: preserve start, trim end (unless also last line)
/// - Last line: trim start (unless also first line), preserve end
/// - Middle lines: trim both start and end
/// - Single-line text (first AND last): no trimming
/// - Empty lines after trimming are removed
/// - Remaining lines joined with a space
fn normalize_jsx_text(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let mut parts: Vec<String> = Vec::new();
    let is_single_line = lines.len() == 1;

    for (i, line) in lines.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == lines.len() - 1;

        // Replace tabs with spaces (matching Babel/SWC)
        let line_str = line.replace('\t', " ");

        let trimmed = if is_single_line {
            // Single-line: no trimming at all
            line_str
        } else if is_first {
            // First line of multi-line: trim end only
            line_str.trim_end().to_string()
        } else if is_last {
            // Last line of multi-line: trim start only
            line_str.trim_start().to_string()
        } else {
            // Middle lines: trim both
            line_str.trim().to_string()
        };

        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }

    parts.join(" ")
}

/// Transform a JSX event attribute name to its output form.
///
/// Handles patterns:
/// - onClick$ -> q-e:click
/// - onDocumentScroll$ -> q-e:documentscroll
/// - on-cLick$ -> q-e:c-lick
/// - onDocument-sCroll$ -> q-e:document--scroll
/// - document:onFocus$ -> q-d:focus
/// - window:onClick$ -> q-w:click
/// - onKeyup$ -> q-e:keyup
/// - onDocument:keyup$ -> q-e:document:keyup
/// - onWindow:keyup$ -> q-e:window:keyup
/// - host:onClick$ -> host:onClick$ (kept as-is, deprecated)
///
/// Returns None if the attribute is not a transformable event handler.
fn transform_event_attr_name(attr_name: &str) -> Option<String> {
    // Port of SWC's jsx_event_to_html_attribute + get_event_scope_data_from_jsx_event.
    //
    // Determines the prefix and starting index of the event name portion:
    //   - window:onXxx$ -> prefix "q-w:", name starts at 9
    //   - document:onXxx$ -> prefix "q-d:", name starts at 11
    //   - onXxx$ -> prefix "q-e:", name starts at 2
    //   - host:xxx -> kept as-is (deprecated, return None)

    let (prefix, name) = if let Some(rest) = attr_name.strip_prefix("window:") {
        if rest.starts_with("on") && rest.ends_with('$') {
            ("q-w:", &rest[2..rest.len() - 1])
        } else {
            return None;
        }
    } else if let Some(rest) = attr_name.strip_prefix("document:") {
        if rest.starts_with("on") && rest.ends_with('$') {
            ("q-d:", &rest[2..rest.len() - 1])
        } else {
            return None;
        }
    } else if attr_name.starts_with("host:") {
        return None;
    } else if attr_name.starts_with("on") && attr_name.ends_with('$') {
        let event_part = &attr_name[2..attr_name.len() - 1];
        // Handle onDocument:xxx$ and onWindow:xxx$ (colon-based scope)
        if let Some(colon_pos) = event_part.find(':') {
            let scope = &event_part[..colon_pos];
            let event = &event_part[colon_pos + 1..];
            return Some(create_event_name(
                &event.to_lowercase(),
                &format!("q-e:{}:", scope.to_lowercase()),
            ));
        }
        ("q-e:", event_part)
    } else {
        return None;
    };

    // Special case: DOMContentLoaded
    if name == "DOMContentLoaded" {
        return Some(format!("{}-d-o-m-content-loaded", prefix));
    }

    // Leading dash is a case-sensitive event name marker:
    // on-cLick$ -> strip the dash, keep the case, then camelCase-to-kebab.
    let processed_name = if let Some(stripped) = name.strip_prefix('-') {
        stripped.to_string()
    } else {
        name.to_lowercase()
    };

    Some(create_event_name(&processed_name, prefix))
}

/// Convert a processed event name from camelCase to kebab-case.
///
/// Port of SWC's `create_event_name` / `fromCamelToKebabCase`:
/// uppercase letters and dashes are both converted to "-{lower}".
fn create_event_name(name: &str, prefix: &str) -> String {
    let mut result = String::from(prefix);
    for c in name.chars() {
        if c.is_ascii_uppercase() || c == '-' {
            result.push('-');
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Check if an element name is a text-only element.
/// Text-only elements (like <title>, <textarea>) have their children kept as-is
/// without signal wrapping. SWC sets jsx_mutable=true for their children.
/// Matches SWC's is_text_only() function.
fn is_text_only_element(name: &str) -> bool {
    matches!(
        name,
        "text" | "textarea" | "title" | "option" | "script" | "style" | "noscript"
    )
}

/// Check if a JSX attribute value is a compile-time constant for prop classification.
///
/// Uses scope-aware classification: identifiers that are imports or const declarations
/// are treated as const, while let/var bindings and function params are non-const.
fn is_const_jsx_value(
    value: &Expression<'_>,
    const_bindings: &std::collections::HashSet<String>,
) -> bool {
    crate::is_const::is_const_expression_with_scope(value, const_bindings)
}

/// Check if an event handler value should be classified as const for prop placement.
///
/// SWC puts event handler values in const_props when they are stable references
/// (qrl() calls, inlinedQrl() calls, const identifier references to hoisted QRLs).
/// Non-const event handlers (_qrlSync, serverQrl, props.onClick$, ternary with
/// mutable parts) go to var_props, which sets static_listeners=false.
///
/// This is different from `is_const_jsx_value` because qrl() calls (CallExpression)
/// are treated as const here even though they're not pure compile-time constants --
/// they produce stable QRL references that don't change between renders.
fn is_const_event_handler(
    value: &Expression<'_>,
    const_bindings: &std::collections::HashSet<String>,
) -> bool {
    use oxc::ast::ast::*;
    match value {
        // Identifier: const if it's a known const binding (hoisted QRL const)
        // or if it's `undefined` (used as ternary alternate for optional handlers)
        Expression::Identifier(ident) => {
            ident.name == "undefined" || const_bindings.contains(ident.name.as_str())
        }

        // Call expression: only qrl() and inlinedQrl() are const.
        // _qrlSync(), serverQrl(), and other calls are non-const.
        Expression::CallExpression(call) => {
            if let Expression::Identifier(ref callee) = call.callee {
                matches!(callee.name.as_str(), "qrl" | "inlinedQrl")
            } else {
                false
            }
        }

        // Conditional: const only if test is const AND both branches are const event handlers
        Expression::ConditionalExpression(cond) => {
            is_const_jsx_value(&cond.test, const_bindings)
                && is_const_event_handler(&cond.consequent, const_bindings)
                && is_const_event_handler(&cond.alternate, const_bindings)
        }

        // Everything else (member expressions, _qrlSync, serverQrl, etc.) is non-const
        _ => false,
    }
}

/// Check if an expression tree contains a reference to a specific identifier name.
/// Used to determine if a var_prop value references the spread source, which affects
/// whether _getConstProps goes in the 2nd arg (as spread) or 3rd arg (bare call).
fn expr_contains_ident(expr: &Expression<'_>, name: &str) -> bool {
    match expr {
        Expression::Identifier(ident) => ident.name.as_str() == name,
        Expression::CallExpression(call) => {
            expr_contains_ident(&call.callee, name)
                || call
                    .arguments
                    .iter()
                    .any(|arg| match arg {
                        Argument::SpreadElement(spread) => expr_contains_ident(&spread.argument, name),
                        _ => {
                            if let Some(expr) = arg.as_expression() {
                                expr_contains_ident(expr, name)
                            } else {
                                false
                            }
                        }
                    })
        }
        Expression::ArrayExpression(arr) => arr.elements.iter().any(|el| match el {
            ArrayExpressionElement::SpreadElement(spread) => expr_contains_ident(&spread.argument, name),
            _ => {
                if let Some(expr) = el.as_expression() {
                    expr_contains_ident(expr, name)
                } else {
                    false
                }
            }
        }),
        Expression::StaticMemberExpression(member) => expr_contains_ident(&member.object, name),
        Expression::ComputedMemberExpression(member) => {
            expr_contains_ident(&member.object, name) || expr_contains_ident(&member.expression, name)
        }
        Expression::ObjectExpression(obj) => obj.properties.iter().any(|prop| match prop {
            ObjectPropertyKind::ObjectProperty(p) => expr_contains_ident(&p.value, name),
            ObjectPropertyKind::SpreadProperty(s) => expr_contains_ident(&s.argument, name),
        }),
        _ => false,
    }
}

/// Check if a child expression is immutable for JSX flag computation.
///
/// This determines whether a child expression breaks `static_subtree`.
/// SWC uses scope analysis (ConstCollector) to check if identifiers are
/// const-bound (imports, const declarations) vs mutable (let/var, params).
/// We use `const_bindings` for the same purpose:
/// - Identifiers in const_bindings (imports, const declarations) are immutable
/// - Identifiers NOT in const_bindings (let/var, function params) are mutable
/// - Member expressions: immutable only if base object is a const binding
/// - Function calls are mutable UNLESS they're known immutable calls
/// - Tagged template expressions are mutable
/// - Literals and template literals (without expressions) are immutable
fn is_child_expression_immutable(
    expr: &Expression<'_>,
    module_imports: &[crate::types::ImportInfo],
    const_bindings: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        // Literals are always immutable
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,

        // Identifiers: immutable only if they are const-bound (imports or const declarations).
        // This matches SWC's ConstCollector which tracks imports and const bindings.
        // Let/var bindings and function params are mutable.
        Expression::Identifier(ident) => const_bindings.contains(ident.name.as_str()),

        // Member expressions: check if the root identifier is a const binding.
        // SWC's create_synthetic_qqsegment returns is_const based on compute_scoped_idents:
        // if all referenced local variables are Var(true), is_const = true.
        // Imports/globals don't count as scoped idents, so expressions that ONLY
        // reference imports (like `dep.thing`) have empty scoped_idents → is_const = true.
        Expression::StaticMemberExpression(member) => {
            if let Expression::Identifier(obj_ident) = &member.object {
                const_bindings.contains(obj_ident.name.as_str())
            } else {
                false
            }
        }
        Expression::ComputedMemberExpression(member) => {
            if let Expression::Identifier(obj_ident) = &member.object {
                const_bindings.contains(obj_ident.name.as_str())
            } else {
                false
            }
        }

        // Template literals: const if no expressions or all expressions are const
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions.is_empty()
                || tpl
                    .expressions
                    .iter()
                    .all(|e| is_child_expression_immutable(e, module_imports, const_bindings))
        }

        // Unary expressions: typeof is always const, others check inner
        Expression::UnaryExpression(unary) => {
            matches!(unary.operator, UnaryOperator::Typeof)
                || is_child_expression_immutable(&unary.argument, module_imports, const_bindings)
        }

        // Binary expressions: const if both sides are const
        Expression::BinaryExpression(bin) => {
            is_child_expression_immutable(&bin.left, module_imports, const_bindings)
                && is_child_expression_immutable(&bin.right, module_imports, const_bindings)
        }

        // Conditional expressions: const if all parts are const
        Expression::ConditionalExpression(cond) => {
            is_child_expression_immutable(&cond.test, module_imports, const_bindings)
                && is_child_expression_immutable(&cond.consequent, module_imports, const_bindings)
                && is_child_expression_immutable(&cond.alternate, module_imports, const_bindings)
        }

        // Logical expressions: const if both sides are const
        Expression::LogicalExpression(log) => {
            is_child_expression_immutable(&log.left, module_imports, const_bindings)
                && is_child_expression_immutable(&log.right, module_imports, const_bindings)
        }

        // Parenthesized: check inner
        Expression::ParenthesizedExpression(paren) => {
            is_child_expression_immutable(&paren.expression, module_imports, const_bindings)
        }

        // Known immutable function calls (transform-generated)
        Expression::CallExpression(call) => {
            if let Expression::Identifier(ref callee) = call.callee {
                matches!(
                    callee.name.as_str(),
                    "_wrapProp"
                        | "_fnSignal"
                        | "_jsxSorted"
                        | "_jsxSplit"
                        | "_jsxC"
                        | "_IMMUTABLE"
                )
            } else {
                false
            }
        }

        // Tagged template expressions are always mutable
        Expression::TaggedTemplateExpression(_) => false,

        // Arrow/function expressions, object/array literals with spreads, etc.
        // are mutable
        _ => false,
    }
}

/// Check if an expression tree contains already-transformed _jsxSorted/_jsxSplit calls
/// with non-immutable component tags.
///
/// In OXC's bottom-up traversal, inner JSX elements are transformed before their parent.
/// When a non-immutable component like `<Stuff/>` is inside a ternary or logical expression,
/// it becomes `_jsxSorted(Stuff, ...)`. SWC would detect `<Stuff/>` as a non-immutable component
/// during top-down children processing and set jsx_mutable. We simulate this by scanning
/// already-transformed expressions for such calls.
fn contains_mutable_jsx_call(
    expr: &Expression<'_>,
    immutable_function_cmp: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        Expression::CallExpression(call) => {
            if let Expression::Identifier(ref callee) = call.callee {
                if matches!(callee.name.as_str(), "_jsxSorted" | "_jsxSplit" | "_jsxC") {
                    // Check first argument: if it's a non-immutable component identifier, mutable
                    if let Some(first_arg) = call.arguments.first() {
                        if let Some(Expression::Identifier(tag_ident)) =
                            first_arg.as_expression()
                        {
                            let tag_name = tag_ident.name.as_str();
                            // Check if it's a component (starts with uppercase) and not immutable
                            if tag_name.starts_with(|c: char| c.is_uppercase())
                                && !immutable_function_cmp.contains(tag_name)
                            {
                                return true;
                            }
                        }
                    }
                    // Also check children (4th argument) recursively
                    if let Some(children_arg) = call.arguments.get(3) {
                        if let Some(child_expr) = children_arg.as_expression() {
                            if contains_mutable_jsx_call(child_expr, immutable_function_cmp) {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        }
        // Recurse into expression types that may contain transformed JSX
        Expression::ConditionalExpression(cond) => {
            contains_mutable_jsx_call(&cond.test, immutable_function_cmp)
                || contains_mutable_jsx_call(&cond.consequent, immutable_function_cmp)
                || contains_mutable_jsx_call(&cond.alternate, immutable_function_cmp)
        }
        Expression::LogicalExpression(log) => {
            contains_mutable_jsx_call(&log.left, immutable_function_cmp)
                || contains_mutable_jsx_call(&log.right, immutable_function_cmp)
        }
        Expression::ParenthesizedExpression(paren) => {
            contains_mutable_jsx_call(&paren.expression, immutable_function_cmp)
        }
        Expression::ArrayExpression(arr) => arr.elements.iter().any(|elem| {
            elem.as_expression()
                .is_some_and(|e| contains_mutable_jsx_call(e, immutable_function_cmp))
        }),
        _ => false,
    }
}

/// Result of analyzing a JSX prop value for signal wrapping.
enum SignalWrapResult {
    /// Expression should be wrapped with _wrapProp(signal) -- Form 1.
    /// The String is the signal identifier name (object of .value).
    WrapPropSignal,
    /// Expression should be wrapped with _wrapProp(source, "propName") -- Form 2.
    /// The String is the prop name. The bool indicates whether the result is const
    /// (true for const-declared local variables like `const state = useStore(...)`,
    /// false for non-const sources like `_rawProps`, function params, destructured props).
    /// When is_const=false, SWC sets jsx_mutable=true, breaking parent static_subtree.
    WrapPropNamed(String, bool),
    /// No signal wrapping needed; use normal var/const classification.
    None,
}

/// Detect if a JSX prop value expression needs signal wrapping.
///
/// Rules:
/// - `X.value` where X is a simple identifier -> WrapPropSignal
///   BUT NOT `X.value()` (call on .value)
/// - `_rawProps.propName` where _rawProps is the props parameter -> WrapPropNamed
/// - Identifier matching a destructured prop key -> WrapPropNamed (with original key)
/// - `X.Y` where X is a local variable (not import, not global) -> WrapPropNamed
///   This matches SWC's create_synthetic_qqsegment which wraps ANY ident.prop
///   member expression with _wrapProp when the ident is a scoped variable.
///
/// `destructured_props` is an optional map of (local_alias, original_key) pairs
/// from active props destructuring. If an identifier matches a local alias,
/// it will be treated as _rawProps.originalKey.
fn detect_signal_wrap(
    value: &Expression<'_>,
    destructured_props: Option<&[(String, String)]>,
    props_param_name: Option<&str>,
    module_imports: &[crate::types::ImportInfo],
    const_bindings: &std::collections::HashSet<String>,
) -> SignalWrapResult {
    match value {
        Expression::StaticMemberExpression(member) => {
            let prop_name = member.property.name.as_str();

            if prop_name == "value" {
                // Look through TSAsExpression wrappers to find the underlying identifier.
                // e.g. `(count as any).value` -> `count.value` after TS stripping.
                let object = {
                    let mut obj = &member.object;
                    loop {
                        match obj {
                            Expression::TSAsExpression(ts) => obj = &ts.expression,
                            Expression::TSSatisfiesExpression(ts) => obj = &ts.expression,
                            Expression::TSNonNullExpression(ts) => obj = &ts.expression,
                            Expression::TSTypeAssertion(ts) => obj = &ts.expression,
                            Expression::ParenthesizedExpression(paren) => obj = &paren.expression,
                            _ => break,
                        }
                    }
                    obj
                };
                if let Expression::Identifier(ident) = object {
                    let name = ident.name.as_str();
                    // If the object is a body-destructured prop alias (e.g., `test` from
                    // `const { test, ...rest } = props`), don't wrap as WrapPropSignal.
                    // Instead, let it fall through to _fnSignal wrapping which treats
                    // `test.value` as `props.test.value`.
                    let is_prop_alias = destructured_props
                        .map(|props| props.iter().any(|(local, _)| local == name))
                        .unwrap_or(false);
                    if !is_prop_alias {
                        return SignalWrapResult::WrapPropSignal;
                    }
                }
            }

            if let Expression::Identifier(ident) = &member.object {
                let obj_name = ident.name.as_str();

                if obj_name == "_rawProps" && prop_name != "value" {
                    // _rawProps is a function parameter, so is_const=false
                    return SignalWrapResult::WrapPropNamed(prop_name.to_string(), false);
                }
                // Non-destructured props param: props.class -> _wrapProp(props, "class")
                if let Some(param_name) = props_param_name {
                    if obj_name == param_name && prop_name != "value" {
                        // props param is a function parameter, so is_const=false
                        return SignalWrapResult::WrapPropNamed(prop_name.to_string(), false);
                    }
                }

                // Generic local variable member access: state.text -> _wrapProp(state, "text")
                // SWC wraps ANY ident.prop where ident is a scoped variable (in decl_stack).
                // SWC's is_const_expr treats all member expressions as non-const, so they
                // go through create_synthetic_qqsegment which produces _wrapProp for ident.prop.
                // We approximate "scoped variable" by checking: the ident is in const_bindings
                // (known declaration) but NOT an import. Unknown free variables (not declared
                // in scope) are NOT wrapped -- SWC would bail with (None, false).
                // is_const=true because const_bindings only contains const declarations.
                if prop_name != "value" {
                    let is_import = is_imported_identifier(obj_name, module_imports);
                    let is_known_local = const_bindings.contains(obj_name) && !is_import;
                    let is_prop_alias = destructured_props
                        .map(|props| props.iter().any(|(local, _)| local == obj_name))
                        .unwrap_or(false);
                    if is_known_local && !is_prop_alias {
                        return SignalWrapResult::WrapPropNamed(prop_name.to_string(), true);
                    }
                }
            }

            SignalWrapResult::None
        }
        // props["bind:value"] -> _wrapProp(props, "bind:value")
        Expression::ComputedMemberExpression(member) => {
            if let Some(param_name) = props_param_name {
                if let Expression::Identifier(ident) = &member.object {
                    if ident.name.as_str() == param_name {
                        if let Expression::StringLiteral(s) = &member.expression {
                            return SignalWrapResult::WrapPropNamed(s.value.to_string(), false);
                        }
                    }
                }
            }
            // Also handle _rawProps["key"] pattern
            if let Expression::Identifier(ident) = &member.object {
                if ident.name.as_str() == "_rawProps" {
                    if let Expression::StringLiteral(s) = &member.expression {
                        return SignalWrapResult::WrapPropNamed(s.value.to_string(), false);
                    }
                }
            }
            SignalWrapResult::None
        }
        // destructured prop alias, treat as _wrapProp(_rawProps, "fromProps")
        Expression::Identifier(ident) => {
            if let Some(props) = destructured_props {
                let name = ident.name.as_str();
                for (local_alias, original_key) in props {
                    if local_alias == name {
                        // Destructured props are from function params, so is_const=false
                        return SignalWrapResult::WrapPropNamed(original_key.clone(), false);
                    }
                }
            }
            SignalWrapResult::None
        }
        _ => SignalWrapResult::None,
    }
}

/// Check if an expression is a call expression on a .value member.
/// E.g., `signal.value()` -- this should NOT be wrapped.
fn is_call_on_value(value: &Expression<'_>) -> bool {
    if let Expression::CallExpression(call) = value {
        if let Expression::StaticMemberExpression(member) = &call.callee {
            return member.property.name.as_str() == "value";
        }
    }
    false
}

/// A reactive dependency found in an expression.
#[derive(Debug, Clone)]
struct ReactiveDep {
    /// The root identifier name (e.g., "signal", "store", "_rawProps").
    root_name: String,
    /// The parameter name assigned (e.g., "p0", "p1").
    param_name: String,
}

/// Check if an expression contains any function calls (makes it non-wrappable).
fn contains_function_call(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::CallExpression(_) => true,
        // Tagged templates are semantically function calls (the tag is called)
        Expression::TaggedTemplateExpression(_) => true,
        Expression::BinaryExpression(bin) => {
            contains_function_call(&bin.left) || contains_function_call(&bin.right)
        }
        Expression::ConditionalExpression(cond) => {
            contains_function_call(&cond.test)
                || contains_function_call(&cond.consequent)
                || contains_function_call(&cond.alternate)
        }
        Expression::UnaryExpression(unary) => contains_function_call(&unary.argument),
        Expression::ObjectExpression(obj) => obj.properties.iter().any(|prop| match prop {
            ObjectPropertyKind::ObjectProperty(p) => contains_function_call(&p.value),
            ObjectPropertyKind::SpreadProperty(s) => contains_function_call(&s.argument),
        }),
        Expression::ArrayExpression(arr) => arr.elements.iter().any(|elem| match elem {
            ArrayExpressionElement::SpreadElement(s) => contains_function_call(&s.argument),
            ArrayExpressionElement::Elision(_) => false,
            _ => false, // array element literals can't contain calls
        }),
        Expression::ParenthesizedExpression(paren) => contains_function_call(&paren.expression),
        Expression::StaticMemberExpression(mem) => contains_function_call(&mem.object),
        Expression::ComputedMemberExpression(mem) => {
            contains_function_call(&mem.object) || contains_function_call(&mem.expression)
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(_) => true,
            ChainElement::StaticMemberExpression(mem) => contains_function_call(&mem.object),
            ChainElement::ComputedMemberExpression(mem) => {
                contains_function_call(&mem.object) || contains_function_call(&mem.expression)
            }
            _ => false,
        },
        Expression::TemplateLiteral(tpl) => tpl
            .expressions
            .iter()
            .any(|e| contains_function_call(e)),
        Expression::LogicalExpression(log) => {
            contains_function_call(&log.left) || contains_function_call(&log.right)
        }
        _ => false,
    }
}

/// Collect reactive dependency root identifiers from an expression.
///
/// A reactive source is:
/// - An identifier whose `.value` is accessed (signal pattern: `signal.value`)
/// - An identifier that is the root of a multi-level property chain (store pattern: `store.address.city`)
/// - `_rawProps` identifier in a member expression (`_rawProps.propName`)
/// - A destructured prop identifier
///
/// Unknown local identifiers (not imports, globals, or props) are "co-reactive":
/// they become deps only when at least one primary reactive dep exists.
/// SWC treats `fromLocal + fromProps` as `_fnSignal(_hf, [_rawProps, fromLocal], ...)`
/// but bare `fromLocal` stays unwrapped.
///
/// Returns the list of unique reactive deps and whether the expression
/// contains any non-reactive non-const sub-expressions (which would prevent wrapping).
fn collect_reactive_deps(
    expr: &Expression<'_>,
    destructured_props: Option<&[(String, String)]>,
    collected_imports: &[crate::types::ImportInfo],
    props_param_name: Option<&str>,
    const_bindings: &std::collections::HashSet<String>,
) -> (Vec<ReactiveDep>, bool) {
    let mut primary_deps: Vec<ReactiveDep> = Vec::new();
    let mut local_deps: Vec<ReactiveDep> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut has_non_reactive_non_const = false;

    collect_reactive_deps_inner(
        expr,
        destructured_props,
        collected_imports,
        &mut primary_deps,
        &mut local_deps,
        &mut seen,
        &mut has_non_reactive_non_const,
        props_param_name,
        const_bindings,
    );

    // Local deps only materialize when there are primary deps.
    // This matches SWC behavior: bare `fromLocal` stays unwrapped,
    // but `fromLocal + fromProps` produces _fnSignal with both deps.
    if !primary_deps.is_empty() {
        // Merge local deps into primary deps, re-numbering params
        for mut local in local_deps {
            local.param_name = format!("p{}", primary_deps.len());
            primary_deps.push(local);
        }
    }

    // Sort deps alphabetically by root_name to match SWC's
    // compute_scoped_idents -> Vec::sort() ordering.
    primary_deps.sort_by(|a, b| a.root_name.cmp(&b.root_name));
    // Re-assign param names after sorting to maintain p0, p1, p2... order
    for (i, dep) in primary_deps.iter_mut().enumerate() {
        dep.param_name = format!("p{}", i);
    }

    (primary_deps, has_non_reactive_non_const)
}

fn collect_reactive_deps_inner(
    expr: &Expression<'_>,
    destructured_props: Option<&[(String, String)]>,
    collected_imports: &[crate::types::ImportInfo],
    primary_deps: &mut Vec<ReactiveDep>,
    local_deps: &mut Vec<ReactiveDep>,
    seen: &mut std::collections::HashSet<String>,
    has_non_reactive_non_const: &mut bool,
    props_param_name: Option<&str>,
    const_bindings: &std::collections::HashSet<String>,
) {
    match expr {
        // signal.value -> signal is a reactive dep
        Expression::StaticMemberExpression(member) => {
            let prop = member.property.name.as_str();

            let root = get_root_identifier(&member.object);

            if prop == "value" {
                if let Some(root_name) = &root {
                    // Check if this is a body-destructured prop alias.
                    // E.g., `test.value` where `test` came from `{ test, ...rest } = props`
                    // In this case, the dep should be `props` (the props param), not `test`.
                    let prop_alias_origin = destructured_props.and_then(|props| {
                        props.iter().find(|(local, _)| local == root_name).and_then(|_| {
                            props_param_name.map(|p| p.to_string())
                        })
                    });

                    if let Some(origin_name) = prop_alias_origin {
                        // Use the props param as the dep instead of the alias
                        if !seen.contains(&origin_name) {
                            let param = format!("p{}", primary_deps.len());
                            seen.insert(origin_name.clone());
                            primary_deps.push(ReactiveDep {
                                root_name: origin_name,
                                param_name: param,
                            });
                        }
                    } else {
                        if !seen.contains(root_name.as_str()) {
                            let param = format!("p{}", primary_deps.len());
                            seen.insert(root_name.clone());
                            primary_deps.push(ReactiveDep {
                                root_name: root_name.clone(),
                                param_name: param,
                            });
                        }
                    }
                    return; // Don't recurse further
                } else {
                    // .value accessed on a complex expression (e.g., (count || count2).value)
                    // where get_root_identifier returns None. Collect ALL identifiers
                    // from the object as primary deps. This matches SWC where
                    // IdentCollector finds all idents and they get classified as
                    // scoped variables.
                    collect_all_idents_as_primary_deps(
                        &member.object,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        seen,
                        props_param_name,
                    );
                    return;
                }
            }

            if let Some(root_name) = &root {
                // _rawProps or non-destructured props param (e.g., "props") are primary reactive sources
                let is_props_source = root_name == "_rawProps"
                    || props_param_name.is_some_and(|p| p == root_name.as_str());
                if is_props_source {
                    if !seen.contains(root_name.as_str()) {
                        let param = format!("p{}", primary_deps.len());
                        seen.insert(root_name.clone());
                        primary_deps.push(ReactiveDep {
                            root_name: root_name.clone(),
                            param_name: param,
                        });
                    }
                    return;
                }

                if is_imported_identifier(root_name, collected_imports) {
                    // Imports act like "side effects" in SWC: they prevent
                    // _fnSignal wrapping when mixed with reactive deps.
                    *has_non_reactive_non_const = true;
                    return;
                }

                if crate::collector::KNOWN_GLOBALS.contains(root_name.as_str()) {
                    let is_harmless_global =
                        matches!(root_name.as_str(), "undefined" | "NaN" | "Infinity");
                    if !is_harmless_global {
                        *has_non_reactive_non_const = true;
                    }
                    return;
                }

                // Check if root_name is a destructured prop alias (e.g., `data` from `{ data }` destructuring).
                // For `data.selectedOutputDetail`, push the raw props name as the dep instead of `data`.
                // This mirrors the Identifier branch logic for bare prop alias references.
                if let Some(props) = destructured_props {
                    for (local_alias, _original_key) in props {
                        if local_alias == root_name.as_str() {
                            let dep_name = props_param_name.unwrap_or("_rawProps");
                            if !seen.contains(dep_name) {
                                let param = format!("p{}", primary_deps.len());
                                seen.insert(dep_name.to_string());
                                primary_deps.push(ReactiveDep {
                                    root_name: dep_name.to_string(),
                                    param_name: param,
                                });
                            }
                            return;
                        }
                    }
                }

                // Store-like member access: panelStore.active, store.stuff, etc.
                // SWC treats any local ident.prop as a scoped variable (reactive dep).
                // We use const_bindings as a proxy for "locally declared": it contains
                // const declarations (useStore/useSignal results) and imports (already
                // filtered above). For depth >= 2 chains (store.errors.test), we don't
                // need the const_bindings check as they're unambiguously store chains.
                let is_known_local = const_bindings.contains(root_name.as_str());
                if has_chain_depth(expr, 1) && (is_known_local || has_chain_depth(expr, 2)) {
                    if !seen.contains(root_name.as_str()) {
                        let param = format!("p{}", primary_deps.len());
                        seen.insert(root_name.clone());
                        primary_deps.push(ReactiveDep {
                            root_name: root_name.clone(),
                            param_name: param,
                        });
                    }
                    return;
                }
            }

            collect_reactive_deps_inner(
                &member.object,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }

        Expression::Identifier(ident) => {
            let name = ident.name.as_str();

            if let Some(props) = destructured_props {
                for (local_alias, _original_key) in props {
                    if local_alias == name {
                        // Use props_param_name if available (body destructuring with named param),
                        // otherwise default to "_rawProps" (param destructuring).
                        let dep_name = props_param_name.unwrap_or("_rawProps");
                        if !seen.contains(dep_name) {
                            let param = format!("p{}", primary_deps.len());
                            seen.insert(dep_name.to_string());
                            primary_deps.push(ReactiveDep {
                                root_name: dep_name.to_string(),
                                param_name: param,
                            });
                        }
                        return;
                    }
                }
            }

            if is_imported_identifier(name, collected_imports) {
                // Imports act like "side effects" in SWC: they prevent
                // _fnSignal wrapping when mixed with reactive deps.
                *has_non_reactive_non_const = true;
                return;
            }

            if crate::collector::KNOWN_GLOBALS.contains(name) {
                // Harmless constant-like globals should NOT block wrapping.
                // SWC's IdentCollector treats these as non-scope variables and
                // they don't appear in scoped_idents, so they don't trigger
                // contains_side_effect.
                let is_harmless_global = matches!(name, "undefined" | "NaN" | "Infinity");
                if !is_harmless_global {
                    *has_non_reactive_non_const = true;
                }
                return;
            }

            // Unknown local identifier (not a prop, import, or global).
            // These are "co-reactive": they become deps only when the
            // expression also contains primary reactive sources (signal.value,
            // _rawProps, store chains).  A bare `fromLocal` stays unwrapped,
            // but `fromLocal + fromProps` produces
            // `_fnSignal(_hf, [_rawProps, fromLocal], ...)`.
            if !seen.contains(name) {
                // Use a placeholder param -- will be renumbered in collect_reactive_deps()
                let param = format!("p{}", local_deps.len());
                seen.insert(name.to_string());
                local_deps.push(ReactiveDep {
                    root_name: name.to_string(),
                    param_name: param,
                });
            }
        }

        Expression::BinaryExpression(bin) => {
            collect_reactive_deps_inner(
                &bin.left,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            collect_reactive_deps_inner(
                &bin.right,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::ConditionalExpression(cond) => {
            collect_reactive_deps_inner(
                &cond.test,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            collect_reactive_deps_inner(
                &cond.consequent,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            collect_reactive_deps_inner(
                &cond.alternate,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::UnaryExpression(unary) => {
            collect_reactive_deps_inner(
                &unary.argument,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        collect_reactive_deps_inner(
                            &p.value,
                            destructured_props,
                            collected_imports,
                            primary_deps,
                            local_deps,
                            seen,
                            has_non_reactive_non_const,
                            props_param_name,
                            const_bindings,
                        );
                    }
                    ObjectPropertyKind::SpreadProperty(s) => {
                        collect_reactive_deps_inner(
                            &s.argument,
                            destructured_props,
                            collected_imports,
                            primary_deps,
                            local_deps,
                            seen,
                            has_non_reactive_non_const,
                            props_param_name,
                            const_bindings,
                        );
                    }
                }
            }
        }
        Expression::ParenthesizedExpression(paren) => {
            collect_reactive_deps_inner(
                &paren.expression,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }

        Expression::TemplateLiteral(tpl) => {
            for expr in &tpl.expressions {
                collect_reactive_deps_inner(
                    expr,
                    destructured_props,
                    collected_imports,
                    primary_deps,
                    local_deps,
                    seen,
                    has_non_reactive_non_const,
                    props_param_name,
                    const_bindings,
                );
            }
        }
        Expression::TaggedTemplateExpression(tagged) => {
            // The tag is a function call -- this is a side effect that prevents wrapping.
            *has_non_reactive_non_const = true;
            // Still recurse into quasi expressions to collect deps for analysis,
            // but the has_non_reactive flag will prevent wrapping.
            for expr in &tagged.quasi.expressions {
                collect_reactive_deps_inner(
                    expr,
                    destructured_props,
                    collected_imports,
                    primary_deps,
                    local_deps,
                    seen,
                    has_non_reactive_non_const,
                    props_param_name,
                    const_bindings,
                );
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        collect_reactive_deps_inner(
                            &spread.argument,
                            destructured_props,
                            collected_imports,
                            primary_deps,
                            local_deps,
                            seen,
                            has_non_reactive_non_const,
                            props_param_name,
                            const_bindings,
                        );
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(expr) = elem.as_expression() {
                            collect_reactive_deps_inner(
                                expr,
                                destructured_props,
                                collected_imports,
                                primary_deps,
                                local_deps,
                                seen,
                                has_non_reactive_non_const,
                                props_param_name,
                                const_bindings,
                            );
                        }
                    }
                }
            }
        }
        Expression::ComputedMemberExpression(member) => {
            // Recurse into both object and computed key
            collect_reactive_deps_inner(
                &member.object,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            collect_reactive_deps_inner(
                &member.expression,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::CallExpression(call) => {
            // A direct call expression is a side effect that prevents wrapping.
            // SWC's contains_side_effect returns true for CallExpression.
            // Optional chaining calls (ChainExpression) are handled separately
            // and do NOT set this flag, matching SWC behavior where
            // signal.formData?.get("username") IS wrapped but signal.value() is NOT.
            *has_non_reactive_non_const = true;
            // Still recurse to collect deps for analysis purposes.
            collect_reactive_deps_inner(
                &call.callee,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    collect_reactive_deps_inner(
                        expr,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        local_deps,
                        seen,
                        has_non_reactive_non_const,
                        props_param_name,
                        const_bindings,
                    );
                }
            }
        }
        Expression::ChainExpression(chain) => {
            match &chain.expression {
                ChainElement::CallExpression(call) => {
                    collect_reactive_deps_inner(
                        &call.callee,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        local_deps,
                        seen,
                        has_non_reactive_non_const,
                        props_param_name,
                        const_bindings,
                    );
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            collect_reactive_deps_inner(
                                expr,
                                destructured_props,
                                collected_imports,
                                primary_deps,
                                local_deps,
                                seen,
                                has_non_reactive_non_const,
                                props_param_name,
                                const_bindings,
                            );
                        }
                    }
                }
                ChainElement::StaticMemberExpression(member) => {
                    collect_reactive_deps_inner(
                        &member.object,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        local_deps,
                        seen,
                        has_non_reactive_non_const,
                        props_param_name,
                        const_bindings,
                    );
                }
                ChainElement::ComputedMemberExpression(member) => {
                    collect_reactive_deps_inner(
                        &member.object,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        local_deps,
                        seen,
                        has_non_reactive_non_const,
                        props_param_name,
                        const_bindings,
                    );
                    collect_reactive_deps_inner(
                        &member.expression,
                        destructured_props,
                        collected_imports,
                        primary_deps,
                        local_deps,
                        seen,
                        has_non_reactive_non_const,
                        props_param_name,
                        const_bindings,
                    );
                }
                _ => {}
            }
        }
        // TS type wrappers -- look through to the inner expression
        Expression::TSAsExpression(ts) => {
            collect_reactive_deps_inner(
                &ts.expression,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::TSSatisfiesExpression(ts) => {
            collect_reactive_deps_inner(
                &ts.expression,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::TSNonNullExpression(ts) => {
            collect_reactive_deps_inner(
                &ts.expression,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }
        Expression::LogicalExpression(log) => {
            collect_reactive_deps_inner(
                &log.left,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
            collect_reactive_deps_inner(
                &log.right,
                destructured_props,
                collected_imports,
                primary_deps,
                local_deps,
                seen,
                has_non_reactive_non_const,
                props_param_name,
                const_bindings,
            );
        }

        _ => {}
    }
}

/// Get the root identifier name from a (possibly chained) member expression.
pub(crate) fn get_root_identifier(expr: &Expression<'_>) -> Option<String> {
    match expr {
        Expression::Identifier(ident) => Some(ident.name.as_str().to_string()),
        Expression::StaticMemberExpression(member) => get_root_identifier(&member.object),
        Expression::ComputedMemberExpression(member) => get_root_identifier(&member.object),
        _ => None,
    }
}

/// Check if a member expression chain has at least `min_depth` levels.
fn has_chain_depth(expr: &Expression<'_>, min_depth: usize) -> bool {
    fn depth(expr: &Expression<'_>) -> usize {
        match expr {
            Expression::StaticMemberExpression(member) => 1 + depth(&member.object),
            Expression::ComputedMemberExpression(member) => 1 + depth(&member.object),
            _ => 0,
        }
    }
    depth(expr) >= min_depth
}

/// Check if an identifier is an imported name.
fn is_imported_identifier(name: &str, imports: &[crate::types::ImportInfo]) -> bool {
    imports
        .iter()
        .any(|imp| imp.specifiers.iter().any(|spec| spec == name))
}

/// Check if any of the given dep names is used as the OBJECT of a member expression
/// (static or computed) within the expression tree.
///
/// SWC's `is_used_as_object_or_call()` checks this to decide whether _fnSignal
/// wrapping is needed. If a dep is only used as a standalone identifier or as a
/// computed member KEY (array index), wrapping is skipped.
///
/// Also checks through || (logical OR) and parenthesized wrappers, because
/// `(a || b).value` means both `a` and `b` are "used as object".
fn is_any_dep_used_as_object(expr: &Expression<'_>, dep_names: &[&str]) -> bool {
    match expr {
        Expression::StaticMemberExpression(member) => {
            // Check if the object is (or contains) one of our dep names
            if is_dep_or_contains_dep(&member.object, dep_names) {
                return true;
            }
            // Also recurse into the object (for nested member chains)
            is_any_dep_used_as_object(&member.object, dep_names)
        }
        Expression::ComputedMemberExpression(member) => {
            // Check if the OBJECT is a dep (not the computed key!)
            if is_dep_or_contains_dep(&member.object, dep_names) {
                return true;
            }
            // Recurse into object
            if is_any_dep_used_as_object(&member.object, dep_names) {
                return true;
            }
            // Recurse into the computed key expression
            is_any_dep_used_as_object(&member.expression, dep_names)
        }
        Expression::BinaryExpression(bin) => {
            is_any_dep_used_as_object(&bin.left, dep_names)
                || is_any_dep_used_as_object(&bin.right, dep_names)
        }
        Expression::ConditionalExpression(cond) => {
            is_any_dep_used_as_object(&cond.test, dep_names)
                || is_any_dep_used_as_object(&cond.consequent, dep_names)
                || is_any_dep_used_as_object(&cond.alternate, dep_names)
        }
        Expression::UnaryExpression(unary) => {
            is_any_dep_used_as_object(&unary.argument, dep_names)
        }
        Expression::ParenthesizedExpression(paren) => {
            is_any_dep_used_as_object(&paren.expression, dep_names)
        }
        Expression::LogicalExpression(logic) => {
            is_any_dep_used_as_object(&logic.left, dep_names)
                || is_any_dep_used_as_object(&logic.right, dep_names)
        }
        Expression::ObjectExpression(obj) => {
            obj.properties.iter().any(|prop| match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    is_any_dep_used_as_object(&p.value, dep_names)
                }
                ObjectPropertyKind::SpreadProperty(s) => {
                    is_any_dep_used_as_object(&s.argument, dep_names)
                }
            })
        }
        Expression::ArrayExpression(arr) => {
            arr.elements.iter().any(|elem| match elem {
                ArrayExpressionElement::SpreadElement(s) => {
                    is_any_dep_used_as_object(&s.argument, dep_names)
                }
                ArrayExpressionElement::Elision(_) => false,
                _ => {
                    if let Some(e) = elem.as_expression() {
                        is_any_dep_used_as_object(e, dep_names)
                    } else {
                        false
                    }
                }
            })
        }
        Expression::TemplateLiteral(tpl) => {
            tpl.expressions
                .iter()
                .any(|e| is_any_dep_used_as_object(e, dep_names))
        }
        Expression::CallExpression(call) => {
            is_any_dep_used_as_object(&call.callee, dep_names)
                || call.arguments.iter().any(|arg| {
                    arg.as_expression()
                        .map(|e| is_any_dep_used_as_object(e, dep_names))
                        .unwrap_or(false)
                })
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => {
                is_any_dep_used_as_object(&call.callee, dep_names)
                    || call.arguments.iter().any(|arg| {
                        arg.as_expression()
                            .map(|e| is_any_dep_used_as_object(e, dep_names))
                            .unwrap_or(false)
                    })
            }
            ChainElement::StaticMemberExpression(member) => {
                if is_dep_or_contains_dep(&member.object, dep_names) {
                    return true;
                }
                is_any_dep_used_as_object(&member.object, dep_names)
            }
            ChainElement::ComputedMemberExpression(member) => {
                if is_dep_or_contains_dep(&member.object, dep_names) {
                    return true;
                }
                is_any_dep_used_as_object(&member.object, dep_names)
                    || is_any_dep_used_as_object(&member.expression, dep_names)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Check if an expression IS a dep identifier (possibly wrapped in || or parens).
/// SWC checks through LogicalExpression (||) and ParenthesizedExpression.
fn is_dep_or_contains_dep(expr: &Expression<'_>, dep_names: &[&str]) -> bool {
    match expr {
        Expression::Identifier(ident) => dep_names.contains(&ident.name.as_str()),
        Expression::ParenthesizedExpression(paren) => {
            is_dep_or_contains_dep(&paren.expression, dep_names)
        }
        Expression::LogicalExpression(logic) => {
            // (a || b) - both sides count as "the object"
            is_dep_or_contains_dep(&logic.left, dep_names)
                || is_dep_or_contains_dep(&logic.right, dep_names)
        }
        _ => false,
    }
}

/// Collect all identifiers from an expression as primary deps.
/// Used when `.value` is accessed on a complex expression like `(count || count2).value`
/// where `get_root_identifier` returns None.
fn collect_all_idents_as_primary_deps(
    expr: &Expression<'_>,
    _destructured_props: Option<&[(String, String)]>,
    collected_imports: &[crate::types::ImportInfo],
    primary_deps: &mut Vec<ReactiveDep>,
    seen: &mut std::collections::HashSet<String>,
    _props_param_name: Option<&str>,
) {
    match expr {
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            if is_imported_identifier(name, collected_imports)
                || crate::collector::KNOWN_GLOBALS.contains(name)
            {
                return;
            }
            if !seen.contains(name) {
                let param = format!("p{}", primary_deps.len());
                seen.insert(name.to_string());
                primary_deps.push(ReactiveDep {
                    root_name: name.to_string(),
                    param_name: param,
                });
            }
        }
        Expression::LogicalExpression(logic) => {
            collect_all_idents_as_primary_deps(
                &logic.left,
                _destructured_props,
                collected_imports,
                primary_deps,
                seen,
                _props_param_name,
            );
            collect_all_idents_as_primary_deps(
                &logic.right,
                _destructured_props,
                collected_imports,
                primary_deps,
                seen,
                _props_param_name,
            );
        }
        Expression::ParenthesizedExpression(paren) => {
            collect_all_idents_as_primary_deps(
                &paren.expression,
                _destructured_props,
                collected_imports,
                primary_deps,
                seen,
                _props_param_name,
            );
        }
        _ => {}
    }
}

/// Build an _fnSignal call and hoisted function declarations.
///
/// Returns (replacement_expression, fn_code, str_code) where fn_code and str_code
/// are string representations of the hoisted const declarations to insert at module level.
fn build_fn_signal_wrapping<'a>(
    expr: Expression<'a>,
    deps: &[ReactiveDep],
    destructured_props: Option<&[(String, String)]>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    props_param_name: Option<&str>,
) -> (Expression<'a>, String, String) {
    // 1. Build the body string FIRST (before allocating _hf index)
    let mut codegen = oxc::codegen::Codegen::new();
    codegen.print_expression(&expr);
    let mut body_str = codegen.into_source_text();

    for dep in deps {
        let is_props_dep = dep.root_name == "_rawProps"
            || props_param_name.is_some_and(|p| p == dep.root_name.as_str());
        if is_props_dep {
            if let Some(props) = destructured_props {
                for (local_alias, original_key) in props {
                    body_str = replace_identifier_in_code(
                        &body_str,
                        local_alias,
                        &format!("{}.{}", dep.param_name, original_key),
                    );
                }
            }
            body_str = replace_identifier_in_code(&body_str, &dep.root_name, &dep.param_name);
        } else {
            body_str = replace_identifier_in_code(&body_str, &dep.root_name, &dep.param_name);
        }
    }

    let params_str = deps
        .iter()
        .map(|d| d.param_name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    let body_for_fn = if body_str.starts_with('{') {
        format!("({})", body_str)
    } else {
        body_str.clone()
    };

    // 2. Build dedup key and check for existing _hf with same body
    let dedup_key = format!("({}) => {}", params_str, body_for_fn);

    let hf_index =
        if let Some(&existing_index) = tracker.hoisted_fn_dedup.get(&dedup_key) {
            existing_index
        } else {
            let new_index = tracker.hoisted_fn_counter;
            tracker.hoisted_fn_counter += 1;
            tracker.hoisted_fn_dedup.insert(dedup_key, new_index);
            new_index
        };

    let hf_name = format!("_hf{}", hf_index);
    let hf_str_name = format!("_hf{}_str", hf_index);

    // 3. Build fn_code and str_code (hoisted const declarations)
    let fn_code = format!("const {} = ({}) => {};", hf_name, params_str, body_for_fn);

    // For the string representation, strip wrapping parens that OXC codegen
    // adds for ambiguous expression starts (e.g., object literals `({...})`).
    // SWC's string form uses the raw expression: `{props:p0.fromProps}`
    let str_body = if body_str.starts_with('(') && body_str.ends_with(')') {
        &body_str[1..body_str.len() - 1]
    } else {
        &body_str
    };
    let minified = minify_expression_string(str_body);
    let str_code = format!(
        "const {} = \"{}\";",
        hf_str_name,
        escape_string_literal(&minified)
    );

    // 4. Build the _fnSignal call expression
    let callee = ctx.ast.expression_identifier(SPAN, "_fnSignal");
    let mut arguments = ctx.ast.vec_with_capacity(3);

    let hf_atom = ctx.ast.atom(&hf_name);
    arguments.push(Argument::from(ctx.ast.expression_identifier(SPAN, hf_atom)));

    let mut dep_elements = ctx.ast.vec_with_capacity(deps.len());
    for dep in deps {
        let dep_atom = ctx.ast.atom(&dep.root_name);
        dep_elements.push(ArrayExpressionElement::from(
            ctx.ast.expression_identifier(SPAN, dep_atom),
        ));
    }
    arguments.push(Argument::from(ctx.ast.expression_array(SPAN, dep_elements)));

    let str_atom = ctx.ast.atom(&hf_str_name);
    arguments.push(Argument::from(
        ctx.ast.expression_identifier(SPAN, str_atom),
    ));

    let fn_signal_call = ctx.ast.expression_call(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
    );

    (fn_signal_call, fn_code, str_code)
}

/// Replace an identifier in code with a replacement string, being aware of word boundaries.
fn replace_identifier_in_code(code: &str, old_name: &str, new_name: &str) -> String {
    let mut result = String::with_capacity(code.len());
    let chars: Vec<char> = code.chars().collect();
    let old_chars: Vec<char> = old_name.chars().collect();
    let old_len = old_chars.len();
    let mut i = 0;

    while i < chars.len() {
        if i + old_len <= chars.len() && &chars[i..i + old_len] == old_chars.as_slice() {
            // Check word boundaries
            let before_ok = i == 0 || !is_ident_char(chars[i - 1]);
            let after_ok = i + old_len >= chars.len() || !is_ident_char(chars[i + old_len]);

            if before_ok && after_ok {
                // Skip replacement if this identifier is at an object property key position.
                // E.g., `{props: props.fromProps}` -> the first `props` is a key, keep it;
                // only replace the second `props` (the value).
                if is_object_key_position(&chars, i, old_len) {
                    result.push_str(old_name);
                    i += old_len;
                    continue;
                }
                result.push_str(new_name);
                i += old_len;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Check if the identifier at position `pos` (length `len`) is in an object key position.
/// Object key position: preceded by `{` or `,` (ignoring whitespace) AND followed by `:` (but not `::`).
fn is_object_key_position(chars: &[char], pos: usize, len: usize) -> bool {
    // Check after: must be followed by `:` (ignoring whitespace), but not `::`
    let mut after = pos + len;
    while after < chars.len()
        && (chars[after] == ' '
            || chars[after] == '\t'
            || chars[after] == '\n'
            || chars[after] == '\r')
    {
        after += 1;
    }
    if after >= chars.len() || chars[after] != ':' {
        return false;
    }
    // Not `::` (scope resolution)
    if after + 1 < chars.len() && chars[after + 1] == ':' {
        return false;
    }

    // Check before: must be preceded by `{`, `,`, or start of string (ignoring whitespace/newlines)
    if pos == 0 {
        return false; // Can't be object key at very start of code (no enclosing {)
    }
    let mut before = pos - 1;
    loop {
        let c = chars[before];
        if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            if before == 0 {
                return false;
            }
            before -= 1;
        } else {
            break;
        }
    }
    matches!(chars[before], '{' | ',')
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Simple minification: remove unnecessary whitespace.
pub(crate) fn minify_expression_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut string_char = '"';
    let mut prev_was_space = false;

    for c in s.chars() {
        if in_string {
            result.push(c);
            if c == string_char {
                in_string = false;
            }
            continue;
        }

        if c == '"' || c == '\'' || c == '`' {
            in_string = true;
            string_char = c;
            prev_was_space = false;
            result.push(c);
            continue;
        }

        if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            if !prev_was_space && !result.is_empty() {
                let last = result.chars().last().unwrap_or(' ');
                if is_ident_char(last) {
                    prev_was_space = true;
                    // Defer the space -- only add if next char is also ident-like
                }
            }
            continue;
        }

        if prev_was_space && is_ident_char(c) {
            result.push(' ');
        }
        prev_was_space = false;
        result.push(c);
    }

    result
}

/// Escape a string for use inside a double-quoted JS string literal.
pub(crate) fn escape_string_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Extract the identifier name from an expression (if it's a simple identifier).
pub(crate) fn extract_identifier_name(expr: &Expression<'_>) -> String {
    match expr {
        Expression::Identifier(ident) => ident.name.as_str().to_string(),
        _ => {
            let mut codegen = oxc::codegen::Codegen::new();
            codegen.print_expression(expr);
            codegen.into_source_text()
        }
    }
}

/// Build an inlinedQrl event handler for bind: directives.
///
/// Produces: `inlinedQrl(_handler, "_handler", [signal])`
/// where _handler is _val or _chk, and signal is the bound signal identifier.
pub(crate) fn build_bind_event_handler<'a>(
    handler_name: &str,
    signal_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let callee = ctx.ast.expression_identifier(SPAN, "inlinedQrl");

    let handler_atom = ctx.ast.atom(handler_name);
    let handler_str_atom = ctx.ast.atom(handler_name);
    let signal_atom = ctx.ast.atom(signal_name);

    let mut arguments = ctx.ast.vec_with_capacity(3);

    arguments.push(Argument::from(
        ctx.ast.expression_identifier(SPAN, handler_atom),
    ));

    arguments.push(Argument::from(ctx.ast.expression_string_literal(
        SPAN,
        handler_str_atom,
        None,
    )));

    let mut elements = ctx.ast.vec_with_capacity(1);
    elements.push(ArrayExpressionElement::from(
        ctx.ast.expression_identifier(SPAN, signal_atom),
    ));
    arguments.push(Argument::from(ctx.ast.expression_array(SPAN, elements)));

    ctx.ast.expression_call(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
    )
}

/// Build the tag expression for a JSX element name.
pub(crate) fn build_tag_expression<'a>(
    name: &JSXElementName<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    match name {
        JSXElementName::Identifier(ident) => {
            let tag_name = ident.name.as_str();
            if tag_name.chars().next().map_or(false, |c| c.is_lowercase()) {
                let atom = ctx.ast.atom(tag_name);
                ctx.ast.expression_string_literal(SPAN, atom, None)
            } else {
                let atom = ctx.ast.atom(tag_name);
                ctx.ast.expression_identifier(SPAN, atom)
            }
        }
        JSXElementName::IdentifierReference(ident_ref) => {
            let atom = ctx.ast.atom(ident_ref.name.as_str());
            ctx.ast.expression_identifier(SPAN, atom)
        }
        JSXElementName::MemberExpression(member) => build_jsx_member_expr(member, ctx),
        JSXElementName::NamespacedName(ns) => {
            let name = format!("{}:{}", ns.namespace.name, ns.name.name);
            let atom = ctx.ast.atom(&name);
            ctx.ast.expression_string_literal(SPAN, atom, None)
        }
        JSXElementName::ThisExpression(_) => ctx.ast.expression_this(SPAN),
    }
}

/// Build a member expression from a JSXMemberExpression (e.g., Foo.Bar.Baz).
pub(crate) fn build_jsx_member_expr<'a>(
    member: &JSXMemberExpression<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let object = match &member.object {
        JSXMemberExpressionObject::IdentifierReference(ident) => {
            let atom = ctx.ast.atom(ident.name.as_str());
            ctx.ast.expression_identifier(SPAN, atom)
        }
        JSXMemberExpressionObject::MemberExpression(inner) => build_jsx_member_expr(inner, ctx),
        JSXMemberExpressionObject::ThisExpression(_) => ctx.ast.expression_this(SPAN),
    };
    let property_atom = ctx.ast.atom(member.property.name.as_str());
    let property = ctx.ast.identifier_name(SPAN, property_atom);
    Expression::StaticMemberExpression(
        ctx.ast
            .alloc_static_member_expression(SPAN, object, property, false),
    )
}

/// Convert a JSXExpression to an Expression. JSXExpression uses inherit_variants!
/// from Expression, so all Expression variants appear directly on JSXExpression.
pub(crate) fn jsx_expression_to_expression<'a>(
    jsx_expr: JSXExpression<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    match jsx_expr {
        JSXExpression::EmptyExpression(_) => ctx.ast.expression_identifier(SPAN, "undefined"),
        // Inherited Expression variants - literals
        JSXExpression::BooleanLiteral(e) => Expression::BooleanLiteral(e),
        JSXExpression::NullLiteral(e) => Expression::NullLiteral(e),
        JSXExpression::NumericLiteral(e) => Expression::NumericLiteral(e),
        JSXExpression::BigIntLiteral(e) => Expression::BigIntLiteral(e),
        JSXExpression::RegExpLiteral(e) => Expression::RegExpLiteral(e),
        JSXExpression::StringLiteral(e) => Expression::StringLiteral(e),
        JSXExpression::TemplateLiteral(e) => Expression::TemplateLiteral(e),
        // Identifiers and special
        JSXExpression::Identifier(e) => Expression::Identifier(e),
        JSXExpression::MetaProperty(e) => Expression::MetaProperty(e),
        JSXExpression::Super(e) => Expression::Super(e),
        // Compound expressions
        JSXExpression::ArrayExpression(e) => Expression::ArrayExpression(e),
        JSXExpression::ArrowFunctionExpression(e) => Expression::ArrowFunctionExpression(e),
        JSXExpression::AssignmentExpression(e) => Expression::AssignmentExpression(e),
        JSXExpression::AwaitExpression(e) => Expression::AwaitExpression(e),
        JSXExpression::BinaryExpression(e) => Expression::BinaryExpression(e),
        JSXExpression::CallExpression(e) => Expression::CallExpression(e),
        JSXExpression::ChainExpression(e) => Expression::ChainExpression(e),
        JSXExpression::ClassExpression(e) => Expression::ClassExpression(e),
        JSXExpression::ConditionalExpression(e) => Expression::ConditionalExpression(e),
        JSXExpression::FunctionExpression(e) => Expression::FunctionExpression(e),
        JSXExpression::ImportExpression(e) => Expression::ImportExpression(e),
        JSXExpression::LogicalExpression(e) => Expression::LogicalExpression(e),
        JSXExpression::NewExpression(e) => Expression::NewExpression(e),
        JSXExpression::ObjectExpression(e) => Expression::ObjectExpression(e),
        JSXExpression::ParenthesizedExpression(e) => Expression::ParenthesizedExpression(e),
        JSXExpression::SequenceExpression(e) => Expression::SequenceExpression(e),
        JSXExpression::TaggedTemplateExpression(e) => Expression::TaggedTemplateExpression(e),
        JSXExpression::ThisExpression(e) => Expression::ThisExpression(e),
        JSXExpression::UnaryExpression(e) => Expression::UnaryExpression(e),
        JSXExpression::UpdateExpression(e) => Expression::UpdateExpression(e),
        JSXExpression::YieldExpression(e) => Expression::YieldExpression(e),
        JSXExpression::PrivateInExpression(e) => Expression::PrivateInExpression(e),
        // Member expressions (inherited from MemberExpression)
        JSXExpression::ComputedMemberExpression(e) => Expression::ComputedMemberExpression(e),
        JSXExpression::StaticMemberExpression(e) => Expression::StaticMemberExpression(e),
        JSXExpression::PrivateFieldExpression(e) => Expression::PrivateFieldExpression(e),
        // JSX
        JSXExpression::JSXElement(e) => Expression::JSXElement(e),
        JSXExpression::JSXFragment(e) => Expression::JSXFragment(e),
        // TypeScript expressions
        JSXExpression::TSAsExpression(e) => Expression::TSAsExpression(e),
        JSXExpression::TSSatisfiesExpression(e) => Expression::TSSatisfiesExpression(e),
        JSXExpression::TSTypeAssertion(e) => Expression::TSTypeAssertion(e),
        JSXExpression::TSNonNullExpression(e) => Expression::TSNonNullExpression(e),
        JSXExpression::TSInstantiationExpression(e) => Expression::TSInstantiationExpression(e),
        JSXExpression::V8IntrinsicExpression(e) => Expression::V8IntrinsicExpression(e),
    }
}

/// Convert a JSXAttributeValue to an Expression, taking ownership.
fn jsx_attr_value_to_expression<'a>(
    value: JSXAttributeValue<'a>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    destructured_props: Option<&[(String, String)]>,
    module_imports: &[crate::types::ImportInfo],
    hoisted_stmts: &mut Vec<(String, String)>,
    loop_depth: u32,
    iteration_vars: &[String],
    props_param_name: Option<&str>,
    key_prefix: &str,
) -> Expression<'a> {
    match value {
        JSXAttributeValue::StringLiteral(lit) => Expression::StringLiteral(lit),
        JSXAttributeValue::ExpressionContainer(container) => {
            jsx_expression_to_expression(container.unbox().expression, ctx)
        }
        JSXAttributeValue::Element(el) => {
            // JSX element as attribute value: transform it (never root JSX)
            transform_jsx_element_inner(
                el.unbox(),
                tracker,
                ctx,
                destructured_props,
                module_imports,
                hoisted_stmts,
                loop_depth,
                iteration_vars,
                props_param_name,
                false,
                key_prefix,
            )
        }
        JSXAttributeValue::Fragment(frag) => transform_jsx_fragment_inner(
            frag.unbox(),
            tracker,
            ctx,
            destructured_props,
            module_imports,
            hoisted_stmts,
            loop_depth,
            iteration_vars,
            props_param_name,
            false,
            key_prefix,
        ),
    }
}

/// Recursively transform a JSXElement into a _jsxSorted/_jsxSplit call expression.
/// When `tracker.custom_jsx_source` is Some, uses the simpler React-style `_jsx("tag", {props})`
/// form instead of Qwik's `_jsxSorted`.
pub(crate) fn transform_jsx_element_inner<'a>(
    mut element: JSXElement<'a>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    destructured_props: Option<&[(String, String)]>,
    module_imports: &[crate::types::ImportInfo],
    hoisted_stmts: &mut Vec<(String, String)>,
    loop_depth: u32,
    iteration_vars: &[String],
    props_param_name: Option<&str>,
    root_jsx_mode: bool,
    key_prefix: &str,
) -> Expression<'a> {
    // When a custom JSX import source is set (e.g., React), use the standard
    // JSX runtime transform: _jsx("tag", {props}) instead of Qwik's _jsxSorted.
    if tracker.custom_jsx_source.is_some() {
        return transform_jsx_element_custom_source(
            element, tracker, ctx, hoisted_stmts, module_imports, destructured_props,
            loop_depth, iteration_vars, props_param_name, key_prefix,
        );
    }

    // Capture the element's opening tag span for dev mode location metadata.
    // Must be done before consuming the element.
    let element_span_start = element.opening_element.span.start;

    let tag = build_tag_expression(&element.opening_element.name, ctx);

    // Determine if this is a component (function) tag -- uppercase first char or member expression
    let is_fn = match &element.opening_element.name {
        JSXElementName::Identifier(ident) => {
            ident.name.as_str().starts_with(|c: char| c.is_uppercase())
        }
        JSXElementName::IdentifierReference(ident) => {
            ident.name.as_str().starts_with(|c: char| c.is_uppercase())
        }
        JSXElementName::MemberExpression(_) => true,
        _ => false,
    };

    // Component tags not in immutable_function_cmp set make the parent's subtree mutable.
    // This communicates to the parent element via tracker.jsx_mutable.
    // (SWC transform.rs lines 866-867)
    if is_fn {
        let tag_name = match &element.opening_element.name {
            JSXElementName::Identifier(ident) => Some(ident.name.as_str()),
            JSXElementName::IdentifierReference(ident) => Some(ident.name.as_str()),
            _ => None,
        };
        if let Some(name) = tag_name {
            if !tracker.immutable_function_cmp.contains(name) {
                tracker.jsx_mutable = true;
            }
        } else {
            // MemberExpression or other complex tags are always mutable
            tracker.jsx_mutable = true;
        }
    }

    /// Merge or add an event handler to a props list.
    /// If a handler with the same key already exists, merge into an array.
    /// This matches SWC's `merge_or_add_event_handler` behavior.
    fn merge_or_add_to_props<'b>(
        props: &mut Vec<(String, Expression<'b>)>,
        key: String,
        handler: Expression<'b>,
        ast: &oxc::ast::AstBuilder<'b>,
    ) {
        let existing_idx = props.iter().position(|(k, _)| k == &key);
        if let Some(idx) = existing_idx {
            let (existing_key, existing_handler) = props.remove(idx);
            let _ = existing_key;
            // Create an array with both handlers
            use oxc::ast::ast::*;
            let mut elements = ast.vec();
            // Check if existing handler is already an array
            if let Expression::ArrayExpression(arr) = existing_handler {
                for elem in arr.unbox().elements.into_iter() {
                    elements.push(elem);
                }
            } else {
                elements.push(ArrayExpressionElement::from(existing_handler));
            }
            elements.push(ArrayExpressionElement::from(handler));
            let array = ast.expression_array(SPAN, elements);
            props.push((key, array));
        } else {
            props.push((key, handler));
        }
    }

    // Classify attributes: detect spreads, separate key, classify var/const props
    let mut has_spread = false;
    let mut key_value: Option<Expression<'a>> = None;
    let mut var_props: Vec<(String, Expression<'a>)> = Vec::new();
    let mut const_props: Vec<(String, Expression<'a>)> = Vec::new();
    let mut spread_args: Vec<Expression<'a>> = Vec::new();
    let mut _has_only_events = true;
    let mut _has_any_visible_prop = false;
    // Track where the first spread attribute occurs in var_props,
    // so that _getVarProps(source) is inserted at the correct position
    // (matching SWC's source-order prop interleaving in _jsxSplit).
    let mut spread_insert_idx: Option<usize> = None;
    // Also track const_props length at the time the first spread is seen,
    // so in _jsxSplit mode we can merge explicit const_props into the
    // var_props object at their correct source position.
    let mut const_spread_insert_idx: Option<usize> = None;

    // Take attributes out of the opening element
    let mut attrs = ctx.ast.vec();
    std::mem::swap(&mut element.opening_element.attributes, &mut attrs);

    // Pre-scan: detect if ANY spread attribute exists.
    // This is needed because bind:value/bind:checked should not be transformed
    // when a spread is present (_jsxSplit path), even if the bind appears before the spread.
    let any_spread = attrs.iter().any(|a| matches!(a, JSXAttributeItem::SpreadAttribute(_)));

    for attr_item in attrs {
        match attr_item {
            JSXAttributeItem::SpreadAttribute(spread) => {
                if !has_spread {
                    // Record the current var_props and const_props lengths so we know where to insert
                    // ..._getVarProps(source) in the _jsxSplit output.
                    spread_insert_idx = Some(var_props.len());
                    const_spread_insert_idx = Some(const_props.len());
                }
                has_spread = true;
                spread_args.push(spread.unbox().argument);
            }
            JSXAttributeItem::Attribute(attr) => {
                let attr = attr.unbox();
                let mut attr_name = match &attr.name {
                    JSXAttributeName::Identifier(ident) => ident.name.as_str().to_string(),
                    JSXAttributeName::NamespacedName(ns) => {
                        format!("{}:{}", ns.namespace.name, ns.name.name)
                    }
                };

                // className -> class for native HTML elements (lowercase tag name).
                // Component elements (uppercase first letter) keep className as-is.
                // Matches SWC behavior in the className transform.
                if attr_name == "className" && !is_fn {
                    attr_name = "class".to_string();
                }

                // Handle key attribute
                if attr_name == "key" {
                    if let Some(val) = attr.value {
                        key_value = Some(jsx_attr_value_to_expression(
                            val,
                            tracker,
                            ctx,
                            destructured_props,
                            module_imports,
                            hoisted_stmts,
                            loop_depth,
                            iteration_vars,
                            props_param_name,
                            key_prefix,
                        ));
                    }
                    continue;
                }

                // Check for event handler attributes
                if let Some(event_name) = transform_event_attr_name(&attr_name) {
                    // Event handler: classify based on value constness.
                    // qrl() and inlinedQrl() calls go to const_props (stable QRL references).
                    // _qrlSync(), serverQrl(), props.onClick$, and other non-const values
                    // go to var_props (SWC sets static_listeners=false for these).
                    let value = if let Some(val) = attr.value {
                        jsx_attr_value_to_expression(
                            val,
                            tracker,
                            ctx,
                            destructured_props,
                            module_imports,
                            hoisted_stmts,
                            loop_depth,
                            iteration_vars,
                            props_param_name,
                            key_prefix,
                        )
                    } else {
                        ctx.ast.expression_boolean_literal(SPAN, true)
                    };
                    if is_const_event_handler(&value, &tracker.const_bindings) {
                        // Use merge for q-e:input since bind:value/checked also generates q-e:input
                        if event_name == "q-e:input" {
                            merge_or_add_to_props(&mut const_props, event_name, value, &ctx.ast);
                        } else {
                            const_props.push((event_name, value));
                        }
                    } else {
                        if event_name == "q-e:input" {
                            merge_or_add_to_props(&mut var_props, event_name, value, &ctx.ast);
                        } else {
                            var_props.push((event_name, value));
                        }
                    }
                    continue;
                }

                // Check for host: prefix (kept as-is) or custom$ (kept as-is)
                if attr_name.starts_with("host:")
                    || (attr_name.ends_with('$') && !attr_name.starts_with("on"))
                {
                    let value = if let Some(val) = attr.value {
                        jsx_attr_value_to_expression(
                            val,
                            tracker,
                            ctx,
                            destructured_props,
                            module_imports,
                            hoisted_stmts,
                            loop_depth,
                            iteration_vars,
                            props_param_name,
                            key_prefix,
                        )
                    } else {
                        ctx.ast.expression_boolean_literal(SPAN, true)
                    };
                    const_props.push((attr_name, value));
                    continue;
                }

                // Check for bind: directive (CONV-12)
                if let Some(bind_prop) = attr_name.strip_prefix("bind:") {
                    let signal_value = if let Some(val) = attr.value {
                        jsx_attr_value_to_expression(
                            val,
                            tracker,
                            ctx,
                            destructured_props,
                            module_imports,
                            hoisted_stmts,
                            loop_depth,
                            iteration_vars,
                            props_param_name,
                            key_prefix,
                        )
                    } else {
                        ctx.ast.expression_boolean_literal(SPAN, true)
                    };

                    // When a spread is present (_jsxSplit), don't transform bind:value/bind:checked.
                    // Pass them through as-is in var_props. The runtime handles them.
                    // SWC: "should_not_transform_bind_value_in_var_props_for_jsx_split"
                    if any_spread {
                        var_props.push((attr_name, signal_value));
                        continue;
                    }

                    match bind_prop {
                        "value" => {
                            // bind:value={signal} ->
                            //   "value": signal (const prop)
                            //   "q-e:input": inlinedQrl(_val, "_val", [signal]) (const prop)
                            // If there's already a q-e:input handler, merge into an array.
                            if !tracker.needs_val {
                                tracker.needs_val = true;
                                tracker.record_synthetic_import("_val");
                            }
                            if !tracker.needs_inlined_qrl {
                                tracker.needs_inlined_qrl = true;
                                tracker.record_synthetic_import(if tracker.jsx_dev_file_name.is_some() { "inlinedQrlDEV" } else { "inlinedQrl" });
                            }

                            let signal_name = extract_identifier_name(&signal_value);
                            let event_handler = build_bind_event_handler("_val", &signal_name, ctx);
                            const_props.push(("value".to_string(), signal_value));
                            merge_or_add_to_props(&mut const_props, "q-e:input".to_string(), event_handler, &ctx.ast);
                            continue;
                        }
                        "checked" => {
                            // bind:checked={signal} ->
                            //   "checked": signal (const prop)
                            //   "q-e:input": inlinedQrl(_chk, "_chk", [signal]) (const prop)
                            // If there's already a q-e:input handler, merge into an array.
                            if !tracker.needs_chk {
                                tracker.needs_chk = true;
                                tracker.record_synthetic_import("_chk");
                            }
                            if !tracker.needs_inlined_qrl {
                                tracker.needs_inlined_qrl = true;
                                tracker.record_synthetic_import(if tracker.jsx_dev_file_name.is_some() { "inlinedQrlDEV" } else { "inlinedQrl" });
                            }

                            let signal_name = extract_identifier_name(&signal_value);
                            let event_handler = build_bind_event_handler("_chk", &signal_name, ctx);
                            const_props.push(("checked".to_string(), signal_value));
                            merge_or_add_to_props(&mut const_props, "q-e:input".to_string(), event_handler, &ctx.ast);
                            continue;
                        }
                        _ => {
                            // bind:other -> pass through as-is in const props
                            const_props.push((attr_name, signal_value));
                            continue;
                        }
                    }
                }

                // q:p and q:ps attributes ALWAYS go to var_props unconditionally.
                // These are iteration variable bindings injected for loop event handlers.
                // Even though the value may be a const-declared loop variable
                // (e.g., `for (const item of ...)` where `item` is in const_bindings),
                // the q:p value changes per iteration and must be in var_props.
                // SWC always puts q:p/q:ps in var_props.
                if attr_name == "q:p" || attr_name == "q:ps" {
                    let value = if let Some(val) = attr.value {
                        jsx_attr_value_to_expression(
                            val,
                            tracker,
                            ctx,
                            destructured_props,
                            module_imports,
                            hoisted_stmts,
                            loop_depth,
                            iteration_vars,
                            props_param_name,
                            key_prefix,
                        )
                    } else {
                        ctx.ast.expression_boolean_literal(SPAN, true)
                    };
                    var_props.push((attr_name, value));
                    continue;
                }

                // Regular attribute
                _has_any_visible_prop = true;
                _has_only_events = false;
                let value = if let Some(val) = attr.value {
                    jsx_attr_value_to_expression(
                        val,
                        tracker,
                        ctx,
                        destructured_props,
                        module_imports,
                        hoisted_stmts,
                        loop_depth,
                        iteration_vars,
                        props_param_name,
                        key_prefix,
                    )
                } else {
                    // Boolean attribute: <input disabled /> -> disabled: true
                    ctx.ast.expression_boolean_literal(SPAN, true)
                };

                // Check for signal wrapping BEFORE const/var classification.
                // signal.value -> _wrapProp(signal) in const props
                // _rawProps.propName -> _wrapProp(_rawProps, "propName") in const props
                // But NOT signal.value() (function call on .value)
                if !is_call_on_value(&value) {
                    match detect_signal_wrap(&value, destructured_props, props_param_name, module_imports, &tracker.const_bindings) {
                        SignalWrapResult::WrapPropSignal => {
                            // Extract the signal identifier from X.value
                            // SWC: is_const depends on compute_scoped_idents.
                            // For const vars (useSignal()) → const_props.
                            // For non-const (function params) → var_props.
                            if let Expression::StaticMemberExpression(member) = value {
                                let root_is_const = if let Expression::Identifier(ref obj_ident) = member.object {
                                    tracker.const_bindings.contains(obj_ident.name.as_str())
                                } else {
                                    false
                                };
                                let signal_obj = member.unbox().object;
                                let wrapped = import_rewrite::build_wrap_prop_call(signal_obj, ctx);
                                if !tracker.needs_wrap_prop {
                                    tracker.needs_wrap_prop = true;
                                    tracker.record_synthetic_import("_wrapProp");
                                }
                                if is_fn || root_is_const {
                                    const_props.push((attr_name, wrapped));
                                } else {
                                    var_props.push((attr_name, wrapped));
                                }
                                continue;
                            }
                        }
                        SignalWrapResult::WrapPropNamed(prop_name, wrap_is_const) => {
                            // Build _wrapProp(source, "propName").
                            // Source is either the props param or _rawProps, extracted from
                            // a StaticMemberExpression/ComputedMemberExpression (for props.X or props["key"]).
                            let raw_props_fallback = props_param_name.unwrap_or("_rawProps");
                            let source_obj =
                                if let Expression::StaticMemberExpression(member) = value {
                                    member.unbox().object
                                } else if let Expression::ComputedMemberExpression(member) = value {
                                    member.unbox().object
                                } else {
                                    // For destructured prop identifiers, build props param reference
                                    let atom = ctx.ast.atom(raw_props_fallback);
                                    ctx.ast.expression_identifier(SPAN, atom)
                                };
                            let wrapped = import_rewrite::build_wrap_prop_call_named(
                                source_obj, &prop_name, ctx,
                            );
                            if !tracker.needs_wrap_prop {
                                tracker.needs_wrap_prop = true;
                                tracker.record_synthetic_import("_wrapProp");
                            }
                            // SWC places _wrapProp result in const_props when is_const=true
                            // (const local variables) or when the element is a component (is_fn).
                            // For non-const sources on native elements, it goes to var_props.
                            if wrap_is_const || is_fn {
                                const_props.push((attr_name, wrapped));
                            } else {
                                var_props.push((attr_name, wrapped));
                            }
                            continue;
                        }
                        SignalWrapResult::None => {}
                    }
                }

                if is_const_jsx_value(&value, &tracker.const_bindings) {
                    const_props.push((attr_name, value));
                } else {
                    // Props path: SWC uses accept_call_expr=true, so call expressions
                    // are allowed. The has_non_reactive flag from collect_reactive_deps
                    // provides the correct bailout for non-reactive refs.
                    let (deps, has_non_reactive) =
                        collect_reactive_deps(&value, destructured_props, module_imports, props_param_name, &tracker.const_bindings);
                    if !deps.is_empty() && !has_non_reactive {
                        // SWC's convert_inlined_fn checks is_used_as_object_or_call():
                        // only wrap with _fnSignal if at least one dep is used as the
                        // object of a member expression. If deps are only used as
                        // standalone identifiers or array indices, skip wrapping.
                        let dep_names: Vec<&str> =
                            deps.iter().map(|d| d.root_name.as_str()).collect();
                        // When a dep comes from destructured prop alias detection,
                        // bypass the is_any_dep_used_as_object check. The expression still
                        // has the original alias (e.g., `data`) which will be rewritten
                        // to `_rawProps.data` (or `props.data`) in the body. After rewriting,
                        // the dep IS used as object, so we can safely skip the check.
                        // This applies to both _rawProps (param destructuring) and named
                        // props params like "props" (body destructuring).
                        let has_destructured_raw_props = destructured_props
                            .map(|props| !props.is_empty())
                            .unwrap_or(false)
                            && deps.iter().any(|d| {
                                d.root_name == "_rawProps"
                                    || props_param_name.is_some_and(|p| p == d.root_name)
                            });
                        if has_destructured_raw_props || is_any_dep_used_as_object(&value, &dep_names) {
                            // Check if all dep roots are const-bound.
                            // SWC's compute_scoped_idents returns is_const=false
                            // when any dep is Var(false) (e.g., loop vars, function params).
                            let all_deps_const = deps.iter().all(|dep| {
                                tracker.const_bindings.contains(&dep.root_name)
                            });
                            // Wrap with _fnSignal
                            let (wrapped, fn_code, str_code) = build_fn_signal_wrapping(
                                value,
                                &deps,
                                destructured_props,
                                tracker,
                                ctx,
                                props_param_name,
                            );
                            if !tracker.needs_fn_signal {
                                tracker.needs_fn_signal = true;
                                tracker.record_synthetic_import("_fnSignal");
                            }
                            if !hoisted_stmts.iter().any(|(fc, _)| fc == &fn_code) {
                                hoisted_stmts.push((fn_code, str_code));
                            }
                            // SWC: convert_to_getter returns is_const from compute_scoped_idents.
                            // For is_fn (component) elements, always const_props.
                            // For native elements: const if all deps const, var otherwise.
                            if is_fn || all_deps_const {
                                const_props.push((attr_name, wrapped));
                            } else {
                                var_props.push((attr_name, wrapped));
                            }
                        } else {
                            // No dep used as object -> skip _fnSignal, treat as var
                            var_props.push((attr_name, value));
                        }
                    } else {
                        var_props.push((attr_name, value));
                    }
                }
            }
        }
    }

    // NOTE: q:p/q:ps injection for iteration variables is handled earlier in
    // replace_jsx_element_handlers (transform.rs), where we still have access to
    // the original lambda bodies before QRL replacement. By this point, handler
    // lambdas are already replaced by QRL identifiers, so we can't scan them.
    // The q:p/q:ps attributes are injected as JSX attributes on the element,
    // and will be classified into var_props above.

    // Detect text-only elements (SWC's is_text_only). For these elements,
    // children are NOT signal-wrapped; they keep the original expression and
    // the subtree is marked mutable. Matches SWC behavior (transform.rs ~1647).
    let is_text_only = match &element.opening_element.name {
        JSXElementName::Identifier(ident) => is_text_only_element(ident.name.as_str()),
        _ => false,
    };

    // Build children
    let (children_expr, _children_count, children_mutable) = transform_jsx_children(
        &mut element.children,
        tracker,
        ctx,
        destructured_props,
        module_imports,
        hoisted_stmts,
        loop_depth,
        iteration_vars,
        props_param_name,
        key_prefix,
        is_text_only,
    );

    // Compute immutability flags (mirrors SWC transform.rs lines 1570-1571, 1931-1937)
    //
    // SWC flag semantics:
    //   static_listeners (bit 0): false if spread, q:p present, or event handlers (q-e:*) in var_props
    //   static_subtree (bit 1): false if spread or children are mutable
    //
    // NOTE: var_props presence does NOT affect static_subtree. In SWC, elements with
    // event handlers or non-const props in var_props can still have static_subtree=true
    // as long as children are immutable. The var_props presence DOES propagate jsx_mutable
    // to the parent element (see below), which breaks the parent's static_subtree.
    let has_qp = var_props.iter().any(|(key, _)| key == "q:p" || key == "q:ps");
    let has_event_in_var_props = var_props.iter().any(|(key, _)| key.starts_with("q-e:"));
    let static_listeners = !has_spread && !has_qp && !has_event_in_var_props;
    let mut static_subtree = !has_spread;

    // Children mutability breaks static_subtree
    if children_mutable {
        static_subtree = false;
    }

    // Encode flags as bitfield: bit 0 = static_listeners, bit 1 = static_subtree
    let mut flags: u32 = 0;
    if static_listeners {
        flags |= 1;
    }
    if static_subtree {
        flags |= 2;
    }

    // Propagate mutability to parent element via tracker.jsx_mutable.
    // In SWC, var_props existence sets self.jsx_mutable = true (transform.rs line 1469),
    // which propagates to the parent's static_subtree check via the save/restore pattern
    // in transform_jsx_children.
    if has_spread || !var_props.is_empty() || children_mutable {
        tracker.jsx_mutable = true;
    }

    // Check for _createElement path before building key_expr, since _createElement
    // consumes the key_value differently (as a property, not a separate argument).
    if has_spread && key_value.is_some() {
        // SWC uses _createElement(tag, { ...source, ...props, key }) when:
        //   - There's a single spread source
        //   - The spread source is NOT the component's props parameter
        //   - The element has a user-provided key
        let spread_source_is_props_param = spread_args.first().map_or(false, |s| {
            if let Expression::Identifier(ident) = s {
                props_param_name.is_some_and(|p| p == ident.name.as_str())
            } else {
                false
            }
        });
        let use_create_element = spread_args.len() == 1 && !spread_source_is_props_param;

        if use_create_element {
            if !tracker.needs_create_element {
                tracker.needs_create_element = true;
                tracker.record_synthetic_import("createElement");
            }

            let spread_source = spread_args.remove(0);
            let user_key = key_value.unwrap();

            // Build props object: { ...source, ...explicit_props, key: keyValue }
            let total = 1 + var_props.len() + const_props.len() + 1;
            let mut obj_props = ctx.ast.vec_with_capacity(total);

            // Raw spread of source
            obj_props.push(ctx.ast.object_property_kind_spread_property(SPAN, spread_source));

            // Add all explicit props (both var and const) in source order
            for (name, value) in const_props.into_iter().chain(var_props) {
                let key = if name.contains(':') || name.contains('-') || name.contains('$') {
                    let atom = ctx.ast.atom(&name);
                    PropertyKey::from(ctx.ast.expression_string_literal(SPAN, atom, None))
                } else {
                    ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom(&name))
                };
                obj_props.push(ctx.ast.object_property_kind_object_property(
                    SPAN, PropertyKind::Init, key, value, false, false, false,
                ));
            }

            // Add key property
            let key_prop_key = ctx.ast.property_key_static_identifier(SPAN, ctx.ast.atom("key"));
            obj_props.push(ctx.ast.object_property_kind_object_property(
                SPAN, PropertyKind::Init, key_prop_key, user_key, false, false, false,
            ));

            let props_obj = ctx.ast.expression_object(SPAN, obj_props);

            let callee = ctx.ast.expression_identifier(SPAN, "_createElement");
            let mut arguments = ctx.ast.vec_with_capacity(2);
            arguments.push(Argument::from(tag));
            arguments.push(Argument::from(props_obj));

            return ctx.ast.expression_call_with_pure(
                SPAN,
                callee,
                None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                arguments,
                false,
                true,
            );
        }
    }

    // Generate key: component tags (is_fn) and root elements (root_jsx_mode) get keys,
    // nested native elements get null (mirrors SWC's should_emit_key = is_fn || root_jsx_mode)
    let should_emit_key = is_fn || root_jsx_mode;
    let key_expr = if let Some(key) = key_value {
        // User-provided key from JSX `key` prop
        key
    } else if should_emit_key {
        // Generate auto-key: "XX_N" where XX = key_prefix, N = counter
        let key_str = format!("{}_{}", key_prefix, tracker.jsx_key_counter);
        tracker.jsx_key_counter += 1;
        let atom = ctx.ast.atom(&key_str);
        ctx.ast.expression_string_literal(SPAN, atom, None)
    } else {
        // No key for nested native elements
        ctx.ast.expression_null_literal(SPAN)
    };

    if has_spread {
        // Use _jsxSplit for elements with spread attributes.
        // Record _getVarProps/_getConstProps BEFORE _jsxSplit to match SWC's
        // encounter order (SWC processes call arguments before the call itself).
        if !tracker.needs_get_var_props {
            tracker.needs_get_var_props = true;
            tracker.record_synthetic_import("_getVarProps");
        }
        if !tracker.needs_get_const_props {
            tracker.needs_get_const_props = true;
            tracker.record_synthetic_import("_getConstProps");
        }
        if !tracker.needs_jsx_split {
            tracker.needs_jsx_split = true;
            tracker.record_synthetic_import("_jsxSplit");
        }

        // Build: _jsxSplit(tag, { ..._getVarProps(source) }, _getConstProps(source), children, flags, key)
        // For multiple spreads, use the first one (simplification)
        let spread_source = if !spread_args.is_empty() {
            spread_args.remove(0)
        } else {
            ctx.ast.expression_identifier(SPAN, "undefined")
        };
        // Extract spread source name for creating multiple identifier references.
        // Track whether the source was an identifier (vs member expression) to decide
        // whether to use _getVarProps/_getConstProps splitting.
        let spread_source_is_ident = matches!(&spread_source, Expression::Identifier(_));
        let spread_source_name: String = if let Expression::Identifier(ref ident) = spread_source {
            ident.name.as_str().to_string()
        } else {
            "props".to_string()
        };
        // We no longer need the original spread_source expression for single-spread case
        // because we'll create fresh identifiers from spread_source_name.
        let _ = spread_source;

        // Build varProps = { ...before, ..._getVarProps(source), ...after }
        // Interleave explicit var_props AND const_props with _getVarProps(source) at the
        // position where the spread appeared in source, matching SWC's prop ordering.
        //
        // In _jsxSplit mode, ALL explicit props go into the var_props object in source
        // order. The const_props/var_props classification is irrelevant for explicit
        // attributes -- only _getConstProps(source) handles the spread source's const
        // props. For multiple spreads, _getConstProps is also inlined and const_props arg
        // is null.
        let has_multiple_spreads = spread_args.len() > 0; // remaining after first was removed
        let var_insert_at = spread_insert_idx.unwrap_or(0);
        let const_insert_at = const_spread_insert_idx.unwrap_or(0);

        // Merge explicit const_props into var_props at their source positions.
        // const_props before the spread go before _getVarProps, after var_props before spread.
        // const_props after the spread go after _getVarProps, before var_props after spread.
        // We build a single ordered list of all explicit props.
        let mut all_before: Vec<(String, Expression<'a>)> = Vec::new();
        let mut all_after: Vec<(String, Expression<'a>)> = Vec::new();

        // Drain const_props and var_props relative to spread position
        let const_before: Vec<_> = const_props.drain(..const_insert_at.min(const_props.len())).collect();
        let var_before: Vec<_> = var_props.drain(..var_insert_at.min(var_props.len())).collect();
        let const_after: Vec<_> = const_props.drain(..).collect();
        let var_after: Vec<_> = var_props.drain(..).collect();

        // For multi-spread: all explicit props (const + var) go together in the var_props
        // object because _getConstProps is inlined as spread.
        // For single-spread: const and var are separated — const_props may go to the
        // 3rd arg or inline as _getConstProps spread, while var_props go in 2nd arg.
        // We track explicit_const separately for single-spread to use later.
        let explicit_const: Vec<(String, Expression<'a>)>;
        if has_multiple_spreads {
            all_before.extend(const_before.into_iter().chain(var_before));
            all_after.extend(const_after.into_iter().chain(var_after));
            explicit_const = Vec::new(); // not used for multi-spread
        } else {
            // Single spread: const_before goes into 2nd arg (before spread), but
            // const_after is kept separate for potential 3rd arg placement.
            all_before.extend(const_before.into_iter().chain(var_before));
            all_after.extend(var_after);
            explicit_const = const_after;
        }

        let total_props = all_before.len() + all_after.len() + 2 + if has_multiple_spreads { 1 + spread_args.len() } else { 0 };
        let mut var_obj_props = ctx.ast.vec_with_capacity(total_props);

        // Helper closure: build an ObjectProperty from (name, value)
        let build_var_prop = |name: &str, value: Expression<'a>, ctx: &mut TraverseCtx<'a, ()>| -> ObjectPropertyKind<'a> {
            let key = if name.contains(':') || name.contains('-') || name.contains('$') {
                let atom = ctx.ast.atom(name);
                PropertyKey::from(ctx.ast.expression_string_literal(SPAN, atom, None))
            } else {
                ctx.ast
                    .property_key_static_identifier(SPAN, ctx.ast.atom(name))
            };
            ctx.ast.object_property_kind_object_property(
                SPAN,
                PropertyKind::Init,
                key,
                value,
                false,
                false,
                false,
            )
        };

        // Add explicit props that appeared BEFORE the spread
        for (name, value) in all_before {
            var_obj_props.push(build_var_prop(&name, value, ctx));
        }

        // _getVarProps(source) spread
        let get_var_callee = ctx.ast.expression_identifier(SPAN, "_getVarProps");
        let mut get_var_args = ctx.ast.vec_with_capacity(1);
        get_var_args.push(Argument::from(
            ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
        ));
        let get_var_call = ctx.ast.expression_call(
            SPAN,
            get_var_callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            get_var_args,
            false,
        );
        var_obj_props.push(
            ctx.ast
                .object_property_kind_spread_property(SPAN, get_var_call),
        );

        // For multiple spreads: add _getConstProps(source) spread inline + remaining spread args
        let const_props_expr = if has_multiple_spreads {
            // _getConstProps(source) as spread inside var_props object
            let get_const_callee = ctx.ast.expression_identifier(SPAN, "_getConstProps");
            let mut get_const_args = ctx.ast.vec_with_capacity(1);
            get_const_args.push(Argument::from(
                ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
            ));
            let get_const_call = ctx.ast.expression_call(
                SPAN,
                get_const_callee,
                None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                get_const_args,
                false,
            );
            var_obj_props.push(
                ctx.ast
                    .object_property_kind_spread_property(SPAN, get_const_call),
            );

            // Add explicit props that appeared AFTER the first spread
            // (these come between _getConstProps and remaining spread args)
            for (name, value) in all_after {
                var_obj_props.push(build_var_prop(&name, value, ctx));
            }

            // Add remaining spread args as spreads.
            // If the extra spread source is the same identifier as the first spread
            // source, wrap it in _getVarProps(). Otherwise, use raw spread.
            let mut all_same_source = spread_source_is_ident;
            for extra_spread in spread_args {
                let is_same_source = spread_source_is_ident
                    && matches!(&extra_spread, Expression::Identifier(ident) if ident.name.as_str() == spread_source_name);
                if !is_same_source {
                    all_same_source = false;
                }
                if is_same_source {
                    // Wrap in _getVarProps(source)
                    let get_var_callee2 = ctx.ast.expression_identifier(SPAN, "_getVarProps");
                    let mut get_var_args2 = ctx.ast.vec_with_capacity(1);
                    get_var_args2.push(Argument::from(extra_spread));
                    let get_var_call2 = ctx.ast.expression_call(
                        SPAN,
                        get_var_callee2,
                        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                        get_var_args2,
                        false,
                    );
                    var_obj_props.push(
                        ctx.ast.object_property_kind_spread_property(SPAN, get_var_call2),
                    );
                } else {
                    var_obj_props.push(
                        ctx.ast.object_property_kind_spread_property(SPAN, extra_spread),
                    );
                }
            }

            // const_props arg: when ALL spreads are the same identifier source,
            // emit _getConstProps(source) as bare call for 3rd arg.
            // When spreads have different sources, use null (SWC already inlined
            // _getConstProps as spread in the 2nd arg object above).
            if all_same_source {
                let get_const_callee = ctx.ast.expression_identifier(SPAN, "_getConstProps");
                let mut get_const_args = ctx.ast.vec_with_capacity(1);
                get_const_args.push(Argument::from(
                    ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
                ));
                ctx.ast.expression_call(
                    SPAN,
                    get_const_callee,
                    None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                    get_const_args,
                    false,
                )
            } else {
                ctx.ast.expression_null_literal(SPAN)
            }
        } else {
            // In spread elements, reclassify const_after entries that reference
            // the spread source as var props (2nd arg). SWC treats QRLs with
            // captures referencing the spread source as var_props for _jsxSplit,
            // even though is_const_event_handler returns true for them.
            let mut remaining_const: Vec<(String, Expression<'a>)> = Vec::new();
            for (name, value) in explicit_const {
                if expr_contains_ident(&value, &spread_source_name) {
                    all_after.push((name, value));
                } else {
                    remaining_const.push((name, value));
                }
            }
            let explicit_const = remaining_const;

            // Placement of _getConstProps depends on:
            //   A) Whether explicit const_after exists (after reclassification)
            //   B) Whether var_after has _fnSignal wrapping that references the spread source
            //
            // SWC algorithm:
            // - If const_after non-empty OR var_after references spread source:
            //   Put ..._getConstProps(source) in 2nd arg (before var_after)
            //   3rd = { ...const_after } or null
            // - If const_after empty AND var_after doesn't reference spread source:
            //   3rd = _getConstProps(source) [bare call]
            let has_explicit_const = !explicit_const.is_empty();

            // Check if any var_after expression uses _fnSignal wrapping that
            // references the spread source. _fnSignal's deps array contains the
            // spread source, requiring const props to be available before evaluation.
            // Other expressions (plain identifiers, qrl captures) don't need this.
            let var_after_has_fn_signal_with_source = all_after.iter().any(|(_, value)| {
                if let Expression::CallExpression(call) = value {
                    if let Expression::Identifier(callee) = &call.callee {
                        if callee.name.as_str() == "_fnSignal" {
                            return call.arguments.iter().any(|arg| {
                                if let Some(expr) = arg.as_expression() {
                                    expr_contains_ident(expr, &spread_source_name)
                                } else {
                                    false
                                }
                            });
                        }
                    }
                }
                false
            });

            let has_var_after = !all_after.is_empty();
            let put_const_in_second = (has_explicit_const && has_var_after) || var_after_has_fn_signal_with_source;

            if put_const_in_second {
                // _getConstProps goes in 2nd arg as spread
                let get_const_callee = ctx.ast.expression_identifier(SPAN, "_getConstProps");
                let mut get_const_args = ctx.ast.vec_with_capacity(1);
                get_const_args.push(Argument::from(
                    ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
                ));
                let get_const_call = ctx.ast.expression_call(
                    SPAN,
                    get_const_callee,
                    None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                    get_const_args,
                    false,
                );
                var_obj_props.push(
                    ctx.ast.object_property_kind_spread_property(SPAN, get_const_call),
                );

                // Add explicit var_props after the spread
                for (name, value) in all_after {
                    var_obj_props.push(build_var_prop(&name, value, ctx));
                }

                // 3rd arg: explicit const props as object, or null
                if has_explicit_const {
                    let mut const_obj_props = ctx.ast.vec_with_capacity(explicit_const.len());
                    for (name, value) in explicit_const {
                        const_obj_props.push(build_var_prop(&name, value, ctx));
                    }
                    ctx.ast.expression_object(SPAN, const_obj_props)
                } else {
                    ctx.ast.expression_null_literal(SPAN)
                }
            } else if has_explicit_const {
                // const_after only (no var_after reference to source)
                // Build 3rd arg: { ..._getConstProps(source), ...explicit_const }
                let mut const_obj_props = ctx.ast.vec_with_capacity(explicit_const.len() + 1);
                let get_const_callee = ctx.ast.expression_identifier(SPAN, "_getConstProps");
                let mut get_const_args = ctx.ast.vec_with_capacity(1);
                get_const_args.push(Argument::from(
                    ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
                ));
                let get_const_call = ctx.ast.expression_call(
                    SPAN,
                    get_const_callee,
                    None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                    get_const_args,
                    false,
                );
                const_obj_props.push(
                    ctx.ast.object_property_kind_spread_property(SPAN, get_const_call),
                );
                for (name, value) in explicit_const {
                    const_obj_props.push(build_var_prop(&name, value, ctx));
                }
                ctx.ast.expression_object(SPAN, const_obj_props)
            } else {
                // No explicit const, no source reference in var_after
                // Add var_after props to 2nd arg
                for (name, value) in all_after {
                    var_obj_props.push(build_var_prop(&name, value, ctx));
                }
                // 3rd = bare _getConstProps call
                let get_const_callee = ctx.ast.expression_identifier(SPAN, "_getConstProps");
                let mut get_const_args = ctx.ast.vec_with_capacity(1);
                get_const_args.push(Argument::from(
                    ctx.ast.expression_identifier(SPAN, ctx.ast.atom(&spread_source_name)),
                ));
                ctx.ast.expression_call(
                    SPAN,
                    get_const_callee,
                    None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                    get_const_args,
                    false,
                )
            }
        };

        let var_props_expr = ctx.ast.expression_object(SPAN, var_obj_props);

        let callee = ctx.ast.expression_identifier(SPAN, "_jsxSplit");
        let capacity = if tracker.jsx_dev_file_name.is_some() { 7 } else { 6 };
        let mut arguments = ctx.ast.vec_with_capacity(capacity);
        arguments.push(Argument::from(tag));
        arguments.push(Argument::from(var_props_expr));
        arguments.push(Argument::from(const_props_expr));
        arguments.push(Argument::from(
            children_expr.unwrap_or_else(|| ctx.ast.expression_null_literal(SPAN)),
        ));
        arguments.push(Argument::from(ctx.ast.expression_numeric_literal(
            SPAN,
            flags as f64,
            None,
            NumberBase::Decimal,
        )));
        arguments.push(Argument::from(key_expr));

        // Dev mode: append { fileName, lineNumber, columnNumber }
        if let Some(ref dev_file) = tracker.jsx_dev_file_name {
            let loc = compute_jsx_dev_location(
                dev_file,
                tracker.jsx_dev_source_code.as_deref().unwrap_or(""),
                element_span_start,
            );
            arguments.push(Argument::from(
                import_rewrite::build_jsx_dev_location(&loc, ctx),
            ));
        }

        ctx.ast.expression_call_with_pure(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            arguments,
            false,
            true,
        )
    } else {
        // Use _jsxSorted for normal elements
        if !tracker.needs_jsx_sorted {
            tracker.needs_jsx_sorted = true;
            let jsx_name = if tracker.custom_jsx_source.is_some() { "_jsx" } else { "_jsxSorted" };
            tracker.record_synthetic_import(jsx_name);
        }

        // Build var_props object or null
        let var_props_arg = if var_props.is_empty() {
            ctx.ast.expression_null_literal(SPAN)
        } else {
            let mut props_vec = ctx.ast.vec_with_capacity(var_props.len());
            for (name, value) in var_props {
                // Use string literal key for names with special chars (q:p, etc.)
                let key = if name.contains(':') || name.contains('-') || name.contains('$') {
                    let atom = ctx.ast.atom(&name);
                    PropertyKey::from(ctx.ast.expression_string_literal(SPAN, atom, None))
                } else {
                    ctx.ast
                        .property_key_static_identifier(SPAN, ctx.ast.atom(&name))
                };
                props_vec.push(ctx.ast.object_property_kind_object_property(
                    SPAN,
                    PropertyKind::Init,
                    key,
                    value,
                    false,
                    false,
                    false,
                ));
            }
            ctx.ast.expression_object(SPAN, props_vec)
        };

        // Build const_props object or null
        let const_props_arg = if const_props.is_empty() {
            ctx.ast.expression_null_literal(SPAN)
        } else {
            let mut props_vec = ctx.ast.vec_with_capacity(const_props.len());
            for (name, value) in const_props {
                // Use string literal key for names with special chars (q-e:, etc.)
                let key = if name.contains(':') || name.contains('-') || name.contains('$') {
                    let atom = ctx.ast.atom(&name);
                    PropertyKey::from(ctx.ast.expression_string_literal(SPAN, atom, None))
                } else {
                    ctx.ast
                        .property_key_static_identifier(SPAN, ctx.ast.atom(&name))
                };
                props_vec.push(ctx.ast.object_property_kind_object_property(
                    SPAN,
                    PropertyKind::Init,
                    key,
                    value,
                    false,
                    false,
                    false,
                ));
            }
            ctx.ast.expression_object(SPAN, props_vec)
        };

        let callee = ctx.ast.expression_identifier(SPAN, "_jsxSorted");
        let capacity = if tracker.jsx_dev_file_name.is_some() { 7 } else { 6 };
        let mut arguments = ctx.ast.vec_with_capacity(capacity);
        arguments.push(Argument::from(tag));
        arguments.push(Argument::from(var_props_arg));
        arguments.push(Argument::from(const_props_arg));
        arguments.push(Argument::from(
            children_expr.unwrap_or_else(|| ctx.ast.expression_null_literal(SPAN)),
        ));
        arguments.push(Argument::from(ctx.ast.expression_numeric_literal(
            SPAN,
            flags as f64,
            None,
            NumberBase::Decimal,
        )));
        arguments.push(Argument::from(key_expr));

        // Dev mode: append { fileName, lineNumber, columnNumber }
        if let Some(ref dev_file) = tracker.jsx_dev_file_name {
            let loc = compute_jsx_dev_location(
                dev_file,
                tracker.jsx_dev_source_code.as_deref().unwrap_or(""),
                element_span_start,
            );
            arguments.push(Argument::from(
                import_rewrite::build_jsx_dev_location(&loc, ctx),
            ));
        }

        ctx.ast.expression_call_with_pure(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            arguments,
            false,
            true,
        )
    }
}

/// Recursively transform a JSXFragment into a _jsxSorted call with _Fragment tag.
pub(crate) fn transform_jsx_fragment_inner<'a>(
    mut fragment: JSXFragment<'a>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    destructured_props: Option<&[(String, String)]>,
    module_imports: &[crate::types::ImportInfo],
    hoisted_stmts: &mut Vec<(String, String)>,
    loop_depth: u32,
    iteration_vars: &[String],
    props_param_name: Option<&str>,
    _root_jsx_mode: bool,
    key_prefix: &str,
) -> Expression<'a> {
    // Capture fragment span for dev mode location metadata
    let fragment_span_start = fragment.opening_fragment.span.start;

    // DON'T set needs_jsx_sorted/needs_fragment flags before children processing.
    // Let child elements record _jsxSorted naturally during their own transform,
    // which places it in the correct encounter-order position (matching SWC's fold
    // where children are processed before the parent Fragment).

    let tag = ctx.ast.expression_identifier(SPAN, "_Fragment");

    // Build children (fragments are never text-only)
    let (children_expr, _children_count, children_mutable) = transform_jsx_children(
        &mut fragment.children,
        tracker,
        ctx,
        destructured_props,
        module_imports,
        hoisted_stmts,
        loop_depth,
        iteration_vars,
        props_param_name,
        key_prefix,
        false,
    );

    // Record _jsxSorted and _Fragment AFTER children processing.
    // If a child element already recorded _jsxSorted, these are no-ops (idempotent).
    // If no child needed _jsxSorted (unlikely for fragments), this records it now.
    if !tracker.needs_jsx_sorted {
        tracker.needs_jsx_sorted = true;
        let jsx_name = if tracker.custom_jsx_source.is_some() { "_jsx" } else { "_jsxSorted" };
        tracker.record_synthetic_import(jsx_name);
    }
    if !tracker.needs_fragment {
        tracker.needs_fragment = true;
        tracker.record_synthetic_import("_Fragment");
    }

    // Fragment has no props, no spread, no event handlers
    // static_listeners is always true for fragments
    let static_subtree = !children_mutable;
    let mut flags: u32 = 1; // static_listeners = true (bit 0)
    if static_subtree {
        flags |= 2; // bit 1
    }

    // Propagate children mutability to parent via tracker.jsx_mutable.
    if children_mutable {
        tracker.jsx_mutable = true;
    }

    // Generate auto-key (fragments always emit key -- is_fn=true in SWC)
    let key_str = format!("{}_{}", key_prefix, tracker.jsx_key_counter);
    tracker.jsx_key_counter += 1;
    let key_atom = ctx.ast.atom(&key_str);
    let key_expr = ctx.ast.expression_string_literal(SPAN, key_atom, None);

    let callee = ctx.ast.expression_identifier(SPAN, "_jsxSorted");
    let capacity = if tracker.jsx_dev_file_name.is_some() { 7 } else { 6 };
    let mut arguments = ctx.ast.vec_with_capacity(capacity);
    arguments.push(Argument::from(tag));
    arguments.push(Argument::from(ctx.ast.expression_null_literal(SPAN)));
    arguments.push(Argument::from(ctx.ast.expression_null_literal(SPAN)));
    arguments.push(Argument::from(
        children_expr.unwrap_or_else(|| ctx.ast.expression_null_literal(SPAN)),
    ));
    arguments.push(Argument::from(ctx.ast.expression_numeric_literal(
        SPAN,
        flags as f64,
        None,
        NumberBase::Decimal,
    )));
    arguments.push(Argument::from(key_expr));

    // Dev mode: append { fileName, lineNumber, columnNumber }
    if let Some(ref dev_file) = tracker.jsx_dev_file_name {
        let loc = compute_jsx_dev_location(
            dev_file,
            tracker.jsx_dev_source_code.as_deref().unwrap_or(""),
            fragment_span_start,
        );
        arguments.push(Argument::from(
            import_rewrite::build_jsx_dev_location(&loc, ctx),
        ));
    }

    ctx.ast.expression_call_with_pure(
        SPAN,
        callee,
        None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
        arguments,
        false,
        true,
    )
}

/// Transform JSX children to expression(s).
/// Returns (children_expression, significant_children_count, any_child_mutable).
/// The third element indicates whether any child was mutable (for flag computation).
pub(crate) fn transform_jsx_children<'a>(
    children: &mut oxc::allocator::Vec<'a, JSXChild<'a>>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    destructured_props: Option<&[(String, String)]>,
    module_imports: &[crate::types::ImportInfo],
    hoisted_stmts: &mut Vec<(String, String)>,
    loop_depth: u32,
    iteration_vars: &[String],
    props_param_name: Option<&str>,
    key_prefix: &str,
    is_text_only: bool,
) -> (Option<Expression<'a>>, usize, bool) {
    let mut child_exprs: Vec<Expression<'a>> = Vec::new();
    let mut any_child_mutable = false;

    // Take children out to process them
    let mut old_children = ctx.ast.vec();
    std::mem::swap(children, &mut old_children);

    for child in old_children {
        match child {
            JSXChild::Text(text) => {
                // String literals are immutable -- no change to any_child_mutable
                let trimmed = normalize_jsx_text(text.value.as_str());
                if !trimmed.is_empty() {
                    let atom = ctx.ast.atom(&trimmed);
                    child_exprs.push(ctx.ast.expression_string_literal(SPAN, atom, None));
                }
            }
            JSXChild::Element(el) => {
                // Capture pre-existing jsx_mutable from inner exit_expression
                // processing (bottom-up: inner JSX elements transformed before parent).
                // E.g., <Stuff/> inside a ternary sets jsx_mutable via exit_expression
                // before the parent element's transform_jsx_children runs.
                if tracker.jsx_mutable {
                    any_child_mutable = true;
                }
                // Save and reset for child element processing
                let prev_mutable = tracker.jsx_mutable;
                tracker.jsx_mutable = false;

                // Recursively transform child JSXElement (children are never root)
                let transformed = transform_jsx_element_inner(
                    el.unbox(),
                    tracker,
                    ctx,
                    destructured_props,
                    module_imports,
                    hoisted_stmts,
                    loop_depth,
                    iteration_vars,
                    props_param_name,
                    false,
                    key_prefix,
                );

                // If child element set jsx_mutable, this subtree is mutable
                if tracker.jsx_mutable {
                    any_child_mutable = true;
                }
                tracker.jsx_mutable = prev_mutable;

                child_exprs.push(transformed);
            }
            JSXChild::Fragment(frag) => {
                // Capture pre-existing jsx_mutable from inner exit_expression
                if tracker.jsx_mutable {
                    any_child_mutable = true;
                }
                // Save and reset for child fragment processing
                let prev_mutable = tracker.jsx_mutable;
                tracker.jsx_mutable = false;

                // Recursively transform child JSXFragment (children are never root)
                let transformed = transform_jsx_fragment_inner(
                    frag.unbox(),
                    tracker,
                    ctx,
                    destructured_props,
                    module_imports,
                    hoisted_stmts,
                    loop_depth,
                    iteration_vars,
                    props_param_name,
                    false,
                    key_prefix,
                );

                // If child fragment set jsx_mutable, this subtree is mutable
                if tracker.jsx_mutable {
                    any_child_mutable = true;
                }
                tracker.jsx_mutable = prev_mutable;

                child_exprs.push(transformed);
            }
            JSXChild::ExpressionContainer(container) => {
                let container = container.unbox();
                match container.expression {
                    JSXExpression::EmptyExpression(_) => {
                        // Skip empty expressions {}
                    }
                    expr => {
                        // The expression inside the container may already have been
                        // transformed by exit_expression (e.g., $() calls, or inner
                        // JSXElement expressions). Convert to Expression.
                        let transformed = jsx_expression_to_expression(expr, ctx);
                        // If it's a JSXElement or JSXFragment, transform it
                        match transformed {
                            Expression::JSXElement(el) => {
                                // Capture pre-existing jsx_mutable from inner exit_expression
                                if tracker.jsx_mutable {
                                    any_child_mutable = true;
                                }
                                // Save/restore jsx_mutable around element processing
                                let prev_mutable = tracker.jsx_mutable;
                                tracker.jsx_mutable = false;

                                let result = transform_jsx_element_inner(
                                    el.unbox(),
                                    tracker,
                                    ctx,
                                    destructured_props,
                                    module_imports,
                                    hoisted_stmts,
                                    loop_depth,
                                    iteration_vars,
                                    props_param_name,
                                    false,
                                    key_prefix,
                                );

                                if tracker.jsx_mutable {
                                    any_child_mutable = true;
                                }
                                tracker.jsx_mutable = prev_mutable;

                                child_exprs.push(result);
                            }
                            Expression::JSXFragment(frag) => {
                                // Capture pre-existing jsx_mutable from inner exit_expression
                                if tracker.jsx_mutable {
                                    any_child_mutable = true;
                                }
                                // Save/restore jsx_mutable around fragment processing
                                let prev_mutable = tracker.jsx_mutable;
                                tracker.jsx_mutable = false;

                                let result = transform_jsx_fragment_inner(
                                    frag.unbox(),
                                    tracker,
                                    ctx,
                                    destructured_props,
                                    module_imports,
                                    hoisted_stmts,
                                    loop_depth,
                                    iteration_vars,
                                    props_param_name,
                                    false,
                                    key_prefix,
                                );

                                if tracker.jsx_mutable {
                                    any_child_mutable = true;
                                }
                                tracker.jsx_mutable = prev_mutable;

                                child_exprs.push(result);
                            }
                            other => {
                                // For text-only elements (title, textarea, etc.),
                                // SWC skips signal wrapping and marks children as mutable.
                                // (SWC transform.rs ~1647: is_text_only branch)
                                if is_text_only {
                                    any_child_mutable = true;
                                    child_exprs.push(other);
                                    continue;
                                }
                                // Check for signal wrapping in children.
                                // SWC's convert_to_signal_item returns (is_const, expr):
                                // - WrapPropSignal (signal.value): is_const=true
                                // - WrapPropNamed (props.X, destructured): is_const=false
                                // - _fnSignal wrapping: is_const=true (const call)
                                // - No wrapping: check is_const_expression
                                if !is_call_on_value(&other) {
                                    match detect_signal_wrap(&other, destructured_props, props_param_name, module_imports, &tracker.const_bindings) {
                                        SignalWrapResult::WrapPropSignal => {
                                            // signal.value -> _wrapProp(signal)
                                            // SWC: is_const depends on compute_scoped_idents.
                                            // For local const vars from useSignal() → Var(true) → is_const=true.
                                            // For function params (e.g., .map(v => ...)) → Var(false) → is_const=false.
                                            if let Expression::StaticMemberExpression(member) =
                                                other
                                            {
                                                // Check if the signal root is const-bound.
                                                // Look through TS wrappers (TSAsExpression etc.) to find the underlying ident.
                                                let root_is_const = {
                                                    let mut obj = &member.object;
                                                    loop {
                                                        match obj {
                                                            Expression::TSAsExpression(ts) => obj = &ts.expression,
                                                            Expression::TSSatisfiesExpression(ts) => obj = &ts.expression,
                                                            Expression::TSNonNullExpression(ts) => obj = &ts.expression,
                                                            Expression::TSTypeAssertion(ts) => obj = &ts.expression,
                                                            Expression::ParenthesizedExpression(paren) => obj = &paren.expression,
                                                            _ => break,
                                                        }
                                                    }
                                                    if let Expression::Identifier(obj_ident) = obj {
                                                        tracker.const_bindings.contains(obj_ident.name.as_str())
                                                    } else {
                                                        false
                                                    }
                                                };
                                                if !root_is_const {
                                                    any_child_mutable = true;
                                                }
                                                let signal_obj = member.unbox().object;
                                                let wrapped =
                                                    import_rewrite::build_wrap_prop_call(
                                                        signal_obj, ctx,
                                                    );
                                                if !tracker.needs_wrap_prop {
                                                    tracker.needs_wrap_prop = true;
                                                    tracker.record_synthetic_import("_wrapProp");
                                                }
                                                child_exprs.push(wrapped);
                                                continue;
                                            }
                                        }
                                        SignalWrapResult::WrapPropNamed(prop_name, wrap_is_const) => {
                                            // _rawProps.propName, props.X, props["X"], destructured prop,
                                            // or local variable member access ->
                                            // _wrapProp(source, "propName")
                                            // SWC: is_const depends on the source variable's constness.
                                            // When is_const=false, SWC sets jsx_mutable=true.
                                            if !wrap_is_const {
                                                any_child_mutable = true;
                                            }
                                            let raw_props_fallback = props_param_name.unwrap_or("_rawProps");
                                            let source_obj = if let Expression::StaticMemberExpression(member) = other {
                                                member.unbox().object
                                            } else if let Expression::ComputedMemberExpression(member) = other {
                                                member.unbox().object
                                            } else {
                                                let atom = ctx.ast.atom(raw_props_fallback);
                                                ctx.ast.expression_identifier(SPAN, atom)
                                            };
                                            let wrapped =
                                                import_rewrite::build_wrap_prop_call_named(
                                                    source_obj, &prop_name, ctx,
                                                );
                                            if !tracker.needs_wrap_prop {
                                                tracker.needs_wrap_prop = true;
                                                tracker.record_synthetic_import("_wrapProp");
                                            }
                                            child_exprs.push(wrapped);
                                            continue;
                                        }
                                        SignalWrapResult::None => {}
                                    }
                                }
                                // Check for _fnSignal wrapping (complex reactive
                                // expressions)
                                // SWC: _fnSignal is_const depends on whether all deps
                                // are const-bound (imports or const declarations with
                                // static initializers). If any dep is non-const (e.g.,
                                // a function parameter like `props`), jsx_mutable=true.
                                if !is_call_on_value(&other)
                                    && !contains_function_call(&other)
                                {
                                    let (deps, has_non_reactive) = collect_reactive_deps(
                                        &other,
                                        destructured_props,
                                        module_imports,
                                        props_param_name,
                                        &tracker.const_bindings,
                                    );
                                    if !deps.is_empty() && !has_non_reactive {
                                        // SWC's convert_inlined_fn checks is_used_as_object_or_call():
                                        // only wrap if at least one dep is used as the object of
                                        // a member expression.
                                        let dep_names: Vec<&str> =
                                            deps.iter().map(|d| d.root_name.as_str()).collect();
                                        // When _rawProps is a dep from destructured prop alias detection,
                                        // bypass the is_any_dep_used_as_object check (see props path).
                                        let has_destructured_raw_props = destructured_props
                                            .map(|props| !props.is_empty())
                                            .unwrap_or(false)
                                            && deps.iter().any(|d| d.root_name == "_rawProps");
                                        if has_destructured_raw_props || is_any_dep_used_as_object(&other, &dep_names) {
                                            // Check if all dep roots are const-bound.
                                            // SWC's compute_scoped_idents returns is_const=false
                                            // when any dep is Var(false) (e.g., function params).
                                            let all_deps_const = deps.iter().all(|dep| {
                                                tracker.const_bindings.contains(&dep.root_name)
                                            });
                                            if !all_deps_const {
                                                any_child_mutable = true;
                                            }
                                            let (wrapped, fn_code, str_code) =
                                                build_fn_signal_wrapping(
                                                    other,
                                                    &deps,
                                                    destructured_props,
                                                    tracker,
                                                    ctx,
                                                    props_param_name,
                                                );
                                            if !tracker.needs_fn_signal {
                                                tracker.needs_fn_signal = true;
                                                tracker.record_synthetic_import("_fnSignal");
                                            }
                                            if !hoisted_stmts.iter().any(|(fc, _)| fc == &fn_code)
                                            {
                                                hoisted_stmts.push((fn_code, str_code));
                                            }
                                            child_exprs.push(wrapped);
                                            continue;
                                        } else {
                                            // No dep used as object -> skip _fnSignal,
                                            // but mark mutable since we have deps
                                            any_child_mutable = true;
                                        }
                                    } else if !deps.is_empty() && has_non_reactive {
                                        // Expression has reactive deps (local vars) mixed with
                                        // non-reactive refs (imports/globals). SWC's
                                        // create_synthetic_qqsegment returns (None, false):
                                        // scoped_idents is non-empty + contains_side_effect.
                                        // This makes jsx_mutable=true.
                                        any_child_mutable = true;
                                    }
                                }
                                // Check if tracker.jsx_mutable was set by an inner JSX element
                                // that was already transformed by exit_expression (bottom-up).
                                // This happens when JSX elements are nested inside non-JSX
                                // expressions like ternaries or logical &&.
                                // Example: {cond ? <p/> : <Stuff/>} -- <Stuff/>'s exit_expression
                                // set jsx_mutable, and we need to pick it up here.
                                if tracker.jsx_mutable {
                                    any_child_mutable = true;
                                    // Reset so it doesn't leak to sibling expressions.
                                    // The parent will propagate via Fix A if needed.
                                    tracker.jsx_mutable = false;
                                }
                                // No wrapping happened. Check the expression itself
                                // for mutability using scope-aware classification.
                                // Identifiers are checked against const_bindings
                                // (imports + const declarations) to determine if
                                // they're const or mutable.
                                if !is_child_expression_immutable(&other, module_imports, &tracker.const_bindings) {
                                    any_child_mutable = true;
                                }
                                // Check for already-transformed _jsxSorted calls with
                                // non-immutable component tags. In OXC's bottom-up
                                // traversal, inner JSX elements like <Stuff/> are
                                // transformed before their parent processes children.
                                // SWC detects these top-down; we scan the expression.
                                if contains_mutable_jsx_call(
                                    &other,
                                    &tracker.immutable_function_cmp,
                                ) {
                                    any_child_mutable = true;
                                }
                                child_exprs.push(other);
                            }
                        }
                    }
                }
            }
            JSXChild::Spread(spread) => {
                // Spread children make subtree mutable
                any_child_mutable = true;
                let spread = spread.unbox();
                child_exprs.push(spread.expression);
            }
        }
    }

    let count = child_exprs.len();
    match count {
        0 => (None, 0, any_child_mutable),
        1 => (
            Some(child_exprs.into_iter().next().unwrap()),
            1,
            any_child_mutable,
        ),
        _ => {
            let mut elements = ctx.ast.vec_with_capacity(count);
            for child in child_exprs {
                elements.push(ArrayExpressionElement::from(child));
            }
            (
                Some(ctx.ast.expression_array(SPAN, elements)),
                count,
                any_child_mutable,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Custom JSX import source transform (React-style _jsx)
// ---------------------------------------------------------------------------

/// Transform a JSXElement using the standard React JSX runtime form: `_jsx("tag", {props})`.
///
/// Unlike Qwik's `_jsxSorted`, this:
/// - Merges all props into a single object (no var/const split)
/// - Does NOT rename event handlers (onClick$ stays as onClick$)
/// - Uses 2-arg form: `_jsx(tag, props)`
/// - Children go into the props object as `children` key
fn transform_jsx_element_custom_source<'a>(
    mut element: JSXElement<'a>,
    tracker: &mut ImportTracker,
    ctx: &mut TraverseCtx<'a, ()>,
    hoisted_stmts: &mut Vec<(String, String)>,
    module_imports: &[crate::types::ImportInfo],
    destructured_props: Option<&[(String, String)]>,
    loop_depth: u32,
    iteration_vars: &[String],
    props_param_name: Option<&str>,
    key_prefix: &str,
) -> Expression<'a> {
    // Signal that we need the _jsx import (reuses needs_jsx_sorted flag --
    // the import emission in transform.rs will emit _jsx instead of _jsxSorted
    // when custom_jsx_source is set).
    if !tracker.needs_jsx_sorted {
        tracker.needs_jsx_sorted = true;
        let jsx_name = if tracker.custom_jsx_source.is_some() { "_jsx" } else { "_jsxSorted" };
        tracker.record_synthetic_import(jsx_name);
    }

    let tag = build_tag_expression(&element.opening_element.name, ctx);

    // Collect all props into a single object
    let mut props: Vec<ObjectPropertyKind<'a>> = Vec::new();

    // Take attributes out of the opening element
    let mut attrs = ctx.ast.vec();
    std::mem::swap(&mut element.opening_element.attributes, &mut attrs);

    for attr_item in attrs {
        match attr_item {
            JSXAttributeItem::SpreadAttribute(spread) => {
                props.push(
                    ctx.ast
                        .object_property_kind_spread_property(SPAN, spread.unbox().argument),
                );
            }
            JSXAttributeItem::Attribute(attr) => {
                let attr = attr.unbox();
                let attr_name = match &attr.name {
                    JSXAttributeName::Identifier(ident) => ident.name.as_str().to_string(),
                    JSXAttributeName::NamespacedName(ns) => {
                        format!("{}:{}", ns.namespace.name, ns.name.name)
                    }
                };

                let value = if let Some(val) = attr.value {
                    jsx_attr_value_to_expression(
                        val,
                        tracker,
                        ctx,
                        destructured_props,
                        module_imports,
                        hoisted_stmts,
                        loop_depth,
                        iteration_vars,
                        props_param_name,
                        key_prefix,
                    )
                } else {
                    // Boolean attribute: <input disabled /> -> disabled: true
                    ctx.ast.expression_boolean_literal(SPAN, true)
                };

                // Use string literal key for names with special chars
                let key = if attr_name.contains(':') || attr_name.contains('-') || attr_name.contains('$') {
                    let atom = ctx.ast.atom(&attr_name);
                    PropertyKey::from(ctx.ast.expression_string_literal(SPAN, atom, None))
                } else {
                    ctx.ast
                        .property_key_static_identifier(SPAN, ctx.ast.atom(&attr_name))
                };

                props.push(ctx.ast.object_property_kind_object_property(
                    SPAN,
                    PropertyKind::Init,
                    key,
                    value,
                    false,
                    false,
                    false,
                ));
            }
        }
    }

    // Process children: add as `children` prop if present (custom source never text-only)
    let (children_expr, _children_count, _children_mutable) = transform_jsx_children(
        &mut element.children,
        tracker,
        ctx,
        destructured_props,
        module_imports,
        hoisted_stmts,
        loop_depth,
        iteration_vars,
        props_param_name,
        key_prefix,
        false,
    );

    if let Some(children) = children_expr {
        let key = ctx
            .ast
            .property_key_static_identifier(SPAN, ctx.ast.atom("children"));
        props.push(ctx.ast.object_property_kind_object_property(
            SPAN,
            PropertyKind::Init,
            key,
            children,
            false,
            false,
            false,
        ));
    }

    // Build: _jsx(tag, {props}) or _jsx(tag) if no props
    let callee = ctx.ast.expression_identifier(SPAN, "_jsx");

    if props.is_empty() {
        // _jsx(tag) -- no props
        let mut arguments = ctx.ast.vec_with_capacity(1);
        arguments.push(Argument::from(tag));
        ctx.ast.expression_call_with_pure(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            arguments,
            false,
            true,
        )
    } else {
        // _jsx(tag, {props})
        let mut props_vec = ctx.ast.vec_with_capacity(props.len());
        for prop in props {
            props_vec.push(prop);
        }
        let props_obj = ctx.ast.expression_object(SPAN, props_vec);

        let mut arguments = ctx.ast.vec_with_capacity(2);
        arguments.push(Argument::from(tag));
        arguments.push(Argument::from(props_obj));
        ctx.ast.expression_call_with_pure(
            SPAN,
            callee,
            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
            arguments,
            false,
            true,
        )
    }
}
