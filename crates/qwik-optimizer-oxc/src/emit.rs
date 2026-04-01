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
///
/// OXC Codegen produces double-quoted strings by default (matches qwik-core SWC output).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_with_source_maps() {
        let source = "const x = 1;\nconst y = x + 2;\n";
        let allocator = oxc::allocator::Allocator::default();
        let source_in_arena = allocator.alloc_str(source);
        let source_type = oxc::span::SourceType::mjs();
        let ret = oxc::parser::Parser::new(&allocator, source_in_arena, source_type).parse();
        assert!(!ret.panicked, "Parse should not panic");

        let options = EmitOptions { source_maps: true };

        let result = emit_module(&ret.program, source_in_arena, &options, "test.js");

        assert!(
            result.code.contains("const x = 1"),
            "Expected code output: {}",
            result.code
        );

        assert!(result.map.is_some(), "Expected source map to be Some");
        let map_json = result.map.unwrap();
        assert!(
            map_json.contains("\"version\""),
            "Expected version field in source map: {}",
            map_json
        );
        assert!(
            map_json.contains("\"mappings\""),
            "Expected mappings field in source map: {}",
            map_json
        );
        assert!(
            map_json.contains("\"sources\""),
            "Expected sources field in source map: {}",
            map_json
        );
    }

    #[test]
    fn test_emit_without_source_maps() {
        let source = "const x = 1;\n";
        let allocator = oxc::allocator::Allocator::default();
        let source_in_arena = allocator.alloc_str(source);
        let source_type = oxc::span::SourceType::mjs();
        let ret = oxc::parser::Parser::new(&allocator, source_in_arena, source_type).parse();
        assert!(!ret.panicked, "Parse should not panic");

        let options = EmitOptions { source_maps: false };

        let result = emit_module(&ret.program, source_in_arena, &options, "test.js");

        assert!(
            result.code.contains("const x = 1"),
            "Expected code output: {}",
            result.code
        );

        assert!(
            result.map.is_none(),
            "Expected source map to be None when source_maps is false"
        );
    }

    #[test]
    fn test_emit_double_quoted_strings() {
        // OXC Codegen emits double-quoted strings by default (matches qwik-core SWC output)
        let source = "const x = 'hello';\n";
        let allocator = oxc::allocator::Allocator::default();
        let source_in_arena = allocator.alloc_str(source);
        let source_type = oxc::span::SourceType::mjs();
        let ret = oxc::parser::Parser::new(&allocator, source_in_arena, source_type).parse();
        assert!(!ret.panicked, "Parse should not panic");

        let options = EmitOptions { source_maps: false };
        let result = emit_module(&ret.program, source_in_arena, &options, "test.js");

        assert!(
            result.code.contains("\"hello\""),
            "Expected double-quoted string in output: {}",
            result.code
        );
    }
}
