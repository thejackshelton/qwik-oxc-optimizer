//! Code generation.
//!
//! Generate JavaScript source code (and optional source maps) from an OXC
//! `Program` AST. Wraps `oxc::codegen::Codegen` with the crate's options
//! (minification, source maps, etc.) and produces `TransformModule` values.

use std::path::PathBuf;

use oxc::codegen::{CodegenOptions, IndentChar};

/// Build CodegenOptions matching SWC's emitter style (2-space indentation).
pub(crate) fn swc_codegen_options(source_map_path: Option<PathBuf>) -> CodegenOptions {
    CodegenOptions {
        source_map_path,
        indent_char: IndentChar::Space,
        indent_width: 2,
        ..Default::default()
    }
}

/// Options controlling code emission.
pub(crate) struct EmitOptions {
    pub source_maps: bool,
}

/// Result of emitting a program to JavaScript source.
pub(crate) struct EmitResult {
    pub code: String,
    pub map: Option<String>,
}

/// Emit a Program AST to JavaScript source code.
///
/// Uses OXC's Codegen to serialize the AST back to JavaScript.
/// When `options.source_maps` is true, generates a v3 source map JSON string
/// via OXC codegen's `source_map_path` option combined with `with_source_text()`.
/// The `source_filename` parameter sets the `"file"` field in the source map JSON.
pub(crate) fn emit_module<'a>(
    program: &oxc::ast::ast::Program<'a>,
    source: &str,
    options: &EmitOptions,
    source_filename: &str,
) -> EmitResult {
    let sm_path = if options.source_maps {
        Some(PathBuf::from(source_filename))
    } else {
        None
    };
    let codegen_result = oxc::codegen::Codegen::new()
        .with_options(swc_codegen_options(sm_path))
        .with_source_text(source)
        .build(program);

    let map = codegen_result.map.map(|sm| sm.to_json_string());
    let code = expand_single_prop_objects(&codegen_result.code);

    EmitResult { code, map }
}

/// Expand single-property object literals from single-line to multi-line format.
///
/// OXC codegen keeps objects with exactly 1 property on a single line:
///   `{ tagName: "my-foo" }`
/// SWC's emitter always expands objects with 1+ properties to multi-line:
///   `{\n  tagName: "my-foo",\n}`
///
/// This post-processing step converts OXC's format to match SWC's.
pub(crate) fn expand_single_prop_objects(code: &str) -> String {
    let mut result = String::with_capacity(code.len() + code.len() / 10);
    let bytes = code.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'{' {
            result.push(bytes[i] as char);
            i += 1;
            continue;
        }

        // Find base indentation from start of current line
        let line_start = code[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let base_indent: &str = {
            let line_prefix = &code[line_start..i];
            let indent_len = line_prefix.len() - line_prefix.trim_start().len();
            &code[line_start..line_start + indent_len]
        };

        // Find matching close brace
        if let Some(close_offset) = find_matching_close_brace(&code[i + 1..]) {
            let content = &code[i + 1..i + 1 + close_offset];
            let content_trimmed = content.trim();

            // Expand if:
            // 1. Single-line (no newlines in content)
            // 2. Non-empty content
            // 3. Looks like object property (has `:` or starts with `...`)
            // 4. Not a block statement (no `;`)
            if !content.contains('\n')
                && !content_trimmed.is_empty()
                && (content_trimmed.contains(':') || content_trimmed.starts_with("..."))
                && !content_trimmed.contains(';')
            {
                let content_with_comma = if content_trimmed.ends_with(',') {
                    content_trimmed.to_string()
                } else {
                    format!("{},", content_trimmed)
                };
                result.push_str("{\n");
                result.push_str(base_indent);
                result.push_str("  ");
                result.push_str(&content_with_comma);
                result.push('\n');
                result.push_str(base_indent);
                result.push('}');

                // Skip past content and closing brace
                i = i + 1 + close_offset + 1;
                continue;
            }
        }

        result.push('{');
        i += 1;
    }

    result
}

/// Find the offset of the matching `}` for an opening `{`.
/// Input `s` starts right after the opening `{`.
fn find_matching_close_brace(s: &str) -> Option<usize> {
    let mut depth: u32 = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}
