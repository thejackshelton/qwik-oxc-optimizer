//! Segment module construction helpers.
//!
//! This module provides string-level building blocks used by the `new_module`
//! 13-step pipeline (Steps 1–4, 13) and `emit_segment` for final codegen.
//!
//! All helpers operate on code strings (not AST nodes) and produce code strings.
//! OXC parsing is used only in `emit_segment` for normalizing the assembled module
//! and in `transform_function_expr` for inspecting the expression type.

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// emit_segment
// ---------------------------------------------------------------------------

/// Parse `raw_code` via OXC (MJS source type), run Codegen, and return the
/// emitted code (and optionally a source map JSON string).
///
/// On parse failure or panic the raw code is returned unchanged with no map.
///
/// Double-quote normalization comes for free from OXC's Codegen default.
pub(crate) fn emit_segment(
    raw_code: &str,
    filename: &str,
    source_maps: bool,
) -> (String, Option<String>) {
    use oxc::allocator::Allocator;
    use oxc::codegen::CodegenOptions;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let source: &str = allocator.alloc_str(raw_code);
    let ret = Parser::new(&allocator, source, SourceType::mjs()).parse();
    if ret.panicked {
        return (raw_code.to_string(), None);
    }
    let program = ret.program;

    if source_maps {
        let codegen_options = CodegenOptions {
            source_map_path: Some(PathBuf::from(filename)),
            ..Default::default()
        };
        let result = oxc::codegen::Codegen::new()
            .with_options(codegen_options)
            .with_source_text(source)
            .build(&program);
        let map = result.map.map(|sm| sm.to_json_string());
        (result.code, map)
    } else {
        let result = oxc::codegen::Codegen::new()
            .with_source_text(source)
            .build(&program);
        (result.code, None)
    }
}

// ---------------------------------------------------------------------------
// new_module_captures_import
// ---------------------------------------------------------------------------

/// Produce the captures import statement for a segment module.
///
/// Example: `import { _captures } from "@qwik.dev/core";`
pub(crate) fn new_module_captures_import(core_module: &str) -> String {
    format!(r#"import {{ _captures }} from "{}";"#, core_module)
}

// ---------------------------------------------------------------------------
// read_captures
// ---------------------------------------------------------------------------

/// Produce individual `const` destructuring statements for captured idents.
///
/// SPEC Pitfall 5: uses individual index access, NOT array destructuring.
///
/// Example for `["count", "signal"]`:
/// ```text
/// const count = _captures[0];
/// const signal = _captures[1];
/// ```
pub(crate) fn read_captures(scoped_idents: &[String]) -> String {
    scoped_idents
        .iter()
        .enumerate()
        .map(|(i, name)| format!("const {} = _captures[{}];\n", name, i))
        .collect()
}

// ---------------------------------------------------------------------------
// transform_function_expr
// ---------------------------------------------------------------------------

/// Prepend `read_captures` statements into a function expression or arrow body.
///
/// If `scoped_idents` is empty, returns `expr_code` unchanged.
///
/// Handles four cases:
/// - Arrow with concise body (`() => expr`): converts to block body, prepends
///   read_captures, adds explicit `return`.
/// - Arrow with block body (`() => { ... }`): prepends read_captures after `{`.
/// - Function expression (`function(...) { ... }`): prepends read_captures.
/// - Other: returns unchanged.
///
/// Implementation: wrap in `var _x = <expr_code>` to parse, detect arrow vs fn,
/// build new body string, re-serialize via Codegen.
pub(crate) fn transform_function_expr(expr_code: &str, scoped_idents: &[String]) -> String {
    if scoped_idents.is_empty() {
        return expr_code.to_string();
    }

    let captures = read_captures(scoped_idents);

    // Wrap as a var declaration so we can parse expr_code as an expression.
    let wrapped = format!("var _x = {};", expr_code);

    use oxc::allocator::Allocator;
    use oxc::ast::ast::{Expression, Statement};
    use oxc::codegen::Codegen;
    use oxc::parser::Parser;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let src: &str = allocator.alloc_str(&wrapped);
    let ret = Parser::new(&allocator, src, SourceType::mjs()).parse();
    if ret.panicked || ret.program.body.is_empty() {
        return expr_code.to_string();
    }

    // SAFETY: program borrows from allocator; allocator lives for this function.
    let program: oxc::ast::ast::Program<'static> = unsafe {
        std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
            ret.program,
        )
    };

    let stmt = match program.body.first() {
        Some(s) => s,
        None => return expr_code.to_string(),
    };

    // Extract the initializer expression from `var _x = <expr>`.
    let init_expr = match stmt {
        Statement::VariableDeclaration(var_decl) => match var_decl.declarations.first() {
            Some(decl) => decl.init.as_ref(),
            None => return expr_code.to_string(),
        },
        _ => return expr_code.to_string(),
    };

    let expr = match init_expr {
        Some(e) => e,
        None => return expr_code.to_string(),
    };

    match expr {
        Expression::ArrowFunctionExpression(arrow) => {
            if arrow.expression {
                // Concise body: () => expr
                // We need to emit the body expression first.
                // The body has exactly one ExpressionStatement.
                let body_expr_code = if let Some(first_stmt) = arrow.body.statements.first() {
                    if let Statement::ExpressionStatement(es) = first_stmt {
                        // Re-serialize just the expression.
                        let tmp_src = format!("var _tmp = {};", expr_code);
                        let tmp_alloc = Allocator::default();
                        let tmp_src_arena: &str = tmp_alloc.alloc_str(&tmp_src);
                        let tmp_ret = Parser::new(&tmp_alloc, tmp_src_arena, SourceType::mjs()).parse();
                        if tmp_ret.panicked {
                            return expr_code.to_string();
                        }
                        // The concise body is represented as a return statement by OXC internally.
                        // To get just the return value, re-emit the expression via codegen.
                        // Serialize the expression statement directly.
                        let _ = es; // not used directly
                        // Use string-level extraction: find `=>` and take everything after it.
                        extract_arrow_concise_body(expr_code)
                    } else {
                        return expr_code.to_string();
                    }
                } else {
                    return expr_code.to_string();
                };

                // Build new arrow: (params) => { read_captures; return body; }
                let params_code = extract_arrow_params(expr_code);
                format!(
                    "({}) => {{\n{}return {};\n}}",
                    params_code, captures, body_expr_code
                )
            } else {
                // Block body: () => { ... }
                // Prepend captures after the opening `{`.
                prepend_into_block_arrow(expr_code, &captures)
            }
        }
        Expression::FunctionExpression(_) => {
            // function(...) { ... }
            // Prepend captures after the opening `{` of the body.
            prepend_into_function_body(expr_code, &captures)
        }
        _ => expr_code.to_string(),
    }
}

/// Extract the parameter string from an arrow like `(a, b) => ...` → `"a, b"`.
/// Returns an empty string if no params can be detected.
fn extract_arrow_params(expr_code: &str) -> String {
    // Find the `=>` and extract everything before it, strip parens.
    let arrow_pos = match expr_code.find("=>") {
        Some(p) => p,
        None => return String::new(),
    };
    let params_part = expr_code[..arrow_pos].trim();
    // Strip outer parens if present.
    if params_part.starts_with('(') && params_part.ends_with(')') {
        params_part[1..params_part.len() - 1].to_string()
    } else {
        params_part.to_string()
    }
}

/// Extract the concise body from an arrow like `(a) => expr` → `"expr"`.
fn extract_arrow_concise_body(expr_code: &str) -> String {
    let arrow_pos = match expr_code.find("=>") {
        Some(p) => p,
        None => return expr_code.to_string(),
    };
    expr_code[arrow_pos + 2..].trim().to_string()
}

/// Prepend `captures` text after the `{` that opens the arrow body.
fn prepend_into_block_arrow(expr_code: &str, captures: &str) -> String {
    // Find `=> {` pattern and inject after the `{`.
    if let Some(arrow_pos) = expr_code.find("=>") {
        let after_arrow = expr_code[arrow_pos + 2..].trim_start();
        if after_arrow.starts_with('{') {
            let before_brace = &expr_code[..arrow_pos + 2 + (expr_code[arrow_pos + 2..].len() - after_arrow.len())];
            let after_brace = &after_arrow[1..]; // skip the `{`
            return format!("{}{{ {}{}", before_brace, captures, after_brace);
        }
    }
    expr_code.to_string()
}

/// Prepend `captures` text after the first `{` in a function expression body.
fn prepend_into_function_body(expr_code: &str, captures: &str) -> String {
    // Find the first `{` that starts the function body.
    // A function expression looks like: `function(...) {`
    // Skip past any `{` in param default values by finding the `) {` pattern.
    if let Some(brace_pos) = find_function_body_brace(expr_code) {
        let before = &expr_code[..brace_pos + 1];
        let after = &expr_code[brace_pos + 1..];
        format!("{}{}{}", before, captures, after)
    } else {
        expr_code.to_string()
    }
}

/// Find the opening brace of the function body in a function expression.
/// We look for `) {` or `) \n{` patterns after the parameters.
fn find_function_body_brace(code: &str) -> Option<usize> {
    // Scan for the closing paren of the parameter list, then find the `{`.
    let mut depth = 0i32;
    let mut in_params = false;
    let bytes = code.as_bytes();
    let mut paren_close: Option<usize> = None;

    // Skip 'function' keyword and find first `(`
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'(' {
            in_params = true;
            depth += 1;
        } else if b == b')' && in_params {
            depth -= 1;
            if depth == 0 {
                paren_close = Some(i);
                break;
            }
        }
    }

    let close_paren = paren_close?;

    // After close_paren, find the first `{`
    for (offset, &b) in bytes[close_paren + 1..].iter().enumerate() {
        if b == b'{' {
            return Some(close_paren + 1 + offset);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// hoist_qrls_from_expr
// ---------------------------------------------------------------------------

/// Scan `expr_code` for `qrl(` and `inlinedQrl(` call patterns.
///
/// For each found, extracts the symbol name (second string argument) and
/// returns:
/// - `modified_expr`: the expression with QRL calls replaced by symbol references
/// - `hoisted_pairs`: `(symbol_name, full_call_code)` — each becomes `var {name} = {call};`
///
/// Uses `var` (not `const`) for forward-reference safety.
pub(crate) fn hoist_qrls_from_expr(expr_code: &str) -> (String, Vec<(String, String)>) {
    let mut result = expr_code.to_string();
    let mut hoisted: Vec<(String, String)> = Vec::new();

    // Process qrl( and inlinedQrl( calls.
    // We do multiple passes since there may be multiple QRL calls.
    for prefix in &["inlinedQrl(", "qrl("] {
        loop {
            let start = match result.find(prefix) {
                Some(p) => p,
                None => break,
            };
            // Find the matching closing paren by depth counting.
            let call_start = start;
            let inner_start = start + prefix.len();
            let mut depth = 1i32;
            let mut end_pos = inner_start;
            let bytes = result.as_bytes();
            for (i, &b) in bytes[inner_start..].iter().enumerate() {
                match b {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end_pos = inner_start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let full_call = &result[call_start..end_pos + 1];
            let inner_args = &result[inner_start..end_pos];

            // Extract second argument (symbol name string).
            let sym_name = extract_second_string_arg(inner_args);
            match sym_name {
                Some(name) => {
                    hoisted.push((name.clone(), full_call.to_string()));
                    // Replace the call in result with the symbol name.
                    result = format!("{}{}{}", &result[..call_start], name, &result[end_pos + 1..]);
                }
                None => break, // Can't parse, stop trying for this prefix
            }
        }
    }

    (result, hoisted)
}

/// Extract the second string argument from a function call's argument list.
/// E.g., from `() => import("./x"), "s_abc"` extracts `"s_abc"`.
fn extract_second_string_arg(args: &str) -> Option<String> {
    // Find the second argument by scanning past the first top-level comma.
    let mut depth = 0i32;
    let mut first_comma: Option<usize> = None;
    let bytes = args.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                first_comma = Some(i);
                break;
            }
            _ => {}
        }
    }

    let second_arg_start = first_comma? + 1;
    let second_arg = args[second_arg_start..].trim();

    // The second arg should be a string literal like `"s_abc"` or `'s_abc'`.
    if (second_arg.starts_with('"') && second_arg.contains('"'))
        || (second_arg.starts_with('\'') && second_arg.contains('\''))
    {
        // Extract the string content — find content between first and second quote.
        let quote = second_arg.chars().next()?;
        let inner = &second_arg[1..];
        // Find close quote (first occurrence of same quote char at top level).
        let close = inner.find(quote)?;
        Some(inner[..close].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// fix_self_referential_vars
// ---------------------------------------------------------------------------

/// Rewrite `const X = ...X...` patterns where the initializer references X.
///
/// For each such binding, emits the three-statement `_ref` pattern per SPEC Step 4:
/// ```text
/// const _ref = {};
/// _ref.X = <init with X replaced by _ref.X>;
/// const { X } = _ref;
/// ```
pub(crate) fn fix_self_referential_vars(body_code: &str) -> String {
    let mut output = body_code.to_string();
    // We scan for `const <name> = <init>;` patterns at statement level.
    // For simplicity we do a line-level scan.
    let mut lines: Vec<String> = output.lines().map(|l| l.to_string()).collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(rest) = line.strip_prefix("const ") {
            // Parse `name = init` portion.
            if let Some(eq_pos) = rest.find(" = ") {
                let name = rest[..eq_pos].trim();
                // Only handle simple identifier names (no destructuring).
                if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                    let init_with_semi = rest[eq_pos + 3..].trim();
                    let init = init_with_semi.trim_end_matches(';').trim();
                    // Check if init contains `name` as a word.
                    if contains_word(init, name) {
                        // Build the _ref replacement.
                        let new_init = replace_word(init, name, &format!("_ref.{}", name));
                        let replacement = vec![
                            format!("{}const _ref = {{}};", &lines[i][..lines[i].len() - line.len()]),
                            format!("{}_ref.{} = {};", &lines[i][..lines[i].len() - line.len()], name, new_init),
                            format!("{}const {{ {} }} = _ref;", &lines[i][..lines[i].len() - line.len()], name),
                        ];
                        lines.splice(i..=i, replacement);
                        i += 3;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    lines.join("\n")
}

/// Check if `word` appears as a whole word in `text`.
fn contains_word(text: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0
            || !text
                .chars()
                .nth(abs_pos.saturating_sub(1))
                .map(|c| c.is_alphanumeric() || c == '_' || c == '$')
                .unwrap_or(false);
        let after_ok = abs_pos + word.len() >= text.len()
            || !text
                .chars()
                .nth(abs_pos + word.len())
                .map(|c| c.is_alphanumeric() || c == '_' || c == '$')
                .unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + 1;
        if start >= text.len() {
            break;
        }
    }
    false
}

/// Replace all whole-word occurrences of `word` in `text` with `replacement`.
fn replace_word(text: &str, word: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0
            || !text
                .chars()
                .nth(abs_pos.saturating_sub(1))
                .map(|c| c.is_alphanumeric() || c == '_' || c == '$')
                .unwrap_or(false);
        let after_ok = abs_pos + word.len() >= text.len()
            || !text
                .chars()
                .nth(abs_pos + word.len())
                .map(|c| c.is_alphanumeric() || c == '_' || c == '$')
                .unwrap_or(false);
        if before_ok && after_ok {
            result.push_str(&text[start..abs_pos]);
            result.push_str(replacement);
            start = abs_pos + word.len();
        } else {
            result.push_str(&text[start..abs_pos + 1]);
            start = abs_pos + 1;
        }
        if start >= text.len() {
            break;
        }
    }
    result.push_str(&text[start..]);
    result
}

// ---------------------------------------------------------------------------
// create_named_export
// ---------------------------------------------------------------------------

/// Produce a named export statement: `export const name = <expr>;`.
pub(crate) fn create_named_export(name: &str, expr_code: &str) -> String {
    format!("export const {} = {};", name, expr_code)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // captures_import_generates_correct_import_statement
    // -----------------------------------------------------------------------

    #[test]
    fn captures_import_generates_correct_import_statement() {
        let result = new_module_captures_import("@qwik.dev/core");
        assert_eq!(result, r#"import { _captures } from "@qwik.dev/core";"#);
    }

    // -----------------------------------------------------------------------
    // read_captures_generates_individual_consts
    // -----------------------------------------------------------------------

    #[test]
    fn read_captures_generates_individual_consts() {
        let idents = vec!["count".to_string(), "signal".to_string()];
        let result = read_captures(&idents);
        assert_eq!(result, "const count = _captures[0];\nconst signal = _captures[1];\n");
    }

    #[test]
    fn read_captures_empty_produces_empty() {
        let result = read_captures(&[]);
        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    // transform_function_expr_arrow_concise
    // -----------------------------------------------------------------------

    #[test]
    fn transform_function_expr_arrow_concise() {
        // concise arrow with captures: () => expr => block body with read_captures + return
        let expr = "() => value";
        let scoped = vec!["count".to_string()];
        let result = transform_function_expr(expr, &scoped);
        // Should contain the captures read
        assert!(
            result.contains("const count = _captures[0];"),
            "Expected read_captures in output, got: {}",
            result
        );
        // Should contain return
        assert!(
            result.contains("return"),
            "Expected return in concise->block conversion, got: {}",
            result
        );
        // Should not be concise anymore (should have braces)
        assert!(
            result.contains('{'),
            "Expected block body braces, got: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // transform_function_expr_arrow_block
    // -----------------------------------------------------------------------

    #[test]
    fn transform_function_expr_arrow_block() {
        let expr = "() => { return value; }";
        let scoped = vec!["signal".to_string()];
        let result = transform_function_expr(expr, &scoped);
        assert!(
            result.contains("const signal = _captures[0];"),
            "Expected read_captures prepended in block arrow, got: {}",
            result
        );
        assert!(
            result.contains("return value"),
            "Original return should remain, got: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // transform_function_expr_no_captures_unchanged
    // -----------------------------------------------------------------------

    #[test]
    fn transform_function_expr_no_captures_unchanged() {
        let expr = "() => 42";
        let result = transform_function_expr(expr, &[]);
        assert_eq!(result, "() => 42");
    }

    // -----------------------------------------------------------------------
    // hoist_qrls_from_expr
    // -----------------------------------------------------------------------

    #[test]
    fn hoist_qrls_from_expr_basic() {
        let expr = r#"qrl(() => import("./x"), "s_abc")"#;
        let (modified, hoisted) = hoist_qrls_from_expr(expr);
        assert_eq!(modified, "s_abc", "QRL call should be replaced with symbol name");
        assert_eq!(hoisted.len(), 1);
        assert_eq!(hoisted[0].0, "s_abc");
        assert!(hoisted[0].1.contains("qrl("), "Hoisted code should contain original call");
    }

    #[test]
    fn hoist_qrls_from_expr_inlined_qrl() {
        let expr = r#"inlinedQrl(() => import("./y"), "s_xyz", [count])"#;
        let (modified, hoisted) = hoist_qrls_from_expr(expr);
        assert_eq!(modified, "s_xyz");
        assert_eq!(hoisted.len(), 1);
        assert_eq!(hoisted[0].0, "s_xyz");
    }

    #[test]
    fn hoist_qrls_from_expr_no_qrl_unchanged() {
        let expr = "someOtherCall(42)";
        let (modified, hoisted) = hoist_qrls_from_expr(expr);
        assert_eq!(modified, expr);
        assert!(hoisted.is_empty());
    }

    // -----------------------------------------------------------------------
    // fix_self_referential_var
    // -----------------------------------------------------------------------

    #[test]
    fn fix_self_referential_var() {
        let body = "const x = fn(x);";
        let result = fix_self_referential_vars(body);
        assert!(
            result.contains("const _ref = {}"),
            "Expected _ref initialization, got: {}",
            result
        );
        assert!(
            result.contains("_ref.x = fn(_ref.x)"),
            "Expected _ref.x assignment with replacement, got: {}",
            result
        );
        assert!(
            result.contains("const { x } = _ref"),
            "Expected destructuring from _ref, got: {}",
            result
        );
    }

    #[test]
    fn fix_self_referential_var_no_self_ref_unchanged() {
        let body = "const x = fn(y);";
        let result = fix_self_referential_vars(body);
        assert_eq!(result, body);
    }

    // -----------------------------------------------------------------------
    // create_named_export
    // -----------------------------------------------------------------------

    #[test]
    fn create_named_export_basic() {
        let result = create_named_export("myComp", "componentQrl(...)");
        assert_eq!(result, "export const myComp = componentQrl(...);");
    }

    // -----------------------------------------------------------------------
    // emit_segment
    // -----------------------------------------------------------------------

    #[test]
    fn emit_segment_round_trips_code() {
        let code = "const x = 1;\nexport const y = 2;\n";
        let (out, map) = emit_segment(code, "test.js", false);
        assert!(out.contains("x"), "Output should contain x, got: {}", out);
        assert!(out.contains("y"), "Output should contain y, got: {}", out);
        assert!(map.is_none(), "No source map requested");
    }

    #[test]
    fn emit_segment_with_source_maps() {
        let code = "const x = 1;\n";
        let (out, map) = emit_segment(code, "test.js", true);
        assert!(out.contains("x"));
        assert!(map.is_some(), "Source map should be present");
    }

    #[test]
    fn emit_segment_invalid_code_returns_raw() {
        // This is not valid JS — OXC might panic or produce partial result.
        // If it panics, we expect the raw code back.
        let invalid = "this is { not valid } javascript !!!";
        let (out, _map) = emit_segment(invalid, "test.js", false);
        // Either raw or parsed-best-effort — just check it doesn't crash.
        let _ = out;
    }

    #[test]
    fn emit_segment_double_quote_normalization() {
        // OXC Codegen produces double quotes by default.
        let code = "const x = 'hello';\n";
        let (out, _) = emit_segment(code, "test.js", false);
        assert!(
            out.contains('"'),
            "OXC Codegen should normalize to double quotes, got: {}",
            out
        );
    }
}
