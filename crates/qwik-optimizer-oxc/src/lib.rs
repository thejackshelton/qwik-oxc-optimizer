//! Qwik optimizer using OXC for code transformation.
//!
//! This crate provides `transform_modules()`, which takes one or more input modules
//! and applies Qwik's $-call extraction, import rewriting, and segment splitting
//! transformations. The output matches the SWC optimizer's JSON wire format,
//! enabling drop-in replacement at the TypeScript binding layer.

mod code_move;
mod collector;
mod const_replace;
mod emit;
mod entry_strategy;
mod errors;
mod filter_exports;
mod hash;
mod import_rewrite;
mod is_const;
mod jsx_transform;
mod parse;
mod props_destructuring;
mod transform;
mod types;
mod words;

// Re-export public types
pub use types::{
    CtxKind, Diagnostic, DiagnosticCategory, EmitMode, EntryStrategy, MinifyMode, SegmentAnalysis,
    SourceLocation, TransformModule, TransformModuleInput, TransformModulesOptions,
    TransformOutput,
};

use types::{SegmentData, TransformOptions};

/// Main entry point: transform one or more modules.
///
/// Accepts a `TransformModulesOptions` configuration and returns a `TransformOutput`
/// containing all transformed modules (main + extracted segments) and any diagnostics.
pub fn transform_modules(
    config: TransformModulesOptions,
) -> Result<TransformOutput, anyhow::Error> {
    let mut all_modules = Vec::new();
    let mut all_diagnostics = Vec::new();
    let mut is_type_script = false;
    let mut is_jsx = false;

    // Build per-module TransformOptions from the top-level config
    let transform_options = TransformOptions {
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
        is_server: config.is_server.unwrap_or(false),
    };

    let emit_options = emit::EmitOptions {
        source_maps: config.source_maps,
    };

    for input in &config.input {
        let allocator = oxc::allocator::Allocator::default();
        let source_in_arena = allocator.alloc_str(&input.code);

        let (parse_result, parse_diags) =
            match parse::parse_module(&allocator, source_in_arena, &input.path) {
                Ok((result, diags)) => (result, diags),
                Err(diagnostics) => {
                    // Unrecoverable parse error (parser panicked) -- skip this input
                    all_diagnostics.extend(diagnostics);
                    continue;
                }
            };
        all_diagnostics.extend(parse_diags);

        if parse_result.source_type.is_typescript() {
            is_type_script = true;
        }
        if parse_result.source_type.is_jsx() {
            is_jsx = true;
        }

        let mut program = parse_result.program;
        let mut scoping = parse_result.scoping;

        // BUG-01: Strip TypeScript types before Qwik transform pass.
        // SWC does typescript::strip() before the Qwik fold. Without this,
        // type annotations remain in output when transpile_ts=true.
        if transform_options.transpile_ts && parse_result.source_type.is_typescript() {
            use std::path::Path;
            let ts_options = oxc::transformer::TransformOptions {
                typescript: oxc::transformer::TypeScriptOptions::default(),
                // Explicitly disable JSX transform -- we only want TS stripping here.
                // JsxOptions::default() enables jsx_plugin, which would transform JSX
                // to React.createElement calls before the Qwik pass runs.
                jsx: oxc::transformer::JsxOptions::disable(),
                ..Default::default()
            };
            let transformer = oxc::transformer::Transformer::new(
                &allocator,
                Path::new(&input.path),
                &ts_options,
            );
            let _ts_return = transformer.build_with_scoping(
                scoping,
                &mut program,
            );
            // Rebuild semantic scoping for the stripped AST.
            // The transformer consumes Scoping, so we must rebuild it.
            let semantic_ret = oxc::semantic::SemanticBuilder::new()
                .with_excess_capacity(2.0)
                .build(&program);
            scoping = semantic_ret.semantic.into_scoping();
        }

        let collect_result = collector::collect(&program, &scoping, config.core_module.as_deref());

        // Must happen before traverse_mut so segment body serialization sees replaced values
        const_replace::replace_build_constants(&mut program, &transform_options, &allocator);

        // Must happen before traverse so $-call detection skips stripped exports
        if !transform_options.strip_exports.is_empty() {
            filter_exports::filter_exports(
                &mut program,
                &transform_options.strip_exports,
                &transform_options.strip_ctx_name,
                &allocator,
            );
        }

        let mut qwik_transform =
            transform::QwikTransform::new(&transform_options, collect_result, &input.path, &input.code);

        if let Some(idx) = input.code.find("@jsxImportSource") {
            let after = &input.code[idx + "@jsxImportSource".len()..];
            let module_path = after
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == '*' || c == '/')
                .next()
                .unwrap_or("")
                .trim();
            if !module_path.is_empty() {
                qwik_transform.set_custom_jsx_import_source(Some(module_path.to_string()));
            }
        }

        let _scoping =
            oxc_traverse::traverse_mut(&mut qwik_transform, &allocator, &mut program, scoping, ());

        qwik_transform.finalize_segments();

        let emit_result = emit::emit_module(&program, source_in_arena, &emit_options, &input.path);

        // Prepend hoisted function declarations after synthetic framework imports
        let hoisted_stmts: Vec<(String, String)> = qwik_transform.hoisted_function_stmts().to_vec();
        let synthetic_import_count = qwik_transform.synthetic_import_count();
        // Only inject _hf* into entry module when the entry module code actually
        // references them. For inline/hoist strategies, the component body stays in the
        // entry module. For segment strategy, _hf* usually go into segment files
        // via code_move.rs -- but inline components (export default arrow with JSX)
        // keep their body in the entry module and may reference _hf* there.
        let is_inline_like_strategy = entry_strategy::should_inline(&transform_options.entry_strategy)
            || matches!(transform_options.entry_strategy, EntryStrategy::Hoist);
        // Also inject when the entry module code references any _hf* variable
        // (e.g., inline components that use _fnSignal with hoisted functions).
        let entry_code_refs_hf = !is_inline_like_strategy && hoisted_stmts.iter().any(|(fn_code, _)| {
            if let Some(var_name) = fn_code.strip_prefix("const ").and_then(|s| s.split(|c: char| !c.is_alphanumeric() && c != '_').next()) {
                emit_result.code.contains(var_name)
            } else {
                false
            }
        });
        let main_code = if !hoisted_stmts.is_empty() && (is_inline_like_strategy || entry_code_refs_hf) {
            let mut hoisted_code = String::new();
            for (fn_code, str_code) in &hoisted_stmts {
                hoisted_code.push_str(fn_code);
                hoisted_code.push('\n');
                hoisted_code.push_str(str_code);
                hoisted_code.push('\n');
            }
            let code = &emit_result.code;
            // Find the injection point: after the Nth import line where N = synthetic_import_count.
            // This ensures hoisted _hf* stmts go AFTER synthetic framework imports but BEFORE
            // _Fragment, non-dollar, and user imports (matching SWC's encounter-order output).
            let mut import_count = 0;
            let mut injection_point = 0;
            let mut pos = 0;
            for line in code.lines() {
                let line_end = pos + line.len() + 1; // +1 for \n
                if line.starts_with("import ") {
                    import_count += 1;
                    if import_count <= synthetic_import_count {
                        injection_point = line_end.min(code.len());
                    }
                }
                pos = line_end;
            }
            // Fallback: if no synthetic imports found, inject after last import
            if injection_point == 0 {
                pos = 0;
                for line in code.lines() {
                    let line_end = pos + line.len() + 1;
                    if line.starts_with("import ") {
                        injection_point = line_end.min(code.len());
                    }
                    pos = line_end;
                }
            }
            if injection_point > 0 {
                format!(
                    "{}{}{}",
                    &code[..injection_point],
                    hoisted_code,
                    &code[injection_point..]
                )
            } else {
                format!("{}{}", hoisted_code, code)
            }
        } else {
            emit_result.code.clone()
        };

        let output_ext = output_extension(
            &input.path,
            transform_options.transpile_ts,
            transform_options.transpile_jsx,
        );
        let main_path = if (transform_options.transpile_ts || transform_options.transpile_jsx)
            && !transform_options.preserve_filenames
        {
            input
                .path
                .rsplit_once('.')
                .map_or(input.path.clone(), |(base, _)| {
                    format!("{}.{}", base, output_ext)
                })
        } else {
            input.path.clone()
        };
        // Normalize Windows backslashes to forward slashes in output paths
        let main_path = main_path.replace('\\', "/");

        let main_module = TransformModule {
            path: main_path,
            is_entry: false,
            code: main_code,
            map: emit_result.map,
            segment: None,
            orig_path: Some(input.path.clone()),
        };
        all_modules.push(main_module);

        let auto_exports = qwik_transform.auto_exports().clone();
        let component_invalid_decls = qwik_transform.component_invalid_decls().clone();
        let mut body_codes = qwik_transform.take_segment_body_codes();

        // Apply segment body DCE (dead code elimination) to match SWC's MinifyMode::Simplify.
        // Strips unused declarations, eliminates if(false) branches, and removes
        // function/class declarations tracked as invalid (C02 diagnostics) from component bodies.
        if matches!(transform_options.minify, MinifyMode::Simplify) {
            for (span_start, body_code) in body_codes.iter_mut() {
                let force_remove = component_invalid_decls.get(span_start);
                *body_code = transform::apply_segment_body_dce(body_code, force_remove);
            }
        }

        // Sort segments by source span position (ascending) for consistent output order.
        // SWC's fold processes nodes top-down in source order, while OXC's traverse
        // visitor exits inner nodes before outer ones, producing a different insertion order.
        // Sorting by span.0 (start position) restores source order to match SWC.
        let mut segments = qwik_transform.extracted_segments().to_vec();
        segments.sort_by_key(|seg| seg.span.0);

        // After DCE, filter out segment needed_imports that are no longer referenced
        // in the body code. DCE may have removed dead branches that referenced some imports.
        if matches!(transform_options.minify, MinifyMode::Simplify) {
            for seg in segments.iter_mut() {
                if let Some((_, body_code)) = body_codes.iter().find(|(s, _)| *s == seg.span.0) {
                    seg.needed_imports.retain(|imp| {
                        imp.specifiers.iter().any(|spec| body_code.contains(spec.as_str()))
                    });
                }
            }
        }
        let stripped_spans = qwik_transform.stripped_segments();
        let custom_jsx_src = qwik_transform.custom_jsx_import_source().map(|s| s.to_string());

        let is_inline_like = entry_strategy::should_inline(&transform_options.entry_strategy)
            || matches!(transform_options.entry_strategy, EntryStrategy::Hoist);

        for seg in &segments {
            if stripped_spans.contains(&seg.span.0) {
                continue;
            }

            if is_inline_like {
                continue;
            }

            let segment_analysis = segment_data_to_analysis(
                seg,
                &input.path,
                transform_options.transpile_ts,
                transform_options.transpile_jsx,
                &transform_options.entry_strategy,
            );
            let seg_ext = output_extension(
                &input.path,
                transform_options.transpile_ts,
                transform_options.transpile_jsx,
            );

            {
                let body_code = body_codes
                    .iter()
                    .find(|(span_start, _)| *span_start == seg.span.0)
                    .map(|(_, code)| code.as_str())
                    .unwrap_or("");

                let seg_path = if segment_analysis.path.is_empty() {
                    format!("{}.{}", segment_analysis.canonical_filename, seg_ext)
                } else {
                    format!(
                        "{}/{}.{}",
                        segment_analysis.path, segment_analysis.canonical_filename, seg_ext
                    )
                };
                let (segment_code, segment_map) = if !body_code.is_empty() {
                    let raw_code = code_move::build_segment_code_with_hoisted(
                        body_code,
                        seg,
                        &transform_options,
                        &hoisted_stmts,
                        custom_jsx_src.as_deref(),
                        &auto_exports,
                    );
                    code_move::emit_segment_with_map(&raw_code, &seg_path, emit_options.source_maps)
                } else {
                    // Diagnostic: non-stripped segment has no body code
                    all_diagnostics.push(Diagnostic {
                        scope: "optimizer".to_string(),
                        message: format!(
                            "Segment '{}' (ctx: {}) has no body code - segment module will be empty",
                            seg.display_name, seg.ctx_name
                        ),
                        category: DiagnosticCategory::Warning,
                        code: Some("EMPTY_SEGMENT_BODY".to_string()),
                        file: input.path.clone(),
                        highlights: None,
                        suggestions: None,
                    });
                    (String::new(), None)
                };

                // SWC: is_entry is true when entry field is None (segment is its own entry point).
                // When entry is Some(...), the segment is grouped into that entry, not a standalone entry.
                let is_entry = segment_analysis.entry.is_none();

                let segment_module = TransformModule {
                    path: seg_path,
                    is_entry,
                    code: segment_code,
                    map: segment_map,
                    segment: Some(segment_analysis),
                    orig_path: Some(input.path.clone()),
                };
                all_modules.push(segment_module);
            }
        }

        all_diagnostics.extend(qwik_transform.diagnostics().to_vec());
    }

    Ok(TransformOutput {
        modules: all_modules,
        diagnostics: all_diagnostics,
        is_type_script,
        is_jsx,
    })
}

/// Compute the output file extension, accounting for transpile_ts and transpile_jsx.
///
/// Logic: strip what you transpile.
/// - transpile_ts removes TypeScript type annotations: tsx->jsx, ts->js
/// - transpile_jsx removes JSX syntax: tsx->ts, jsx->js
/// - both: tsx->js, ts->js
/// Otherwise the original extension is preserved.
fn output_extension(input_path: &str, transpile_ts: bool, transpile_jsx: bool) -> String {
    let ext = input_path.rsplit('.').next().unwrap_or("js");
    match (transpile_ts, transpile_jsx, ext) {
        (true, true, "tsx") => "js".to_string(),
        (true, true, "ts") => "js".to_string(),
        (true, false, "tsx") => "jsx".to_string(),
        (true, false, "ts") => "js".to_string(),
        (false, true, "tsx") => "ts".to_string(),
        (false, true, "jsx") => "js".to_string(),
        _ => ext.to_string(),
    }
}

/// Extract the directory part of a path (everything before the last path separator).
///
/// E.g., "src/routes/_repl/[id]/[[...slug]].tsx" -> "src/routes/_repl/[id]"
/// E.g., "test.tsx" -> ""
/// E.g., "components\\apps\\apps.tsx" -> "components\\apps"
///
/// Handles both Unix `/` and Windows `\\` separators.
/// Matches SWC's `path_data.rel_dir`.
fn rel_dir(path: &str) -> String {
    let last_sep = path.rfind(|c: char| c == '/' || c == '\\');
    if let Some(pos) = last_sep {
        path[..pos].replace('\\', "/")
    } else {
        String::new()
    }
}

/// Compute the `entry` field for a segment based on the entry strategy.
///
/// Mirrors SWC's `EntryPolicy::get_entry_for_sym()`:
/// - Inline/Hoist: `Some("entry_segments")`
/// - Single: `Some("entry_segments")`
/// - Segment/Hook: `None`
/// - Smart: non-function segments (event handlers) or "event$" without captures => None,
///          top-level functions (empty stack_ctxt) => None,
///          nested functions => origin + "_entry_" + root context name
/// - Component: no context => "entry_segments", has context => origin + "_entry_" + root
fn compute_entry_field(
    strategy: &EntryStrategy,
    origin: &str,
    stack_ctxt: &[String],
    ctx_kind: &CtxKind,
    ctx_name: &str,
    has_captures: bool,
) -> Option<String> {
    match strategy {
        EntryStrategy::Inline | EntryStrategy::Hoist => Some("entry_segments".to_string()),
        EntryStrategy::Single => Some("entry_segments".to_string()),
        EntryStrategy::Segment | EntryStrategy::Hook => None,
        EntryStrategy::Smart => {
            // SWC: if scoped_idents.is_empty() && (ctx_kind != Function || ctx_name == "event$")
            // Event handlers (non-function) or "event$" function, without captures -> None
            if !has_captures
                && (matches!(ctx_kind, CtxKind::EventHandler) || ctx_name == "event$")
            {
                return None;
            }
            // Top-level QRLs (empty stack_ctxt) get None, nested get component-based entry
            if let Some(root) = stack_ctxt.first() {
                Some(format!("{}_entry_{}", origin, root))
            } else {
                None
            }
        }
        EntryStrategy::Component => {
            if let Some(root) = stack_ctxt.first() {
                Some(format!("{}_entry_{}", origin, root))
            } else {
                Some("entry_segments".to_string())
            }
        }
    }
}

/// Convert internal SegmentData to public SegmentAnalysis.
fn segment_data_to_analysis(
    seg: &SegmentData,
    origin_path: &str,
    transpile_ts: bool,
    transpile_jsx: bool,
    entry_strategy: &EntryStrategy,
) -> SegmentAnalysis {
    // Normalize origin path: replace backslashes with forward slashes (Windows support)
    let normalized_origin = origin_path.replace('\\', "/");
    let canonical_filename = format!("{}_{}", seg.display_name, seg.hash);
    let ext = output_extension(origin_path, transpile_ts, transpile_jsx);

    // Compute entry field from strategy.
    // For Smart/Component strategies, stack_ctxt contains the scope names at segment creation.
    // Event handlers without captures get None for Smart strategy (SWC behavior).
    let entry = compute_entry_field(
        entry_strategy,
        &normalized_origin,
        &seg.stack_ctxt,
        &seg.ctx_kind,
        &seg.ctx_name,
        seg.captures,
    );

    SegmentAnalysis {
        origin: normalized_origin,
        name: seg.name.clone(),
        entry,
        display_name: seg.display_name.clone(),
        hash: seg.hash.clone(),
        canonical_filename,
        path: rel_dir(origin_path),
        extension: ext,
        parent: seg.parent.clone(),
        ctx_kind: seg.ctx_kind.clone(),
        ctx_name: seg.ctx_name.clone(),
        captures: seg.captures,
        capture_names: if seg.capture_names.is_empty() {
            None
        } else {
            Some(seg.capture_names.clone())
        },
        loc: seg.span,
        param_names: if seg.param_names.is_empty() {
            None
        } else {
            Some(seg.param_names.clone())
        },
    }
}

