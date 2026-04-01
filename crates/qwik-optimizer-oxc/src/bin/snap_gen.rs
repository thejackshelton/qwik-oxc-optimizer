//! snap-gen — Generate `.snap` files from fixtures.json using the OXC optimizer.
//!
//! Usage:
//!   snap-gen --fixtures <path> --output <dir>
//!
//! Reads fixtures.json, runs transform_modules on each fixture, and emits
//! `.snap` files in the format expected by the TypeScript test harness.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use qwik_optimizer_oxc::{
    EmitMode, EntryStrategy, MinifyMode, TransformModuleInput, TransformModulesOptions,
    TransformOutput, SegmentAnalysis,
};

// ---------------------------------------------------------------------------
// fixtures.json deserialization types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FixturesFile {
    #[allow(dead_code)]
    version: u32,
    fixtures: HashMap<String, FixtureConfig>,
}

#[derive(Debug, Deserialize)]
struct FixtureConfig {
    src_dir: String,
    #[serde(default)]
    root_dir: Option<String>,
    #[serde(default)]
    source_maps: bool,
    #[serde(default = "default_minify")]
    minify: String,
    #[serde(default)]
    transpile_ts: bool,
    #[serde(default)]
    transpile_jsx: bool,
    #[serde(default)]
    preserve_filenames: bool,
    #[serde(default)]
    explicit_extensions: bool,
    #[serde(default = "default_entry_strategy")]
    entry_strategy: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    scope: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    core_module: Option<String>,
    #[serde(default)]
    strip_exports: Option<Vec<String>>,
    #[serde(default)]
    strip_ctx_name: Option<Vec<String>>,
    #[serde(default)]
    strip_event_handlers: bool,
    #[serde(default)]
    reg_ctx_name: Option<Vec<String>>,
    #[serde(default)]
    is_server: Option<bool>,
    #[serde(default)]
    manual_chunks: Option<HashMap<String, String>>,
    inputs: Vec<FixtureInput>,
}

#[derive(Debug, Deserialize)]
struct FixtureInput {
    path: String,
    #[allow(dead_code)]
    #[serde(default)]
    dev_path: Option<String>,
    code: String,
}

fn default_minify() -> String {
    "Simplify".to_string()
}

fn default_entry_strategy() -> String {
    "Segment".to_string()
}

fn default_mode() -> String {
    "Lib".to_string()
}

// ---------------------------------------------------------------------------
// String → enum mapping (plain string → library enum)
// ---------------------------------------------------------------------------

fn parse_mode(s: &str) -> Result<EmitMode, String> {
    match s {
        "Test" => Ok(EmitMode::Test),
        "Prod" => Ok(EmitMode::Prod),
        "Dev" => Ok(EmitMode::Dev),
        "Lib" => Ok(EmitMode::Lib),
        "Hmr" => Ok(EmitMode::Hmr),
        _ => Err(format!("unknown mode: {}", s)),
    }
}

fn parse_entry_strategy(s: &str) -> Result<EntryStrategy, String> {
    match s {
        "Segment" => Ok(EntryStrategy::Segment),
        "Inline" => Ok(EntryStrategy::Inline),
        "Hoist" => Ok(EntryStrategy::Hoist),
        "Single" => Ok(EntryStrategy::Single),
        "Hook" => Ok(EntryStrategy::Hook),
        "Component" => Ok(EntryStrategy::Component),
        "Smart" => Ok(EntryStrategy::Smart),
        _ => Err(format!("unknown entry_strategy: {}", s)),
    }
}

fn parse_minify(s: &str) -> Result<MinifyMode, String> {
    match s {
        "Simplify" => Ok(MinifyMode::Simplify),
        "None" => Ok(MinifyMode::None),
        _ => Err(format!("unknown minify: {}", s)),
    }
}

// ---------------------------------------------------------------------------
// Build TransformModulesOptions from FixtureConfig
// ---------------------------------------------------------------------------

fn build_options(config: &FixtureConfig) -> Result<TransformModulesOptions, String> {
    let mode = parse_mode(&config.mode)?;
    let entry_strategy = parse_entry_strategy(&config.entry_strategy)?;
    let minify = parse_minify(&config.minify)?;

    let input: Vec<TransformModuleInput> = config
        .inputs
        .iter()
        .map(|i| TransformModuleInput {
            path: i.path.clone(),
            code: i.code.clone(),
            dev_path: None,
        })
        .collect();

    Ok(TransformModulesOptions {
        src_dir: config.src_dir.clone(),
        root_dir: config.root_dir.clone(),
        input,
        source_maps: config.source_maps,
        minify,
        transpile_ts: config.transpile_ts,
        transpile_jsx: config.transpile_jsx,
        preserve_filenames: config.preserve_filenames,
        entry_strategy,
        explicit_extensions: config.explicit_extensions,
        mode,
        scope: config.scope.clone(),
        core_module: config.core_module.clone(),
        strip_exports: config.strip_exports.clone(),
        strip_ctx_name: config.strip_ctx_name.clone(),
        strip_event_handlers: config.strip_event_handlers,
        reg_ctx_name: config.reg_ctx_name.clone(),
        is_server: config.is_server,
    })
}

// ---------------------------------------------------------------------------
// emit_snap — format and write a .snap file
// ---------------------------------------------------------------------------

/// Escape a JSON string for embedding in `Some("...")` source map lines.
/// Only escapes inner `"` to `\"`. The value from the optimizer is already
/// a JSON string (no newlines), so this is the only required escape.
fn escape_source_map(json: &str) -> String {
    json.replace('"', "\\\"")
}

/// Format a SegmentAnalysis as pretty JSON for the metadata block.
fn format_segment_json(segment: &SegmentAnalysis) -> String {
    serde_json::to_string_pretty(segment).expect("SegmentAnalysis serialization cannot fail")
}

fn emit_snap(
    name: &str,
    input_code: &str,
    output: &TransformOutput,
    out_dir: &Path,
) -> Result<(), anyhow::Error> {
    let mut buf = String::new();

    // Frontmatter
    buf.push_str("---\n");
    buf.push_str("source: packages/optimizer/core/src/test.rs\n");
    buf.push_str("assertion_line: 0\n");
    buf.push_str("expression: output\n");
    buf.push_str("---\n");

    // Input section
    buf.push_str("==INPUT==\n\n");
    buf.push_str(input_code);
    buf.push('\n');

    // Module sections ordered by module.order (already sorted by transform_modules)
    for module in &output.modules {
        if module.is_entry {
            // Segment / entry point
            buf.push_str(&format!(
                "\n============================= {} (ENTRY POINT)==\n\n",
                module.path
            ));
            buf.push_str(&module.code);
            buf.push_str("\n\n");
            if let Some(ref map) = module.map {
                buf.push_str(&format!("Some(\"{}\")\n", escape_source_map(map)));
            }
            if let Some(ref segment) = module.segment {
                buf.push_str("/*\n");
                buf.push_str(&format_segment_json(segment));
                buf.push_str("\n*/\n");
            }
        } else {
            // Root module (non-segment)
            buf.push_str(&format!(
                "\n============================= {} ==\n\n",
                module.path
            ));
            buf.push_str(&module.code);
            buf.push_str("\n\n");
            if let Some(ref map) = module.map {
                buf.push_str(&format!("Some(\"{}\")\n", escape_source_map(map)));
            }
            // No metadata block for root modules
        }
    }

    // Diagnostics section (always last)
    buf.push_str("== DIAGNOSTICS ==\n\n");
    let diag_json = serde_json::to_string_pretty(&output.diagnostics)
        .expect("diagnostics serialization cannot fail");
    buf.push_str(&diag_json);
    buf.push('\n');

    // Write to file
    let snap_path = out_dir.join(format!("{}.snap", name));
    fs::write(&snap_path, buf)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut fixtures_path: Option<PathBuf> = None;
    let mut output_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fixtures" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --fixtures requires a value");
                    std::process::exit(2);
                }
                fixtures_path = Some(PathBuf::from(&args[i]));
            }
            "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --output requires a value");
                    std::process::exit(2);
                }
                output_dir = Some(PathBuf::from(&args[i]));
            }
            arg => {
                eprintln!("error: unknown argument: {}", arg);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let fixtures_path = fixtures_path.unwrap_or_else(|| {
        eprintln!("error: --fixtures <path> is required");
        std::process::exit(2);
    });

    let output_dir = output_dir.unwrap_or_else(|| {
        eprintln!("error: --output <dir> is required");
        std::process::exit(2);
    });

    // Create output directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(&output_dir) {
        eprintln!("error: failed to create output directory: {}", e);
        std::process::exit(1);
    }

    // Read and parse fixtures.json
    let fixtures_content = match fs::read_to_string(&fixtures_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to read fixtures file {:?}: {}", fixtures_path, e);
            std::process::exit(1);
        }
    };

    let fixtures_file: FixturesFile = match serde_json::from_str(&fixtures_content) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: failed to parse fixtures JSON: {}", e);
            std::process::exit(1);
        }
    };

    // Sort fixture names alphabetically for deterministic processing
    let mut fixture_names: Vec<String> = fixtures_file.fixtures.keys().cloned().collect();
    fixture_names.sort();

    let total = fixture_names.len();
    let mut failures: Vec<String> = Vec::new();

    for (idx, fixture_name) in fixture_names.iter().enumerate() {
        eprintln!("[{}/{}] {}", idx + 1, total, fixture_name);

        let config = &fixtures_file.fixtures[fixture_name];

        // Get input code (use first input's code for the ==INPUT== section)
        let input_code = config.inputs.first().map(|i| i.code.as_str()).unwrap_or("");

        // Build transform options
        let opts = match build_options(config) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("  error building options for {}: {}", fixture_name, e);
                failures.push(fixture_name.clone());
                continue;
            }
        };

        // Run optimizer
        let output = match qwik_optimizer_oxc::transform_modules(opts) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("  error transforming {}: {}", fixture_name, e);
                failures.push(fixture_name.clone());
                continue;
            }
        };

        // Emit snap file
        if let Err(e) = emit_snap(fixture_name, input_code, &output, &output_dir) {
            eprintln!("  error writing snap for {}: {}", fixture_name, e);
            failures.push(fixture_name.clone());
        }
    }

    if failures.is_empty() {
        eprintln!("All {} fixtures processed successfully.", total);
        std::process::exit(0);
    } else {
        eprintln!("{} fixture(s) failed:", failures.len());
        for f in &failures {
            eprintln!("  - {}", f);
        }
        std::process::exit(1);
    }
}
