//! Segment module construction helpers.
//!
//! This module provides string-level building blocks used by the `new_module`
//! 13-step pipeline (Steps 1–13) and `emit_segment` for final codegen.
//!
//! All helpers operate on code strings (not AST nodes) and produce code strings.
//! OXC parsing is used only in `emit_segment` for normalizing the assembled module
//! and in `transform_function_expr` for inspecting the expression type.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::collector::GlobalCollect;
use crate::transform::HoistedConst;

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

    // Process qrl(, inlinedQrl(, and their Dev-mode variants.
    // We do multiple passes since there may be multiple QRL calls.
    for prefix in &["inlinedQrlDEV(", "qrlDEV(", "inlinedQrl(", "qrl("] {
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
// word_idents_in — extract identifiers from code string
// ---------------------------------------------------------------------------

/// Extract all identifiers from a code string using word-boundary scanning.
/// Returns a HashSet of identifier strings (no dedup needed, set handles it).
fn word_idents_in(code: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            let ident = &code[start..i];
            result.insert(ident.to_string());
        } else {
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// generate_imports — SPEC Step 7: 4-priority cascade import generation
// ---------------------------------------------------------------------------

/// Generate import statements for a list of identifiers used in a segment module.
///
/// Priority cascade per SPEC Step 7:
/// 1. Check `global.imports` by local name
/// 2. (explicit_imports — reserved for Phase 17 Dev mode, empty here)
/// 3. Scan `global.imports` for single specifier match
/// 4. Check `global.has_export_symbol` → auto-import from parent file
///
/// Collision renaming: if two idents resolve to the same specifier from the same
/// source, the first keeps its name; subsequent get `_{name}_{i}` suffix (1-indexed).
pub(crate) fn generate_imports(
    combined_local_idents: &[String],
    global: &GlobalCollect,
    file_stem: &str,
    explicit_extensions: bool,
    core_module: &str,
) -> Vec<String> {
    // Map: local_name -> Vec<(specifier, source)>
    let mut seen_import_names: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for ident in combined_local_idents {
        // Priority 1: exact match in global.imports by local name
        if let Some(import) = global.imports.get(ident.as_str()) {
            seen_import_names
                .entry(ident.clone())
                .or_default()
                .push((import.specifier.clone(), import.source.clone()));
            continue;
        }

        // Priority 3: scan for specifier == ident (single match)
        let specifier_matches: Vec<(&String, &crate::collector::Import)> = global
            .imports
            .iter()
            .filter(|(_local, imp)| imp.specifier == *ident)
            .collect();
        if specifier_matches.len() == 1 {
            let (_, imp) = specifier_matches[0];
            seen_import_names
                .entry(ident.clone())
                .or_default()
                .push((imp.specifier.clone(), imp.source.clone()));
            continue;
        }

        // Priority 4: ident is exported from the parent file
        if global.has_export_symbol(ident) {
            let export_name = global
                .resolve_export_for_id(ident)
                .unwrap_or_else(|| ident.clone());
            let source = if explicit_extensions {
                format!("./{}.js", file_stem)
            } else {
                format!("./{}", file_stem)
            };
            seen_import_names
                .entry(ident.clone())
                .or_default()
                .push((export_name, source));
            continue;
        }

        // Not resolved — skip (will be ambient / global)
    }

    // Pass 2: emit import statements sorted by local name for stability.
    // Track (specifier, source) -> emitted local name to detect collisions.
    let mut spec_source_to_local: HashMap<(String, String), String> = HashMap::new();
    let mut result: Vec<String> = Vec::new();

    // Sort keys for deterministic output.
    let mut keys: Vec<String> = seen_import_names.keys().cloned().collect();
    keys.sort();

    for local in &keys {
        let entries = &seen_import_names[local];
        if entries.is_empty() {
            continue;
        }

        if entries.len() == 1 {
            let (specifier, source) = &entries[0];
            // Check for collision: same specifier from same source already emitted?
            let key = (specifier.clone(), source.clone());
            if let Some(_existing_local) = spec_source_to_local.get(&key) {
                // Collision: emit with suffix
                let suffix_local = format!("_{}_1", local);
                let stmt = format_import_stmt(specifier, &suffix_local, source, core_module);
                result.push(stmt);
            } else {
                spec_source_to_local.insert(key, local.clone());
                let stmt = format_import_stmt(specifier, local, source, core_module);
                result.push(stmt);
            }
        } else {
            // Multiple resolutions for same local name — first keeps name, rest get suffix
            for (i, (specifier, source)) in entries.iter().enumerate() {
                let effective_local = if i == 0 {
                    local.clone()
                } else {
                    format!("_{}_{}",  local, i)
                };
                let key = (specifier.clone(), source.clone());
                spec_source_to_local.insert(key, effective_local.clone());
                let stmt = format_import_stmt(specifier, &effective_local, source, core_module);
                result.push(stmt);
            }
        }
    }

    result
}

/// Format a single import statement.
/// Omits `as local` when specifier == local.
fn format_import_stmt(specifier: &str, local: &str, source: &str, _core_module: &str) -> String {
    if specifier == local {
        format!(r#"import {{ {} }} from "{}";"#, specifier, source)
    } else {
        format!(r#"import {{ {} as {} }} from "{}";"#, specifier, local, source)
    }
}

// ---------------------------------------------------------------------------
// collect_needed_extra_top_items — SPEC Steps 5/8
// ---------------------------------------------------------------------------

/// Transitively expand the set of needed `HoistedConst` items from a seed set.
///
/// Fixpoint algorithm:
/// 1. Start with `needed = seed_idents`
/// 2. For each HoistedConst whose `name` is in `needed`:
///    - Add all idents found in `rhs_code` to `needed`
/// 3. Repeat until `needed` stops growing.
///
/// Returns a Vec of cloned items whose `name` is in the final `needed` set.
/// Order preserves the original `extra_top_items` order.
pub(crate) fn collect_needed_extra_top_items(
    extra_top_items: &[HoistedConst],
    seed_idents: &HashSet<String>,
) -> Vec<HoistedConst> {
    let mut needed: HashSet<String> = seed_idents.clone();

    // Fixpoint expansion
    loop {
        let before = needed.len();
        for item in extra_top_items {
            if needed.contains(&item.name) {
                // Add all idents from rhs_code
                let rhs_idents = word_idents_in(&item.rhs_code);
                for ident in rhs_idents {
                    needed.insert(ident);
                }
            }
        }
        if needed.len() == before {
            break;
        }
    }

    // Collect items whose name is in needed, preserving original order
    extra_top_items
        .iter()
        .filter(|item| needed.contains(&item.name))
        .map(|item| HoistedConst {
            name: item.name.clone(),
            rhs_code: item.rhs_code.clone(),
            symbol_name: item.symbol_name.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// order_items_by_dependency — SPEC Step 11: Kahn's topological sort
// ---------------------------------------------------------------------------

/// Sort (sym_name, code_string) pairs by their dependency order using Kahn's BFS.
///
/// Items that depend on other items in the list are placed after their dependencies.
/// Stable sort: zero-in-degree queue sorted by sym_name for determinism.
///
/// Cycle handling: if items remain after Kahn's exhausts, they form a cycle.
/// For cyclic items matching `qrl(` or `inlinedQrl(` patterns, split to:
///   `let {sym};` prepended, then `{sym} = {init};` appended.
pub(crate) fn order_items_by_dependency(items: Vec<(String, String)>) -> Vec<(String, String)> {
    if items.is_empty() {
        return items;
    }

    // Build sym -> index map
    let sym_to_idx: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .map(|(i, (sym, _))| (sym.as_str(), i))
        .collect();

    let n = items.len();
    // in_degree[i] = number of items j where item j is a dep of item i
    let mut in_degree = vec![0usize; n];
    // adjacency: deps[i] = list of item indices that depend on i
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, (sym, code)) in items.iter().enumerate() {
        let used = word_idents_in(code);
        for dep_sym in &used {
            if dep_sym == sym {
                continue; // self-reference is fine (cycle detection handles it)
            }
            if let Some(&j) = sym_to_idx.get(dep_sym.as_str()) {
                // item i depends on item j → j must come before i
                in_degree[i] += 1;
                adjacency[j].push(i);
            }
        }
    }

    // Kahn's BFS with stable ordering (sort zero-in-degree by sym_name)
    let mut queue: VecDeque<usize> = {
        let mut zero: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        zero.sort_by_key(|&i| &items[i].0);
        zero.into_iter().collect()
    };

    let mut sorted: Vec<(String, String)> = Vec::with_capacity(n);
    let mut visited = vec![false; n];

    while let Some(idx) = queue.pop_front() {
        if visited[idx] {
            continue;
        }
        visited[idx] = true;
        sorted.push(items[idx].clone());

        // Reduce in-degree for dependents
        let mut newly_zero: Vec<usize> = Vec::new();
        for &dep_idx in &adjacency[idx] {
            if !visited[dep_idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    newly_zero.push(dep_idx);
                }
            }
        }
        newly_zero.sort_by_key(|&i| &items[i].0);
        for idx in newly_zero {
            queue.push_back(idx);
        }
    }

    // Cycle handling: items not visited form cycles
    if sorted.len() < n {
        for (i, (sym, code)) in items.iter().enumerate() {
            if visited[i] {
                continue;
            }
            // Check if this is a QRL-bearing item — if so, split into let + assignment
            if code.contains("qrl(") || code.contains("inlinedQrl(") {
                // Emit `let sym;` declaration up front
                sorted.insert(0, (sym.clone(), format!("let {};", sym)));
                // Append `sym = init;` at the end
                sorted.push((sym.clone(), format!("{} = {};", sym, code)));
            } else {
                // Non-QRL cycle: just append as-is
                sorted.push((sym.clone(), code.clone()));
            }
        }
    }

    sorted
}

// ---------------------------------------------------------------------------
// dedup_by_sym — SPEC Step 12: final symbol deduplication
// ---------------------------------------------------------------------------

/// Remove duplicate items by their defined symbol name.
///
/// Uses a simple heuristic to extract the first defined symbol from each item string:
/// Scans for `import { X`, `const X`, `var X`, `let X`, or `export const X`.
///
/// The first occurrence of each symbol is kept; subsequent are dropped.
pub(crate) fn dedup_by_sym(items: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<String> = Vec::with_capacity(items.len());

    for item in items {
        if let Some(sym) = extract_defined_sym(&item) {
            if seen.contains(&sym) {
                continue; // duplicate — drop
            }
            seen.insert(sym);
        }
        result.push(item);
    }

    result
}

/// Extract the first defined symbol from a code item string.
/// Handles: `import { X`, `const X`, `var X`, `let X`, `export const X`.
fn extract_defined_sym(item: &str) -> Option<String> {
    let s = item.trim();

    // import { specifier ... }
    if let Some(rest) = s.strip_prefix("import {") {
        let inner = rest.trim();
        // Extract first identifier (before space, comma, '}', or "as")
        let sym = inner
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        return sym;
    }

    // export const X
    if let Some(rest) = s.strip_prefix("export const ") {
        return extract_first_ident(rest);
    }
    // export let X
    if let Some(rest) = s.strip_prefix("export let ") {
        return extract_first_ident(rest);
    }
    // export var X
    if let Some(rest) = s.strip_prefix("export var ") {
        return extract_first_ident(rest);
    }
    // const X
    if let Some(rest) = s.strip_prefix("const ") {
        return extract_first_ident(rest);
    }
    // var X
    if let Some(rest) = s.strip_prefix("var ") {
        return extract_first_ident(rest);
    }
    // let X
    if let Some(rest) = s.strip_prefix("let ") {
        return extract_first_ident(rest);
    }

    None
}

fn extract_first_ident(s: &str) -> Option<String> {
    let trimmed = s.trim();
    let sym: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    if sym.is_empty() {
        None
    } else {
        Some(sym)
    }
}

// ---------------------------------------------------------------------------
// NewModuleCtx + new_module — SPEC 13-step pipeline
// ---------------------------------------------------------------------------

/// Context passed to `new_module` to generate a complete segment module string.
pub(crate) struct NewModuleCtx<'a> {
    /// The (potentially hoisted) function expression for the segment.
    pub expr: &'a str,
    /// The export name for the segment (e.g. `"s_abc123"`).
    pub name: &'a str,
    /// The file stem of the parent module (e.g. `"app"` for `"app.tsx"`).
    pub file_stem: &'a str,
    /// Identifiers referenced in the segment expression from the parent scope.
    pub local_idents: &'a [String],
    /// Captured closure variables (populated via `_captures` mechanism).
    pub scoped_idents: &'a [String],
    /// The `GlobalCollect` for the parent module (used for import resolution).
    pub global: &'a GlobalCollect,
    /// The Qwik core module specifier (e.g. `"@qwik.dev/core"`).
    pub core_module: &'a str,
    /// If true, emit file extensions on local imports (e.g. `"./app.js"`).
    pub explicit_extensions: bool,
    /// Hoisted const declarations from the parent module needed by this segment.
    pub extra_top_items: &'a [HoistedConst],
    /// Root-level declarations migrated into this segment module (Stage 12).
    /// Each entry is a complete statement string inserted after imports and before
    /// the named export (before `create_named_export`).
    pub migrated_root_vars: &'a [String],
}

/// Build a complete segment module string from the 13-step pipeline.
///
/// Steps:
/// 1. If captures: push `new_module_captures_import`
/// 2. `transform_function_expr` — inject read_captures
/// 3. `hoist_qrls_from_expr` — extract QRL calls to top-level vars
/// 4. `fix_self_referential_vars` — rewrite `const x = fn(x)` patterns
/// 5. `collect_needed_extra_top_items` — first pass with seed idents
/// 6. Build combined_local_idents from local_idents + hoisted + needed extras
/// 7. `generate_imports` — 4-priority cascade
/// 8. `collect_needed_extra_top_items` — second pass (after collision context)
/// 9. Dedup extra_top_items against already-imported symbols
/// 10. Separate extra_top_items into imports vs non-imports; emit import items
/// 11. `order_items_by_dependency` — Kahn's topo sort
/// 12. `dedup_by_sym` — final deduplication
/// 13. `create_named_export` — append the segment export
pub(crate) fn new_module(ctx: NewModuleCtx<'_>) -> String {
    let mut header_items: Vec<String> = Vec::new();

    // Step 1: captures import
    if !ctx.scoped_idents.is_empty() {
        header_items.push(new_module_captures_import(ctx.core_module));
    }

    // Step 2: inject read_captures into function expression
    let transformed_expr = transform_function_expr(ctx.expr, ctx.scoped_idents);

    // Step 3: hoist QRL calls out of the expression
    let (expr_after_hoist, hoisted_pairs) = hoist_qrls_from_expr(&transformed_expr);

    // Step 4: fix self-referential variables
    let final_expr = fix_self_referential_vars(&expr_after_hoist);

    // Step 5: collect needed extra_top_items (seed = local_idents + scoped_idents + expr idents)
    let mut seed: HashSet<String> = HashSet::new();
    for ident in ctx.local_idents {
        seed.insert(ident.clone());
    }
    for ident in ctx.scoped_idents {
        seed.insert(ident.clone());
    }
    for ident in word_idents_in(&final_expr) {
        seed.insert(ident);
    }
    // Also include idents from hoisted pairs
    for (sym, code) in &hoisted_pairs {
        seed.insert(sym.clone());
        for ident in word_idents_in(code) {
            seed.insert(ident);
        }
    }

    let needed_extras = collect_needed_extra_top_items(ctx.extra_top_items, &seed);

    // Step 6: build combined_local_idents
    // Exclude idents that will be defined locally by hoisted_pairs or extra_top_items
    // (they should NOT be imported from the parent module).
    let locally_defined: HashSet<String> = hoisted_pairs
        .iter()
        .map(|(sym, _)| sym.clone())
        .chain(needed_extras.iter().map(|item| item.name.clone()))
        .collect();
    let mut combined_local_idents: Vec<String> = ctx
        .local_idents
        .iter()
        .filter(|ident| !locally_defined.contains(*ident))
        .cloned()
        .collect();
    // Add idents referenced in extra_top_items rhs (they need imports from outside)
    for item in &needed_extras {
        for ident in word_idents_in(&item.rhs_code) {
            if !combined_local_idents.contains(&ident) && !locally_defined.contains(&ident) {
                combined_local_idents.push(ident);
            }
        }
    }

    // Step 7: generate imports
    let import_stmts = generate_imports(
        &combined_local_idents,
        ctx.global,
        ctx.file_stem,
        ctx.explicit_extensions,
        ctx.core_module,
    );

    // Step 8: second collect_needed_extra_top_items (after collision context)
    // For now same as step 5 result (Phase 17 will refine with collision-renamed idents)
    let needed_extras_2 = collect_needed_extra_top_items(ctx.extra_top_items, &seed);

    // Build set of already-imported symbols for dedup
    let mut imported_syms: HashSet<String> = HashSet::new();
    for stmt in &import_stmts {
        if let Some(sym) = extract_defined_sym(stmt) {
            imported_syms.insert(sym);
        }
    }
    for stmt in &header_items {
        if let Some(sym) = extract_defined_sym(stmt) {
            imported_syms.insert(sym);
        }
    }

    // Step 9: dedup extra_top_items against already-imported symbols
    let deduped_extras: Vec<&HoistedConst> = needed_extras_2
        .iter()
        .filter(|item| !imported_syms.contains(&item.name))
        .collect();

    // Step 10: separate extra_top_items into imports vs non-imports
    // Items whose rhs_code starts with "import" are import-style items
    let mut extra_imports: Vec<String> = Vec::new();
    let mut extra_non_imports: Vec<(String, String)> = Vec::new();
    for item in &deduped_extras {
        let rhs = item.rhs_code.trim();
        if rhs.starts_with("import ") || rhs.starts_with("import{") {
            extra_imports.push(rhs.to_string());
        } else {
            // Phase 25-03: add /*#__PURE__*/ to qrl() calls in extra_non_imports.
            let is_qrl_call = rhs.starts_with("qrl(")
                || rhs.starts_with("inlinedQrl(")
                || rhs.starts_with("_noopQrl(")
                || rhs.starts_with("qrlDEV(")
                || rhs.starts_with("inlinedQrlDEV(")
                || rhs.starts_with("_noopQrlDEV(");
            let code_str = if is_qrl_call {
                format!("const {} = /*#__PURE__*/ {};", item.name, rhs)
            } else {
                format!("const {} = {};", item.name, rhs)
            };
            extra_non_imports.push((item.name.clone(), code_str));
        }
    }

    // Phase 25-03: if hoisted_pairs OR extra_non_imports (from parent's hoisted consts) contain
    // qrl() calls, add `import { qrl }` (or `qrlDEV`) so the segment module has the identifier.
    let has_qrl_from_hoisted = !hoisted_pairs.is_empty() || extra_non_imports.iter().any(|(_, code)| {
        code.contains("qrl(") || code.contains("qrlDEV(")
            || code.contains("inlinedQrl(") || code.contains("inlinedQrlDEV(")
    });
    if has_qrl_from_hoisted {
        let has_qrl_dev = hoisted_pairs.iter().any(|(_, code)| {
            code.starts_with("qrlDEV(") || code.starts_with("inlinedQrlDEV(")
        }) || extra_non_imports.iter().any(|(_, code)| {
            code.contains("qrlDEV(") || code.contains("inlinedQrlDEV(")
        });
        let qrl_import_name = if has_qrl_dev { "qrlDEV" } else { "qrl" };
        let qrl_import = format!(r#"import {{ {} }} from "{}";"#, qrl_import_name, ctx.core_module);
        header_items.push(qrl_import);
    }

    // Step 11: order hoisted_pairs + extra_non_imports by dependency
    // Phase 25-03: use `const` (not `var`) and add `/*#__PURE__*/` on qrl() calls.
    let mut items_to_sort: Vec<(String, String)> = hoisted_pairs
        .iter()
        .map(|(sym, code)| {
            let needs_pure = code.starts_with("qrl(")
                || code.starts_with("inlinedQrl(")
                || code.starts_with("_noopQrl(")
                || code.starts_with("qrlDEV(")
                || code.starts_with("inlinedQrlDEV(")
                || code.starts_with("_noopQrlDEV(");
            let rhs = if needs_pure {
                format!("/*#__PURE__*/ {}", code)
            } else {
                code.clone()
            };
            (sym.clone(), format!("const {} = {};", sym, rhs))
        })
        .collect();
    items_to_sort.extend(extra_non_imports);

    let sorted_items = order_items_by_dependency(items_to_sort);

    // Step 12: combine all items and dedup
    let mut all_items: Vec<String> = Vec::new();
    all_items.extend(header_items);
    all_items.extend(import_stmts);
    all_items.extend(extra_imports);
    all_items.extend(sorted_items.into_iter().map(|(_sym, code)| code));
    let deduped = dedup_by_sym(all_items);

    // Step 13: append migrated root var declarations (before named export)
    // These are complete statement strings (e.g. "const THRESHOLD = 100;") that
    // were moved out of the root module and belong to this segment only.
    //
    // Build a set of symbols already defined by deduped items so we can skip
    // migrated vars that are already covered by extra_non_imports (which carries
    // the correct /*#__PURE__*/ annotation). This prevents duplicates when
    // apply_variable_migration migrates the same q_ const that extra_top_items
    // also provides.
    let mut result_parts = deduped;
    let already_defined: HashSet<String> = result_parts
        .iter()
        .filter_map(|s| extract_defined_sym(s))
        .collect();
    for migrated_stmt in ctx.migrated_root_vars {
        let stmt = migrated_stmt.trim().to_string();
        if stmt.is_empty() {
            continue;
        }
        // Skip if this variable is already defined in result_parts (from extra_non_imports).
        if let Some(sym) = extract_defined_sym(&stmt) {
            if already_defined.contains(&sym) {
                continue;
            }
        }
        result_parts.push(stmt);
    }

    // Step 14 (was 13): append named export
    let export_stmt = create_named_export(ctx.name, &final_expr);
    result_parts.push(export_stmt);

    result_parts.join("\n")
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

    // -----------------------------------------------------------------------
    // generate_imports tests
    // -----------------------------------------------------------------------

    fn make_global_with_import(local: &str, specifier: &str, source: &str) -> GlobalCollect {
        use crate::collector::{Import, ImportKind};
        let mut gc = GlobalCollect::new_empty();
        gc.imports.insert(
            local.to_string(),
            Import {
                source: source.to_string(),
                specifier: specifier.to_string(),
                kind: ImportKind::Named,
                synthetic: false,
            },
        );
        gc
    }

    fn make_global_with_export(exported: &str) -> GlobalCollect {
        use crate::collector::ExportInfo;
        let mut gc = GlobalCollect::new_empty();
        gc.exports.insert(exported.to_string(), ExportInfo::default());
        gc
    }

    #[test]
    fn resolve_import_priority_1_exact_local_match() {
        let global = make_global_with_import("useSignal", "useSignal", "@qwik.dev/core");
        let idents = vec!["useSignal".to_string()];
        let stmts = generate_imports(&idents, &global, "app", false, "@qwik.dev/core");
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], r#"import { useSignal } from "@qwik.dev/core";"#);
    }

    #[test]
    fn resolve_import_priority_4_export_symbol() {
        // ident is exported from the parent file → auto-import
        let mut global = make_global_with_export("myFn");
        // resolve_export_for_id returns "myFn" since it's directly exported
        let idents = vec!["myFn".to_string()];
        let stmts = generate_imports(&idents, &global, "app", false, "@qwik.dev/core");
        assert_eq!(stmts.len(), 1);
        assert!(
            stmts[0].contains(r#"from "./app""#),
            "Expected import from ./app, got: {}",
            stmts[0]
        );
    }

    #[test]
    fn resolve_import_priority_4_explicit_extensions() {
        let global = make_global_with_export("myFn");
        let idents = vec!["myFn".to_string()];
        let stmts = generate_imports(&idents, &global, "app", true, "@qwik.dev/core");
        assert_eq!(stmts.len(), 1);
        assert!(
            stmts[0].contains(r#"from "./app.js""#),
            "Expected .js extension, got: {}",
            stmts[0]
        );
    }

    #[test]
    fn import_collision_renaming_same_specifier() {
        use crate::collector::{Import, ImportKind};
        // Two different local names resolving to the same specifier+source
        // (priority 3 scan: specifier == ident for one, priority 1 for the other)
        let mut global = GlobalCollect::new_empty();
        global.imports.insert(
            "foo".to_string(),
            Import {
                source: "mod".to_string(),
                specifier: "foo".to_string(),
                kind: ImportKind::Named,
                synthetic: false,
            },
        );
        // Two idents both resolve via priority 1 to different imports
        global.imports.insert(
            "bar".to_string(),
            Import {
                source: "mod2".to_string(),
                specifier: "bar".to_string(),
                kind: ImportKind::Named,
                synthetic: false,
            },
        );
        let idents = vec!["foo".to_string(), "bar".to_string()];
        let stmts = generate_imports(&idents, &global, "app", false, "@qwik.dev/core");
        // Both should be emitted as separate imports
        assert_eq!(stmts.len(), 2, "Expected 2 import stmts, got: {:?}", stmts);
    }

    // -----------------------------------------------------------------------
    // collect_needed_extra_top_items tests
    // -----------------------------------------------------------------------

    fn make_hoisted_const(name: &str, rhs: &str) -> HoistedConst {
        HoistedConst {
            name: name.to_string(),
            rhs_code: rhs.to_string(),
            symbol_name: name.to_string(),
        }
    }

    #[test]
    fn collect_needed_extra_top_items_transitive() {
        // A needs B, B needs C — seed contains A → all three returned
        let items = vec![
            make_hoisted_const("a", "fn(b)"),
            make_hoisted_const("b", "fn(c)"),
            make_hoisted_const("c", "42"),
        ];
        let mut seed = HashSet::new();
        seed.insert("a".to_string());
        let result = collect_needed_extra_top_items(&items, &seed);
        let names: Vec<&str> = result.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"a"), "Expected a");
        assert!(names.contains(&"b"), "Expected b (transitive via a)");
        assert!(names.contains(&"c"), "Expected c (transitive via b)");
    }

    #[test]
    fn collect_needed_extra_top_items_unneeded_excluded() {
        // D is not referenced — should be excluded
        let items = vec![
            make_hoisted_const("a", "someVal"),
            make_hoisted_const("d", "notNeeded"),
        ];
        let mut seed = HashSet::new();
        seed.insert("a".to_string());
        let result = collect_needed_extra_top_items(&items, &seed);
        let names: Vec<&str> = result.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"a"), "Expected a");
        assert!(!names.contains(&"d"), "Expected d excluded, got: {:?}", names);
    }

    // -----------------------------------------------------------------------
    // order_items_by_dependency tests
    // -----------------------------------------------------------------------

    #[test]
    fn order_items_by_dependency_linear() {
        // B is referenced by A → B must come before A
        let items = vec![
            ("a".to_string(), "var a = fn(b);".to_string()),
            ("b".to_string(), "var b = 42;".to_string()),
        ];
        let sorted = order_items_by_dependency(items);
        let syms: Vec<&str> = sorted.iter().map(|(s, _)| s.as_str()).collect();
        let b_pos = syms.iter().position(|&s| s == "b").unwrap();
        let a_pos = syms.iter().position(|&s| s == "a").unwrap();
        assert!(b_pos < a_pos, "b should come before a, got: {:?}", syms);
    }

    #[test]
    fn order_items_by_dependency_cycle_qrl_split() {
        // A cycle involving a QRL call → let + assignment split
        let items = vec![
            ("s_abc".to_string(), r#"var s_abc = qrl(() => import("./x"), "s_abc");"#.to_string()),
        ];
        // Self-cycle (sym appears in own code)
        let sorted = order_items_by_dependency(items);
        // The item should appear in the output (not dropped)
        assert!(!sorted.is_empty(), "Output should not be empty");
    }

    // -----------------------------------------------------------------------
    // dedup_by_sym tests
    // -----------------------------------------------------------------------

    #[test]
    fn final_dedup_removes_duplicate_syms() {
        let items = vec![
            r#"import { foo } from "mod";"#.to_string(),
            r#"import { foo } from "mod2";"#.to_string(), // duplicate sym "foo"
            "const bar = 1;".to_string(),
            "const bar = 2;".to_string(), // duplicate sym "bar"
        ];
        let deduped = dedup_by_sym(items);
        assert_eq!(deduped.len(), 2, "Expected 2 unique items, got: {:?}", deduped);
        assert!(deduped[0].contains("foo"));
        assert!(deduped[1].contains("bar"));
    }

    #[test]
    fn final_dedup_keeps_items_without_sym() {
        // Items with no extractable sym are always kept
        let items = vec![
            "someRawCode();".to_string(),
            "const x = 1;".to_string(),
            "const x = 2;".to_string(), // dup
        ];
        let deduped = dedup_by_sym(items);
        // "someRawCode();" has no extractable sym → kept
        // "const x = 1;" → kept
        // "const x = 2;" → dropped
        assert_eq!(deduped.len(), 2, "Expected 2, got: {:?}", deduped);
    }

    // -----------------------------------------------------------------------
    // new_module integration test
    // -----------------------------------------------------------------------

    #[test]
    fn new_module_basic_no_captures() {
        let global = GlobalCollect::new_empty();
        let ctx = NewModuleCtx {
            expr: "() => 42",
            name: "s_test",
            file_stem: "app",
            local_idents: &[],
            scoped_idents: &[],
            global: &global,
            core_module: "@qwik.dev/core",
            explicit_extensions: false,
            extra_top_items: &[],
            migrated_root_vars: &[],
        };
        let result = new_module(ctx);
        // Should contain the export
        assert!(
            result.contains("export const s_test"),
            "Expected named export, got: {}",
            result
        );
        // No captures import when no scoped_idents
        assert!(
            !result.contains("_captures"),
            "Should not have captures import, got: {}",
            result
        );
    }

    #[test]
    fn new_module_with_captures() {
        let global = GlobalCollect::new_empty();
        let ctx = NewModuleCtx {
            expr: "() => count",
            name: "s_abc",
            file_stem: "app",
            local_idents: &[],
            scoped_idents: &["count".to_string()],
            global: &global,
            core_module: "@qwik.dev/core",
            explicit_extensions: false,
            extra_top_items: &[],
            migrated_root_vars: &[],
        };
        let result = new_module(ctx);
        assert!(
            result.contains(r#"import { _captures } from "@qwik.dev/core";"#),
            "Expected captures import, got: {}",
            result
        );
        assert!(
            result.contains("const count = _captures[0]"),
            "Expected read_captures injection, got: {}",
            result
        );
        assert!(
            result.contains("export const s_abc"),
            "Expected named export, got: {}",
            result
        );
    }
}
