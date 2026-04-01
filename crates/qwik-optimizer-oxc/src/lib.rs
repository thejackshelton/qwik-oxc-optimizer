//! Qwik optimizer using OXC for code transformation.
//!
//! This crate provides the type definitions and utility functions needed for
//! the Qwik $-call extraction pipeline. Full transform logic is added in
//! subsequent phases.

pub mod hash;

mod collector;
mod const_replace;
mod props_destructuring;
mod errors;
pub(crate) mod emit;
mod filter_exports;
mod is_const;
pub(crate) mod parse;
mod rename_imports;
mod types;
mod words;

pub(crate) mod entry_strategy;

// Re-export all public types
pub use types::{
    CtxKind,
    Diagnostic,
    DiagnosticCategory,
    EmitMode,
    EntryStrategy,
    MinifyMode,
    QwikBundle,
    QwikManifest,
    SegmentAnalysis,
    SourceLocation,
    TransformModule,
    TransformModuleInput,
    TransformModulesOptions,
    TransformOutput,
};

use std::path::Path;

use path_slash::PathBufExt as _;

use emit::EmitOptions;
use types::TransformCodeOptions;

// ---------------------------------------------------------------------------
// TransformOutput::append
// ---------------------------------------------------------------------------

impl TransformOutput {
    /// Merge `other` into `self`.
    ///
    /// - Appends all modules and diagnostics from `other`.
    /// - `is_type_script` and `is_jsx` are logical OR (true if either input has the flag).
    ///
    /// SPEC lines 3671–3677.
    pub fn append(&mut self, other: &mut TransformOutput) {
        self.modules.append(&mut other.modules);
        self.diagnostics.append(&mut other.diagnostics);
        self.is_type_script = self.is_type_script || other.is_type_script;
        self.is_jsx = self.is_jsx || other.is_jsx;
    }
}

// ---------------------------------------------------------------------------
// transform_code — per-file identity pipeline (Stages 0–1 only for Phase 8)
// ---------------------------------------------------------------------------

fn transform_code(
    config: &TransformCodeOptions,
    input_code: &str,
    input_path: &str,
    _dev_path: Option<&str>,
) -> Result<TransformOutput, anyhow::Error> {
    // Stage 0: Decompose path.
    let path_data = parse::parse_path(input_path, Path::new(&config.src_dir))?;

    // Stage 1: Allocate arena and parse source.
    let allocator = oxc::allocator::Allocator::default();
    let source_in_arena: &str = allocator.alloc_str(input_code);

    let (is_type_script, is_jsx, mut program, diagnostics) =
        match parse::parse_module(&allocator, source_in_arena, input_path) {
            Ok((parse_result, diags)) => {
                let is_ts = parse_result.source_type.is_typescript();
                let is_jsx = parse_result.source_type.is_jsx();
                (is_ts, is_jsx, parse_result.program, diags)
            }
            Err(diags) => {
                // Unrecoverable parse error: return empty output with diagnostics.
                return Ok(TransformOutput {
                    modules: vec![],
                    diagnostics: diags,
                    is_type_script: false,
                    is_jsx: false,
                });
            }
        };

    // Stage 2: Strip exports (conditional).
    if !config.strip_exports.is_empty() {
        filter_exports::filter_exports(&mut program, &config.strip_exports, &allocator);
    }

    // Stage 5: Import rename (always).
    rename_imports::rename_imports(&mut program, &allocator);

    // Stage 7: Global collect (always).
    let mut collect = collector::global_collect(&program);

    // Stage 8: Props destructuring (always, all modes).
    props_destructuring::transform_props_destructuring(
        &mut program, &mut collect, &config.core_module, &allocator,
    );

    // Stage 9: Const replacement (denylist: skip Lib and Test modes).
    const_replace::replace_build_constants(&mut program, config, &collect, &allocator);

    // Stages 10–13: No-op until future phases.

    // did_transform remains false: Stages 3/4 (TS strip, JSX transpile) are still no-ops.
    // When those stages are active, this flag will be set true and preserve_filenames logic applies.
    let did_transform = false;

    // Emit: codegen the transformed AST back to JavaScript.
    let emit_result = emit::emit_module(
        &program,
        source_in_arena,
        &EmitOptions {
            source_maps: config.source_maps,
        },
        input_path,
    );

    // Compute output path.
    // When did_transform is true and preserve_filenames is false, the extension
    // is replaced via output_extension(). Otherwise the original filename is kept.
    let output_path = if did_transform && !config.preserve_filenames {
        let ext = parse::output_extension(input_path, config.transpile_ts, config.transpile_jsx);
        let stem = &path_data.file_stem;
        let new_filename = format!("{stem}.{ext}");
        if path_data.rel_dir == std::path::PathBuf::new() {
            new_filename
        } else {
            path_data
                .rel_dir
                .join(new_filename)
                .to_slash_lossy()
                .into_owned()
        }
    } else {
        // did_transform=false: keep original filename (Phase 8).
        if path_data.rel_dir == std::path::PathBuf::new() {
            path_data.file_name.clone()
        } else {
            path_data
                .rel_dir
                .join(&path_data.file_name)
                .to_slash_lossy()
                .into_owned()
        }
    };

    let module = TransformModule {
        path: output_path,
        is_entry: false,
        code: emit_result.code,
        map: emit_result.map,
        segment: None,
        orig_path: Some(input_path.to_string()),
        order: 0,
    };

    Ok(TransformOutput {
        modules: vec![module],
        diagnostics,
        is_type_script,
        is_jsx,
    })
}

// ---------------------------------------------------------------------------
// transform_modules — public entry point
// ---------------------------------------------------------------------------

/// Transform one or more input modules.
///
/// Iterates all inputs, calls `transform_code` per file, merges results via
/// `TransformOutput::append`, and sorts output modules by their `order` field.
///
/// `is_server` defaults to `true` when `None` per SPEC line 259.
pub fn transform_modules(config: TransformModulesOptions) -> Result<TransformOutput, anyhow::Error> {
    let code_config = TransformCodeOptions {
        src_dir: config.src_dir.clone(),
        root_dir: config.root_dir.clone(),
        source_maps: config.source_maps,
        minify: config.minify.clone(),
        transpile_ts: config.transpile_ts,
        transpile_jsx: config.transpile_jsx,
        preserve_filenames: config.preserve_filenames,
        entry_strategy: config.entry_strategy.clone(),
        explicit_extensions: config.explicit_extensions,
        mode: config.mode.clone(),
        scope: config.scope.clone(),
        core_module: config
            .core_module
            .clone()
            .unwrap_or_else(|| "@qwik.dev/core".to_string()),
        strip_exports: config.strip_exports.clone().unwrap_or_default(),
        strip_ctx_name: config.strip_ctx_name.clone().unwrap_or_default(),
        strip_event_handlers: config.strip_event_handlers,
        reg_ctx_name: config.reg_ctx_name.clone().unwrap_or_default(),
        // SPEC line 259: is_server defaults to true when not specified.
        is_server: config.is_server.unwrap_or(true),
    };

    let mut result = TransformOutput {
        modules: vec![],
        diagnostics: vec![],
        is_type_script: false,
        is_jsx: false,
    };

    for input in &config.input {
        let mut file_output = transform_code(
            &code_config,
            &input.code,
            &input.path,
            input.dev_path.as_deref(),
        )?;
        result.append(&mut file_output);
    }

    // OUT-02: sort by order for deterministic output.
    result.modules.sort_unstable_by_key(|m| m.order);

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(code: &str, path: &str) -> TransformModuleInput {
        TransformModuleInput {
            code: code.to_string(),
            path: path.to_string(),
            dev_path: None,
        }
    }

    fn opts_with_inputs(src_dir: &str, inputs: Vec<TransformModuleInput>) -> TransformModulesOptions {
        TransformModulesOptions {
            src_dir: src_dir.to_string(),
            input: inputs,
            source_maps: false,
            ..TransformModulesOptions::default()
        }
    }

    #[test]
    fn test_transform_tsx_single_module_flags() {
        let opts = opts_with_inputs(
            "/project",
            vec![make_input("export const x = 1;", "component.tsx")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        assert_eq!(result.modules.len(), 1);
        assert!(result.is_type_script, "Expected is_type_script=true for .tsx");
        assert!(result.is_jsx, "Expected is_jsx=true for .tsx");
    }

    #[test]
    fn test_transform_js_single_module_flags() {
        let opts = opts_with_inputs(
            "/project",
            vec![make_input("export const x = 1;", "utils.js")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        assert_eq!(result.modules.len(), 1);
        assert!(!result.is_type_script, "Expected is_type_script=false for .js");
        assert!(!result.is_jsx, "Expected is_jsx=false for .js");
    }

    #[test]
    fn test_transform_empty_input() {
        let opts = opts_with_inputs("/project", vec![]);
        let result = transform_modules(opts).expect("transform_modules failed");
        assert_eq!(result.modules.len(), 0);
    }

    #[test]
    fn test_transform_module_path_no_subdir() {
        let opts = opts_with_inputs(
            "/project",
            vec![make_input("const x = 1;", "component.tsx")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        assert_eq!(result.modules[0].path, "component.tsx");
    }

    #[test]
    fn test_transform_module_path_with_subdir() {
        let opts = opts_with_inputs(
            "/project",
            vec![make_input("const x = 1;", "src/routes/index.tsx")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        assert_eq!(result.modules[0].path, "src/routes/index.tsx");
    }

    #[test]
    fn test_transform_root_module_is_entry_false() {
        let opts = opts_with_inputs(
            "/project",
            vec![make_input("export const x = 1;", "test.ts")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        let module = &result.modules[0];
        assert!(!module.is_entry, "Root modules have is_entry=false");
        assert!(module.segment.is_none(), "Root modules have segment=None");
        assert_eq!(module.order, 0, "Root modules have order=0");
    }

    #[test]
    fn test_transform_code_identity_valid_js() {
        let source = "export const greeting = \"hello\";\n";
        let opts = opts_with_inputs(
            "/project",
            vec![make_input(source, "greet.js")],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        assert!(
            result.modules[0].code.contains("greeting"),
            "Expected identity output containing 'greeting': {}",
            result.modules[0].code
        );
    }

    #[test]
    fn test_transform_output_append_merges() {
        let mut a = TransformOutput {
            modules: vec![TransformModule {
                path: "a.tsx".to_string(),
                is_entry: false,
                code: "const a = 1;".to_string(),
                map: None,
                segment: None,
                orig_path: None,
                order: 0,
            }],
            diagnostics: vec![],
            is_type_script: true,
            is_jsx: false,
        };
        let mut b = TransformOutput {
            modules: vec![TransformModule {
                path: "b.js".to_string(),
                is_entry: false,
                code: "const b = 2;".to_string(),
                map: None,
                segment: None,
                orig_path: None,
                order: 1,
            }],
            diagnostics: vec![],
            is_type_script: false,
            is_jsx: true,
        };
        a.append(&mut b);
        assert_eq!(a.modules.len(), 2);
        assert!(a.is_type_script, "OR: true | false = true");
        assert!(a.is_jsx, "OR: false | true = true");
    }

    #[test]
    fn test_transform_modules_sorted_by_order() {
        // Multiple inputs — after sort they should be in order
        let opts = opts_with_inputs(
            "/project",
            vec![
                make_input("const a = 1;", "a.tsx"),
                make_input("const b = 2;", "b.tsx"),
            ],
        );
        let result = transform_modules(opts).expect("transform_modules failed");
        let orders: Vec<u64> = result.modules.iter().map(|m| m.order).collect();
        let mut sorted = orders.clone();
        sorted.sort_unstable();
        assert_eq!(orders, sorted, "Modules should be sorted by order field");
    }

    #[test]
    fn test_is_server_defaults_true_when_none() {
        // TransformModulesOptions with is_server=None should produce is_server=true
        // (verifiable indirectly — the function must accept None and succeed)
        let opts = TransformModulesOptions {
            src_dir: "/project".to_string(),
            input: vec![make_input("const x = 1;", "test.js")],
            is_server: None,
            source_maps: false,
            ..TransformModulesOptions::default()
        };
        // Should not error
        let result = transform_modules(opts);
        assert!(result.is_ok(), "transform_modules should accept is_server=None");
    }

    // -----------------------------------------------------------------------
    // Integration: Stage 2 (strip_exports) via transform_modules
    // -----------------------------------------------------------------------

    #[test]
    fn integration_strip_exports_produces_throw() {
        let opts = TransformModulesOptions {
            src_dir: "/project".to_string(),
            input: vec![make_input("export const onGet = () => 42;", "route.ts")],
            strip_exports: Some(vec!["onGet".to_string()]),
            source_maps: false,
            ..TransformModulesOptions::default()
        };
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            code.contains("throw"),
            "Stripped export should contain throw stub, got: {code}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration: Stage 5 (import rename) via transform_modules
    // -----------------------------------------------------------------------

    #[test]
    fn integration_import_rename_rewrites_builder_io() {
        let src = r#"import { component$ } from "@builder.io/qwik";"#;
        let opts = opts_with_inputs("/project", vec![make_input(src, "comp.ts")]);
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            code.contains("@qwik.dev/core"),
            "Import source should be rewritten to @qwik.dev/core, got: {code}"
        );
        assert!(
            !code.contains("@builder.io/qwik"),
            "Old import source should be gone, got: {code}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration: Stage 8 (props destructuring) via transform_modules
    // -----------------------------------------------------------------------

    #[test]
    fn integration_props_destructuring_basic() {
        let src = r#"const Cmp = ({ foo, bar }) => { return foo + bar; };"#;
        let opts = opts_with_inputs("/project", vec![make_input(src, "test.tsx")]);
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            code.contains("_rawProps"),
            "Destructured params should be rewritten to _rawProps, got: {code}"
        );
        assert!(
            code.contains("_rawProps.foo"),
            "Should have _rawProps.foo access, got: {code}"
        );
    }

    #[test]
    fn integration_props_destructuring_skips_plain_param() {
        let src = r#"const Cmp = (props) => { return props; };"#;
        let opts = opts_with_inputs("/project", vec![make_input(src, "test.tsx")]);
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            !code.contains("_rawProps"),
            "Plain param should NOT be rewritten, got: {code}"
        );
    }

    #[test]
    fn integration_props_destructuring_lib_mode_runs() {
        let src = r#"const Cmp = ({ foo }) => { return foo; };"#;
        let opts = TransformModulesOptions {
            src_dir: "/project".to_string(),
            input: vec![make_input(src, "test.tsx")],
            mode: EmitMode::Lib,
            source_maps: false,
            ..TransformModulesOptions::default()
        };
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            code.contains("_rawProps"),
            "Lib mode should still run props destructuring, got: {code}"
        );
    }

    // -----------------------------------------------------------------------
    // Integration: Stage 9 (const replace) via transform_modules
    // -----------------------------------------------------------------------

    #[test]
    fn integration_const_replace_is_server_in_output() {
        let src = r#"import { isServer } from "@qwik.dev/core/build"; export const s = isServer;"#;
        let opts = TransformModulesOptions {
            src_dir: "/project".to_string(),
            input: vec![make_input(src, "check.ts")],
            is_server: Some(true),
            mode: EmitMode::Prod,
            source_maps: false,
            ..TransformModulesOptions::default()
        };
        let result = transform_modules(opts).expect("transform_modules failed");
        let code = &result.modules[0].code;
        assert!(
            code.contains("= true"),
            "isServer should be replaced with true in output, got: {code}"
        );
    }
}
