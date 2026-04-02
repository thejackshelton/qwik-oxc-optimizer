//! Segment extraction and code generation.
//!
//! After the transform pass identifies segments and serializes their bodies,
//! this module constructs complete JavaScript module source code for each
//! extracted segment. Each segment becomes its own module file containing
//! the extracted function body as an exported const, with any needed imports.

use crate::types::{ImportKind, SegmentData, TransformOptions};

/// Build a segment's JavaScript source code with optional hoisted function declarations.
///
/// Takes the serialized body code and segment metadata, constructs a
/// complete JavaScript module string with:
/// 1. Framework imports (`import { _captures } from "@qwik.dev/core"`)
/// 2. QRL import for nested $-calls (`import { qrl } from "@qwik.dev/core"`)
/// 3. Lazy import declarations (`const i_hash = () => import(...)`)
/// 4. Hoisted function declarations for _fnSignal (`const _hfN = ...`)
/// 5. Capture restoration statements (`const varName = _captures[N]`)
/// 6. Export declaration (`export const name = body`)
///
/// Returns the complete JavaScript module source code.
pub(crate) fn build_segment_code_with_hoisted(
    body_code: &str,
    segment: &SegmentData,
    options: &TransformOptions,
    hoisted_stmts: &[(String, String)],
    custom_jsx_source: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if segment.captures && !segment.capture_names.is_empty() {
        parts.push(format!(
            "import {{ _captures }} from \"{}\";",
            options.core_module
        ));
    }

    if segment.needs_qrl_import {
        parts.push(format!(
            "import {{ qrl }} from \"{}\";",
            options.core_module
        ));
    }

    for (hash, import_path) in &segment.child_lazy_imports {
        parts.push(format!(
            "const i_{} = () => import(\"{}\");",
            hash, import_path
        ));
    }

    // Emit Qrl-suffixed imports needed by this segment (from nested $-calls)
    for qrl_name in &segment.segment_qrl_names {
        parts.push(format!(
            "import {{ {} }} from \"{}\";",
            qrl_name, options.core_module
        ));
    }

    // When custom JSX source is set and body contains _jsx, emit from custom source.
    // Otherwise fall through to _jsxSorted from core module.
    if let Some(jsx_source) = custom_jsx_source {
        if body_code.contains("_jsx") {
            parts.push(format!(
                "import {{ jsx as _jsx }} from \"{}/jsx-runtime\";",
                jsx_source
            ));
        }
    } else if body_code.contains("_jsxSorted") {
        parts.push(format!(
            "import {{ _jsxSorted }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_jsxSplit") {
        parts.push(format!(
            "import {{ _jsxSplit }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_fnSignal") || !hoisted_stmts.is_empty() {
        parts.push(format!(
            "import {{ _fnSignal }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_wrapProp") {
        parts.push(format!(
            "import {{ _wrapProp }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_Fragment") {
        parts.push(format!(
            "import {{ Fragment as _Fragment }} from \"{}/jsx-runtime\";",
            options.core_module
        ));
    }
    if body_code.contains("inlinedQrl") {
        parts.push(format!(
            "import {{ inlinedQrl }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_noopQrl") {
        parts.push(format!(
            "import {{ _noopQrl }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_qrlSync") {
        parts.push(format!(
            "import {{ _qrlSync }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_getVarProps") {
        parts.push(format!(
            "import {{ _getVarProps }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_getConstProps") {
        parts.push(format!(
            "import {{ _getConstProps }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_restProps") {
        parts.push(format!(
            "import {{ _restProps }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_chk") {
        parts.push(format!(
            "import {{ _chk }} from \"{}\";",
            options.core_module
        ));
    }
    if body_code.contains("_val") {
        parts.push(format!(
            "import {{ _val }} from \"{}\";",
            options.core_module
        ));
    }

    // Emit user-code imports needed by this segment body.
    // These are imports from the original module that the segment references
    // (e.g., `import dep3 from "dep3/something"`, `import { bar as bbar } from "../state"`).
    for import_info in &segment.needed_imports {
        for (idx, spec_name) in import_info.specifiers.iter().enumerate() {
            let kind = import_info
                .specifier_kinds
                .get(idx)
                .unwrap_or(&ImportKind::Named);
            match kind {
                ImportKind::Default => {
                    parts.push(format!("import {} from \"{}\";", spec_name, import_info.source));
                }
                ImportKind::Namespace => {
                    parts.push(format!(
                        "import * as {} from \"{}\";",
                        spec_name, import_info.source
                    ));
                }
                ImportKind::Named => {
                    if let Some(imported_name) = import_info.specifier_aliases.get(spec_name) {
                        parts.push(format!(
                            "import {{ {} as {} }} from \"{}\";",
                            imported_name, spec_name, import_info.source
                        ));
                    } else {
                        parts.push(format!(
                            "import {{ {} }} from \"{}\";",
                            spec_name, import_info.source
                        ));
                    }
                }
            }
        }
    }

    for (fn_code, str_code) in hoisted_stmts {
        parts.push(fn_code.clone());
        parts.push(str_code.clone());
    }

    let segment_name = &segment.name;
    if segment.captures && !segment.capture_names.is_empty() {
        let capture_stmts: Vec<String> = segment
            .capture_names
            .iter()
            .enumerate()
            .map(|(i, name)| format!("const {} = _captures[{}]", name, i))
            .collect();

        let modified_body = inject_captures_into_body(body_code, &capture_stmts);
        parts.push(format!("export const {} = {}", segment_name, modified_body));
    } else {
        parts.push(format!("export const {} = {}", segment_name, body_code));
    }

    parts.join("\n")
}

/// Inject capture restoration statements into an arrow function body.
///
/// For block bodies like `() => { ... }`:
///   Insert capture stmts after the opening `{`.
///
/// For expression bodies like `() => expr`:
///   Convert to `() => { const x = _captures[0]; return expr; }`
fn inject_captures_into_body(body_code: &str, capture_stmts: &[String]) -> String {
    if capture_stmts.is_empty() {
        return body_code.to_string();
    }

    let capture_code: String = capture_stmts.iter().map(|s| format!("{};\n", s)).collect();

    if let Some(arrow_pos) = find_arrow_position(body_code) {
        let after_arrow = body_code[arrow_pos + 2..].trim_start();

        if after_arrow.starts_with('{') {
            let brace_offset = body_code[arrow_pos + 2..]
                .find('{')
                .map(|p| arrow_pos + 2 + p);

            if let Some(brace_pos) = brace_offset {
                let before = &body_code[..brace_pos + 1];
                let after = &body_code[brace_pos + 1..];
                return format!("{}\n{}{}", before, capture_code, after);
            }
        }

        let prefix = &body_code[..arrow_pos + 2];
        let expr_body = body_code[arrow_pos + 2..].trim();
        let expr_body = expr_body.strip_suffix(';').unwrap_or(expr_body);
        return format!("{} {{\n{}return {};\n}}", prefix, capture_code, expr_body);
    }

    body_code.to_string()
}

/// Find the position of the `=>` arrow operator in an arrow function string.
/// Skips arrows inside parenthesized parameter lists.
fn find_arrow_position(code: &str) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut paren_depth = 0;
    let mut in_string = false;
    let mut string_char: u8 = 0;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        // Track string literals
        if in_string {
            if b == string_char && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if b == b'\'' || b == b'"' || b == b'`' {
            in_string = true;
            string_char = b;
            i += 1;
            continue;
        }

        // Track parentheses depth
        if b == b'(' {
            paren_depth += 1;
        } else if b == b')' {
            paren_depth -= 1;
        }

        // Look for `=>` at top level (outside parens)
        if b == b'=' && i + 1 < bytes.len() && bytes[i + 1] == b'>' && paren_depth == 0 {
            return Some(i);
        }

        i += 1;
    }

    None
}

/// Emit segment code with optional source map generation.
///
/// Parses the raw segment JavaScript string, runs Codegen with optional
/// `source_map_path`, and returns (normalized_code, optional_map_json).
///
/// Since segment code is string-constructed (not AST-extracted with preserved
/// spans), the source maps produce identity-like mappings (line N maps to
/// line N in the re-parsed code). This is still useful for debugging as it
/// provides column-level mapping within each generated line.
pub(crate) fn emit_segment_with_map(
    raw_code: &str,
    segment_filename: &str,
    source_maps: bool,
) -> (String, Option<String>) {
    let allocator = oxc::allocator::Allocator::default();
    let source_in_arena = allocator.alloc_str(raw_code);
    let source_type = oxc::span::SourceType::mjs();
    let ret = oxc::parser::Parser::new(&allocator, source_in_arena, source_type).parse();
    if ret.panicked || !ret.errors.is_empty() {
        return (raw_code.to_string(), None);
    }

    if source_maps {
        use std::path::PathBuf;
        let codegen_options = oxc::codegen::CodegenOptions {
            source_map_path: Some(PathBuf::from(segment_filename)),
            ..Default::default()
        };
        let codegen_result = oxc::codegen::Codegen::new()
            .with_options(codegen_options)
            .with_source_text(source_in_arena)
            .build(&ret.program);

        let map = codegen_result.map.map(|sm| sm.to_json_string());
        (codegen_result.code, map)
    } else {
        let codegen_result = oxc::codegen::Codegen::new()
            .with_source_text(source_in_arena)
            .build(&ret.program);
        (codegen_result.code, None)
    }
}
