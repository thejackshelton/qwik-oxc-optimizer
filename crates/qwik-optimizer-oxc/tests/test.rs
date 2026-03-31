//! Unified old-style snapshot test harness for qwik-optimizer-oxc.
//!
//! This file intentionally avoids parsing markdown spec files at runtime.
//! Test inputs are loaded from `tests/input/*.tsx`, and per-case configuration
//! is defined in Rust via `apply_case_overrides`.

use qwik_optimizer_oxc::{
    CtxKind, EmitMode, EntryStrategy, MinifyMode, SegmentAnalysis, TransformModuleInput,
    TransformModulesOptions, transform_modules,
};
use serde::Serialize;
use serde_json::to_string_pretty;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
struct TestInput {
    name: String,
    code: String,
    filename: String,
    src_dir: String,
    root_dir: Option<String>,
    entry_strategy: EntryStrategy,
    minify: MinifyMode,
    transpile_ts: bool,
    transpile_jsx: bool,
    preserve_filenames: bool,
    explicit_extensions: bool,
    source_maps: bool,
    mode: EmitMode,
    core_module: Option<String>,
    scope: Option<String>,
    strip_exports: Option<Vec<String>>,
    reg_ctx_name: Option<Vec<String>>,
    strip_ctx_name: Option<Vec<String>>,
    strip_event_handlers: bool,
    is_server: Option<bool>,
    additional_inputs: Vec<TransformModuleInput>,
}

impl TestInput {
    fn for_case(name: &str) -> Self {
        Self {
            name: name.to_string(),
            code: load_input_code(name),
            filename: "test.tsx".to_string(),
            src_dir: ".".to_string(),
            root_dir: None,
            entry_strategy: EntryStrategy::Segment,
            minify: MinifyMode::Simplify,
            transpile_ts: false,
            transpile_jsx: false,
            preserve_filenames: false,
            explicit_extensions: false,
            source_maps: false,
            mode: EmitMode::Lib,
            core_module: None,
            scope: None,
            strip_exports: None,
            reg_ctx_name: None,
            strip_ctx_name: None,
            strip_event_handlers: false,
            is_server: None,
            additional_inputs: Vec::new(),
        }
    }
}

fn load_input_code(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("input")
        .join(format!("{}.tsx", name));
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("failed to load input fixture {}: {}", path.display(), e);
    })
}

fn build_case(name: &str) -> TestInput {
    let mut case = TestInput::for_case(name);
    apply_case_overrides(name, &mut case);

    if name == "relative_paths" {
        case.filename = "components/main.tsx".to_string();
        case.additional_inputs.push(TransformModuleInput {
            code: RELATIVE_PATHS_DEP.to_string(),
            path: "../../node_modules/dep/dist/lib.mjs".to_string(),
        });
    }

    case
}

fn run_case(case: &TestInput) -> Result<qwik_optimizer_oxc::TransformOutput, anyhow::Error> {
    let mut input = case.additional_inputs.clone();
    input.push(TransformModuleInput {
        code: case.code.clone(),
        path: case.filename.clone(),
    });

    transform_modules(TransformModulesOptions {
        src_dir: case.src_dir.clone(),
        root_dir: case.root_dir.clone(),
        input,
        source_maps: case.source_maps,
        minify: case.minify.clone(),
        transpile_ts: case.transpile_ts,
        transpile_jsx: case.transpile_jsx,
        preserve_filenames: case.preserve_filenames,
        entry_strategy: case.entry_strategy.clone(),
        explicit_extensions: case.explicit_extensions,
        mode: case.mode.clone(),
        scope: case.scope.clone(),
        core_module: case.core_module.clone(),
        strip_exports: case.strip_exports.clone(),
        strip_ctx_name: case.strip_ctx_name.clone(),
        strip_event_handlers: case.strip_event_handlers,
        reg_ctx_name: case.reg_ctx_name.clone(),
        is_server: case.is_server,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotSegmentMetadata<'a> {
    origin: &'a str,
    name: &'a str,
    entry: Option<&'a str>,
    display_name: &'a str,
    hash: &'a str,
    canonical_filename: &'a str,
    path: &'a str,
    extension: &'a str,
    parent: Option<&'a str>,
    ctx_kind: &'a CtxKind,
    ctx_name: &'a str,
    captures: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_names: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    param_names: Option<&'a Vec<String>>,
}

fn format_segment_metadata(segment: &SegmentAnalysis) -> String {
    let snapshot_metadata = SnapshotSegmentMetadata {
        origin: &segment.origin,
        name: &segment.name,
        entry: segment.entry.as_deref(),
        display_name: &segment.display_name,
        hash: &segment.hash,
        canonical_filename: &segment.canonical_filename,
        path: &segment.path,
        extension: &segment.extension,
        parent: segment.parent.as_deref(),
        ctx_kind: &segment.ctx_kind,
        ctx_name: &segment.ctx_name,
        captures: segment.captures,
        capture_names: segment.capture_names.as_ref(),
        param_names: segment.param_names.as_ref(),
    };
    to_string_pretty(&snapshot_metadata).expect("failed to serialize segment metadata")
}

#[derive(Debug, Clone)]
struct SnapshotModuleOutput {
    path: String,
    is_entry: bool,
    code: String,
    segment_json: Option<String>,
}

#[derive(Debug, Clone)]
struct SnapshotOutput {
    input_filename: String,
    input_code: String,
    modules: Vec<SnapshotModuleOutput>,
    diagnostics_json: String,
}

#[derive(Debug, Clone)]
enum SnapshotCaseData {
    Output(SnapshotOutput),
    Error(String),
}

#[derive(Debug, Clone)]
struct SnapshotCase {
    name: String,
    data: SnapshotCaseData,
}

fn build_snapshot_output(
    case: &TestInput,
    result: &qwik_optimizer_oxc::TransformOutput,
) -> SnapshotOutput {
    let modules = result
        .modules
        .iter()
        .map(|module| SnapshotModuleOutput {
            path: module.path.clone(),
            is_entry: module.is_entry,
            code: module.code.clone(),
            segment_json: module.segment.as_ref().map(format_segment_metadata),
        })
        .collect();

    let diagnostics_json =
        to_string_pretty(&result.diagnostics).expect("failed to serialize diagnostics");

    SnapshotOutput {
        input_filename: case.filename.clone(),
        input_code: case.code.clone(),
        modules,
        diagnostics_json,
    }
}

fn render_snapshot_output(output: &SnapshotOutput) -> String {
    let mut s = String::new();
    s.push_str("=== INPUT ===\n");
    s.push_str(&output.input_code);
    if !output.input_code.ends_with('\n') {
        s.push('\n');
    }
    s.push('\n');

    for (i, module) in output.modules.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        let entry_marker = if module.is_entry { " (ENTRY)" } else { "" };
        s.push_str(&format!("=== {} ==={}\n", module.path, entry_marker));
        s.push_str(&module.code);
        if !module.code.ends_with('\n') {
            s.push('\n');
        }

        if let Some(segment_json) = &module.segment_json {
            s.push_str("\n/*\n");
            s.push_str(segment_json);
            s.push_str("\n*/\n");
        }
    }

    s.push_str("\n=== DIAGNOSTICS ===\n\n");
    s.push_str(&output.diagnostics_json);

    s
}

/// Replace real hashes with XXXXXXXXXXXX in a SnapshotOutput before oxfmt runs,
/// so formatting decisions are made with the same placeholder widths as the golden
/// snapshots (which were formatted by format-snapshot-code-blocks.mjs after hash
/// replacement).
fn replace_hashes_in_output(output: &mut SnapshotOutput) {
    let hash_re = regex_lite::Regex::new(r#""hash":\s*"([A-Za-z0-9_-]+)""#).unwrap();
    let mut hashes: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for module in &output.modules {
        if let Some(ref json) = module.segment_json {
            for cap in hash_re.captures_iter(json) {
                let hash = cap[1].to_string();
                if seen.insert(hash.clone()) {
                    hashes.push(hash);
                }
            }
        }
    }

    if hashes.is_empty() {
        return;
    }

    for hash in &hashes {
        for module in &mut output.modules {
            module.code = module.code.replace(hash.as_str(), "XXXXXXXXXXXX");
            module.path = module.path.replace(hash.as_str(), "XXXXXXXXXXXX");
            if let Some(ref mut json) = module.segment_json {
                *json = json.replace(hash.as_str(), "XXXXXXXXXXXX");
            }
        }
        output.diagnostics_json = output.diagnostics_json.replace(hash.as_str(), "XXXXXXXXXXXX");
    }
}

#[derive(Debug)]
enum OxfmtRunner {
    Local(PathBuf),
    GlobalBinary,
    Unavailable,
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn detect_oxfmt_runner() -> OxfmtRunner {
    let bin_name = if cfg!(windows) { "oxfmt.cmd" } else { "oxfmt" };

    // Walk up from CARGO_MANIFEST_DIR to find node_modules/.bin/oxfmt.
    // This handles git worktrees where node_modules lives in the main repo
    // root rather than the worktree directory.
    for ancestor in PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors() {
        let candidate = ancestor.join("node_modules").join(".bin").join(bin_name);
        if candidate.exists() {
            return OxfmtRunner::Local(candidate);
        }
    }

    if command_available("oxfmt") {
        return OxfmtRunner::GlobalBinary;
    }
    OxfmtRunner::Unavailable
}

fn oxfmt_runner() -> &'static OxfmtRunner {
    static RUNNER: OnceLock<OxfmtRunner> = OnceLock::new();
    RUNNER.get_or_init(detect_oxfmt_runner)
}

fn snapshot_oxfmt_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("QWIK_SNAPSHOT_OXFMT") {
        Ok(value) => !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    })
}

fn oxfmt_extension(path_like: &str) -> &'static str {
    let normalized = path_like.replace('\\', "/").to_ascii_lowercase();
    if normalized.ends_with(".ts") {
        "ts"
    } else if normalized.ends_with(".jsx") {
        "jsx"
    } else if normalized.ends_with(".js") {
        "js"
    } else if normalized.ends_with(".mjs") {
        "mjs"
    } else if normalized.ends_with(".cjs") {
        "cjs"
    } else if normalized.ends_with(".mts") {
        "mts"
    } else if normalized.ends_with(".cts") {
        "cts"
    } else {
        "tsx"
    }
}

#[derive(Debug, Clone, Copy)]
enum SnapshotCodeTarget {
    Input,
    Module(usize),
}

#[derive(Debug, Clone)]
struct SnapshotCodeRef {
    case_index: usize,
    target: SnapshotCodeTarget,
    tmp_file: PathBuf,
}

fn run_oxfmt_write(target_dir: &std::path::Path) -> bool {
    let mut cmd = match oxfmt_runner() {
        OxfmtRunner::Local(path) => {
            let mut c = Command::new(path);
            c.arg("--write").arg(".");
            c
        }
        OxfmtRunner::GlobalBinary => {
            let mut c = Command::new("oxfmt");
            c.arg("--write").arg(".");
            c
        }
        OxfmtRunner::Unavailable => return false,
    };

    cmd.current_dir(target_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn bulk_oxfmt_snapshot_cases(cases: &mut [SnapshotCase]) {
    if !snapshot_oxfmt_enabled() || matches!(oxfmt_runner(), OxfmtRunner::Unavailable) {
        return;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let tmp_dir = std::env::temp_dir().join(format!(
        "qwik-optimizer-oxfmt-{}-{}",
        std::process::id(),
        nonce
    ));
    if fs::create_dir_all(&tmp_dir).is_err() {
        return;
    }

    let mut refs: Vec<SnapshotCodeRef> = Vec::new();
    let mut block_counter: usize = 0;

    for (case_index, case) in cases.iter().enumerate() {
        let SnapshotCaseData::Output(output) = &case.data else {
            continue;
        };

        block_counter += 1;
        let input_file = tmp_dir.join(format!(
            "block_{block_counter:05}.{}",
            oxfmt_extension(&output.input_filename)
        ));
        if fs::write(&input_file, &output.input_code).is_ok() {
            refs.push(SnapshotCodeRef {
                case_index,
                target: SnapshotCodeTarget::Input,
                tmp_file: input_file,
            });
        }

        for (module_index, module) in output.modules.iter().enumerate() {
            block_counter += 1;
            let module_file = tmp_dir.join(format!(
                "block_{block_counter:05}.{}",
                oxfmt_extension(&module.path)
            ));
            if fs::write(&module_file, &module.code).is_ok() {
                refs.push(SnapshotCodeRef {
                    case_index,
                    target: SnapshotCodeTarget::Module(module_index),
                    tmp_file: module_file,
                });
            }
        }
    }

    if refs.is_empty() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return;
    }

    if !run_oxfmt_write(&tmp_dir) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return;
    }

    for code_ref in refs {
        let Ok(formatted) = fs::read_to_string(&code_ref.tmp_file) else {
            continue;
        };

        let Some(case) = cases.get_mut(code_ref.case_index) else {
            continue;
        };
        let SnapshotCaseData::Output(output) = &mut case.data else {
            continue;
        };

        match code_ref.target {
            SnapshotCodeTarget::Input => output.input_code = formatted,
            SnapshotCodeTarget::Module(module_index) => {
                if let Some(module) = output.modules.get_mut(module_index) {
                    module.code = formatted;
                }
            }
        }
    }

    let _ = fs::remove_dir_all(&tmp_dir);
}

const RELATIVE_PATHS_DEP: &str = r#"import { componentQrl, inlinedQrl, useStore, useLexicalScope } from "@qwik.dev/core";
import { jsx, jsxs } from "@qwik.dev/core/jsx-runtime";
import { state } from './sibling';

const useData = () => {
    return useStore({
        count: 0
    });
}

export const App = /*#__PURE__*/ componentQrl(inlinedQrl(()=>{
    const store = useData();
    return /*#__PURE__*/ jsxs("div", {
        children: [
            /*#__PURE__*/ jsxs("p", {
                children: [
                    "Count: ",
                    store.count
                ]
            }),
            /*#__PURE__*/ jsx("p", {
                children: /*#__PURE__*/ jsx("button", {
                    onClick$: inlinedQrl(()=>{
                        const [store] = useLexicalScope();
                        return store.count++;
                    }, "App_component_div_p_button_onClick_8dWUa0cJAr4", [
                        store
                    ]),
                    children: "Click"
                })
            })
        ]
    });
}, "App_component_AkbU84a8zes"));"#;

#[test]
fn snapshot_all_transforms() {
    let mut cases: Vec<SnapshotCase> = Vec::with_capacity(CASE_NAMES.len());

    for name in CASE_NAMES {
        let case = build_case(name);
        let data = match run_case(&case) {
            Ok(result) => SnapshotCaseData::Output(build_snapshot_output(&case, &result)),
            Err(e) => SnapshotCaseData::Error(format!("TRANSFORM ERROR: {}", e)),
        };
        cases.push(SnapshotCase {
            name: case.name,
            data,
        });
    }

    // Replace hashes BEFORE oxfmt so formatting decisions use the same
    // XXXXXXXXXXXX placeholder widths as the golden (SWC) snapshots.
    for case in &mut cases {
        if let SnapshotCaseData::Output(ref mut output) = case.data {
            replace_hashes_in_output(output);
        }
    }

    bulk_oxfmt_snapshot_cases(&mut cases);

    for case in cases {
        let output = match &case.data {
            SnapshotCaseData::Output(output) => render_snapshot_output(output),
            SnapshotCaseData::Error(error) => error.clone(),
        };
        insta::with_settings!({prepend_module_to_snapshot => false}, {
            insta::assert_snapshot!(case.name, output);
        });
    }
}

const CASE_NAMES: &[&str] = &[
    "destructure_args_colon_props",
    "destructure_args_colon_props2",
    "destructure_args_colon_props3",
    "destructure_args_inline_cmp_block_stmt",
    "destructure_args_inline_cmp_block_stmt2",
    "destructure_args_inline_cmp_expr_stmt",
    "example_1",
    "example_10",
    "example_11",
    "example_2",
    "example_3",
    "example_4",
    "example_5",
    "example_6",
    "example_7",
    "example_8",
    "example_9",
    "example_build_server",
    "example_capture_imports",
    "example_capturing_fn_class",
    "example_class_name",
    "example_component_with_event_listeners_inside_loop",
    "example_custom_inlined_functions",
    "example_dead_code",
    "example_default_export",
    "example_default_export_index",
    "example_default_export_invalid_ident",
    "example_derived_signals_children",
    "example_derived_signals_cmp",
    "example_derived_signals_complext_children",
    "example_derived_signals_div",
    "example_derived_signals_multiple_children",
    "example_dev_mode",
    "example_dev_mode_inlined",
    "example_drop_side_effects",
    "example_explicit_ext_no_transpile",
    "example_explicit_ext_transpile",
    "example_export_issue",
    "example_exports",
    "example_fix_dynamic_import",
    "example_functional_component",
    "example_functional_component_2",
    "example_functional_component_capture_props",
    "example_getter_generation",
    "example_immutable_analysis",
    "example_immutable_function_components",
    "example_import_assertion",
    "example_inlined_entry_strategy",
    "example_input_bind",
    "example_invalid_references",
    "example_invalid_segment_expr1",
    "example_issue_33443",
    "example_issue_4438",
    "example_jsx",
    "example_jsx_import_source",
    "example_jsx_keyed",
    "example_jsx_keyed_dev",
    "example_jsx_listeners",
    "example_lightweight_functional",
    "example_manual_chunks",
    "example_missing_custom_inlined_functions",
    "example_multi_capture",
    "example_mutable_children",
    "example_noop_dev_mode",
    "example_of_synchronous_qrl",
    "example_optimization_issue_3542",
    "example_optimization_issue_3561",
    "example_optimization_issue_3795",
    "example_optimization_issue_4386",
    "example_parsed_inlined_qrls",
    "example_preserve_filenames",
    "example_preserve_filenames_segments",
    "example_prod_node",
    "example_props_optimization",
    "example_props_wrapping",
    "example_props_wrapping2",
    "example_props_wrapping_children",
    "example_props_wrapping_children2",
    "example_qwik_conflict",
    "example_qwik_react",
    "example_qwik_react_inline",
    "example_qwik_router_inline",
    "example_reg_ctx_name_segments",
    "example_reg_ctx_name_segments_hoisted",
    "example_reg_ctx_name_segments_inlined",
    "example_renamed_exports",
    "example_server_auth",
    "example_skip_transform",
    "example_spread_jsx",
    "example_strip_client_code",
    "example_strip_exports_unused",
    "example_strip_exports_used",
    "example_strip_server_code",
    "example_transpile_jsx_only",
    "example_transpile_ts_only",
    "example_ts_enums",
    "example_ts_enums_issue_1341",
    "example_ts_enums_no_transpile",
    "example_use_client_effect",
    "example_use_optimization",
    "example_use_server_mount",
    "example_with_style",
    "example_with_tagname",
    "hoisted_fn_signal_in_loop",
    "impure_template_fns",
    "issue_117",
    "issue_150",
    "issue_476",
    "issue_5008",
    "issue_7216_add_test",
    "issue_964",
    "lib_mode_fn_signal",
    "relative_paths",
    "rename_builder_io",
    "should_convert_jsx_events",
    "should_convert_rest_props",
    "should_destructure_args",
    "should_extract_single_qrl",
    "should_extract_single_qrl_2",
    "should_extract_single_qrl_with_index",
    "should_extract_single_qrl_with_nested_components",
    "should_handle_dangerously_set_inner_html",
    "should_ignore_null_inlined_qrl",
    "should_mark_props_as_var_props_for_inner_cmp",
    "should_merge_attributes_with_spread_props",
    "should_merge_attributes_with_spread_props_before_and_after",
    "should_merge_bind_checked_and_on_input",
    "should_merge_bind_value_and_on_input",
    "should_merge_on_input_and_bind_checked",
    "should_merge_on_input_and_bind_value",
    "should_move_bind_value_to_var_props",
    "should_move_props_related_to_iteration_variables_to_var_props",
    "should_not_generate_conflicting_props_identifiers",
    "should_not_move_over_side_effects",
    "should_not_transform_bind_checked_in_var_props_for_jsx_split",
    "should_not_transform_bind_value_in_var_props_for_jsx_split",
    "should_not_transform_events_on_non_elements",
    "should_not_wrap_fn",
    "should_not_wrap_ternary_function_operator_with_fn",
    "should_not_wrap_var_template_string",
    "should_split_spread_props",
    "should_split_spread_props_with_additional_prop",
    "should_split_spread_props_with_additional_prop2",
    "should_split_spread_props_with_additional_prop3",
    "should_split_spread_props_with_additional_prop4",
    "should_split_spread_props_with_additional_prop5",
    "should_transform_component_with_normal_function",
    "should_transform_event_names_without_jsx_transpile",
    "should_transform_multiple_event_handlers",
    "should_transform_multiple_event_handlers_case2",
    "should_transform_nested_loops",
    "should_transform_qrls_in_ternary_expression",
    "should_wrap_inner_inline_component_prop",
    "should_wrap_logical_expression_in_template",
    "should_wrap_object_with_fn_signal",
    "should_wrap_prop_from_destructured_array",
    "should_wrap_store_expression",
    "should_wrap_type_asserted_variables_in_template",
    "special_jsx",
    "support_windows_paths",
    "ternary_prop",
    "transform_qrl_in_regular_prop",
];

fn apply_case_overrides(name: &str, case: &mut TestInput) {
    match name {
        "destructure_args_colon_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "destructure_args_colon_props2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "destructure_args_colon_props3" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "destructure_args_inline_cmp_block_stmt" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "destructure_args_inline_cmp_block_stmt2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "destructure_args_inline_cmp_expr_stmt" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_10" => {
            case.filename = "project/test.tsx".to_string();
        }
        "example_11" => {
            case.filename = "project/test.tsx".to_string();
        }
        "example_build_server" => {
            case.mode = EmitMode::Prod;
            case.is_server = Some(true);
        }
        "example_capture_imports" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_capturing_fn_class" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_class_name" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_component_with_event_listeners_inside_loop" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_custom_inlined_functions" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_default_export" => {
            case.filename = "src/routes/_repl/[id]/[[...slug]].tsx".to_string();
            case.entry_strategy = EntryStrategy::Smart;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_default_export_index" => {
            case.filename = "src/components/mongo/index.tsx".to_string();
            case.entry_strategy = EntryStrategy::Inline;
        }
        "example_default_export_invalid_ident" => {
            case.filename = "src/components/mongo/404.tsx".to_string();
        }
        "example_derived_signals_children" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_derived_signals_cmp" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_derived_signals_complext_children" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_derived_signals_div" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_derived_signals_multiple_children" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_dev_mode" => {
            case.src_dir = "/user/qwik/src/".to_string();
            case.mode = EmitMode::Dev;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_dev_mode_inlined" => {
            case.src_dir = "/user/qwik/src/".to_string();
            case.entry_strategy = EntryStrategy::Inline;
            case.mode = EmitMode::Dev;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_drop_side_effects" => {
            case.src_dir = "/user/qwik/src/".to_string();
            case.mode = EmitMode::Dev;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.strip_ctx_name = Some(vec!["server".to_string()]);
            case.is_server = Some(false);
        }
        "example_explicit_ext_no_transpile" => {
            case.entry_strategy = EntryStrategy::Single;
            case.explicit_extensions = true;
        }
        "example_explicit_ext_transpile" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_export_issue" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_exports" => {
            case.filename = "project/test.tsx".to_string();
            case.transpile_ts = true;
        }
        "example_fix_dynamic_import" => {
            case.filename = "project/folder/test.tsx".to_string();
            case.entry_strategy = EntryStrategy::Single;
        }
        "example_functional_component" => {
            case.minify = MinifyMode::None;
        }
        "example_functional_component_2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_functional_component_capture_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_getter_generation" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_immutable_analysis" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_immutable_function_components" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_import_assertion" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_inlined_entry_strategy" => {
            case.entry_strategy = EntryStrategy::Inline;
        }
        "example_input_bind" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.mode = EmitMode::Prod;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_invalid_references" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_invalid_segment_expr1" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_issue_33443" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_issue_4438" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_jsx" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_jsx_import_source" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_jsx_keyed" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_jsx_keyed_dev" => {
            case.filename = "project/index.tsx".to_string();
            case.src_dir = "/src/project".to_string();
            case.mode = EmitMode::Dev;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_jsx_listeners" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_manual_chunks" => {
            case.entry_strategy = EntryStrategy::Smart;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_missing_custom_inlined_functions" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_multi_capture" => {
            case.transpile_ts = true;
        }
        "example_mutable_children" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_noop_dev_mode" => {
            case.src_dir = "/hello/from/dev/".to_string();
            case.mode = EmitMode::Dev;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.strip_ctx_name = Some(vec!["server".to_string()]);
            case.strip_event_handlers = true;
        }
        "example_of_synchronous_qrl" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_optimization_issue_3542" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.is_server = Some(false);
        }
        "example_optimization_issue_3561" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.is_server = Some(false);
        }
        "example_optimization_issue_3795" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.is_server = Some(false);
        }
        "example_optimization_issue_4386" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.is_server = Some(false);
        }
        "example_parsed_inlined_qrls" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.mode = EmitMode::Prod;
        }
        "example_preserve_filenames" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_jsx = true;
            case.preserve_filenames = true;
            case.explicit_extensions = true;
        }
        "example_preserve_filenames_segments" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.preserve_filenames = true;
            case.explicit_extensions = true;
        }
        "example_prod_node" => {
            case.mode = EmitMode::Prod;
        }
        "example_props_optimization" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_props_wrapping" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_props_wrapping2" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_props_wrapping_children" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_props_wrapping_children2" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_qwik_conflict" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_qwik_react" => {
            case.filename = "../node_modules/@qwik.dev/react/index.qwik.mjs".to_string();
            case.explicit_extensions = true;
        }
        "example_qwik_react_inline" => {
            case.filename = "../node_modules/@qwik.dev/react/index.qwik.mjs".to_string();
            case.entry_strategy = EntryStrategy::Inline;
            case.explicit_extensions = true;
        }
        "example_qwik_router_inline" => {
            case.filename = "../node_modules/@qwik.dev/router/index.qwik.mjs".to_string();
            case.entry_strategy = EntryStrategy::Smart;
            case.explicit_extensions = true;
        }
        "example_reg_ctx_name_segments" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.strip_event_handlers = true;
            case.reg_ctx_name = Some(vec!["server".to_string()]);
        }
        "example_reg_ctx_name_segments_hoisted" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.reg_ctx_name = Some(vec!["server".to_string()]);
        }
        "example_reg_ctx_name_segments_inlined" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.reg_ctx_name = Some(vec!["server".to_string()]);
        }
        "example_renamed_exports" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_server_auth" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_skip_transform" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_spread_jsx" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_strip_client_code" => {
            case.filename = "components/component.tsx".to_string();
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.strip_ctx_name = Some(vec!["useClientMount$".to_string()]);
            case.strip_event_handlers = true;
        }
        "example_strip_exports_unused" => {
            case.strip_exports = Some(vec!["onGet".to_string()]);
        }
        "example_strip_exports_used" => {
            case.strip_exports = Some(vec!["onGet".to_string()]);
        }
        "example_strip_server_code" => {
            case.mode = EmitMode::Prod;
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.strip_ctx_name = Some(vec!["server".to_string()]);
        }
        "example_transpile_jsx_only" => {
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "example_transpile_ts_only" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.explicit_extensions = true;
        }
        "example_ts_enums" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_ts_enums_issue_1341" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_use_client_effect" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "example_use_optimization" => {
            case.entry_strategy = EntryStrategy::Inline;
            case.transpile_ts = true;
            case.is_server = Some(false);
        }
        "example_use_server_mount" => {
            case.entry_strategy = EntryStrategy::Smart;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "hoisted_fn_signal_in_loop" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "impure_template_fns" => {
            case.transpile_jsx = true;
        }
        "issue_117" => {
            case.filename = "project/test.tsx".to_string();
            case.entry_strategy = EntryStrategy::Single;
        }
        "issue_150" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "issue_5008" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "issue_7216_add_test" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "issue_964" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "lib_mode_fn_signal" => {
            case.transpile_jsx = true;
        }
        "relative_paths" => {
            case.src_dir = "/path/to/app/src/thing".to_string();
            case.root_dir = Some("/path/to/app/".to_string());
            case.transpile_ts = true;
            case.transpile_jsx = true;
            case.explicit_extensions = true;
        }
        "rename_builder_io" => {
            case.transpile_jsx = true;
        }
        "should_convert_jsx_events" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_convert_rest_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_destructure_args" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_extract_single_qrl" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_extract_single_qrl_2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_extract_single_qrl_with_index" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_extract_single_qrl_with_nested_components" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_handle_dangerously_set_inner_html" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_ignore_null_inlined_qrl" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_mark_props_as_var_props_for_inner_cmp" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_attributes_with_spread_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_attributes_with_spread_props_before_and_after" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_bind_checked_and_on_input" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_bind_value_and_on_input" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_on_input_and_bind_checked" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_merge_on_input_and_bind_value" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_move_bind_value_to_var_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_move_props_related_to_iteration_variables_to_var_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_generate_conflicting_props_identifiers" => {
            case.entry_strategy = EntryStrategy::Hoist;
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_move_over_side_effects" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_transform_bind_checked_in_var_props_for_jsx_split" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_transform_bind_value_in_var_props_for_jsx_split" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_wrap_fn" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_wrap_ternary_function_operator_with_fn" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_not_wrap_var_template_string" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props_with_additional_prop" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props_with_additional_prop2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props_with_additional_prop3" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props_with_additional_prop4" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_split_spread_props_with_additional_prop5" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_transform_component_with_normal_function" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_transform_multiple_event_handlers" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_transform_multiple_event_handlers_case2" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_transform_nested_loops" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_transform_qrls_in_ternary_expression" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_inner_inline_component_prop" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_logical_expression_in_template" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_object_with_fn_signal" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_prop_from_destructured_array" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_store_expression" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "should_wrap_type_asserted_variables_in_template" => {
            case.transpile_ts = true;
            case.transpile_jsx = true;
        }
        "support_windows_paths" => {
            case.filename = "components\\apps\\apps.tsx".to_string();
            case.src_dir = "C:\\users\\apps".to_string();
            case.transpile_jsx = true;
            case.is_server = Some(false);
        }
        "ternary_prop" => {
            case.transpile_jsx = true;
        }
        "transform_qrl_in_regular_prop" => {
            case.transpile_jsx = true;
        }
        _ => {}
    }
}
