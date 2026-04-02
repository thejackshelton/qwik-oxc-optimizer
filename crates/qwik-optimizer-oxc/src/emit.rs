//! Code generation.
//!
//! Generate JavaScript source code (and optional source maps) from an OXC
//! `Program` AST. Wraps `oxc::codegen::Codegen` with the crate's options
//! (minification, source maps, etc.) and produces `TransformModule` values.

use std::path::PathBuf;

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
    if options.source_maps {
        let codegen_options = oxc::codegen::CodegenOptions {
            source_map_path: Some(PathBuf::from(source_filename)),
            ..Default::default()
        };
        let codegen_result = oxc::codegen::Codegen::new()
            .with_options(codegen_options)
            .with_source_text(source)
            .build(program);

        let map = codegen_result.map.map(|sm| sm.to_json_string());

        EmitResult {
            code: codegen_result.code,
            map,
        }
    } else {
        let codegen_result = oxc::codegen::Codegen::new()
            .with_source_text(source)
            .build(program);

        EmitResult {
            code: codegen_result.code,
            map: None,
        }
    }
}
