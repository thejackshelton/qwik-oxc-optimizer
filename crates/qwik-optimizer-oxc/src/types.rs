//! All public and internal type definitions for the qwik-optimizer-oxc crate.
//!
//! This is a pure data module with no logic -- only structs, enums, derives,
//! and serde attributes. Separating types into their own module prevents
//! circular dependencies since every other module can import from `types`
//! without importing logic.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public Types
// ---------------------------------------------------------------------------

/// Top-level configuration for transforming one or more modules.
///
/// SWC equivalent: TransformModulesOptions in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformModulesOptions {
    /// Root directory for resolving relative paths.
    pub src_dir: String,

    /// Optional root directory override (used for monorepo setups).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_dir: Option<String>,

    /// List of input modules to transform.
    pub input: Vec<TransformModuleInput>,

    /// Whether to generate source maps.
    /// Default: true
    #[serde(default = "default_true")]
    pub source_maps: bool,

    /// Minification mode.
    /// Default: MinifyMode::Simplify
    #[serde(default)]
    pub minify: MinifyMode,

    /// Whether to strip TypeScript type annotations.
    /// Default: false
    #[serde(default)]
    pub transpile_ts: bool,

    /// Whether to transpile JSX to function calls.
    /// Default: false
    #[serde(default)]
    pub transpile_jsx: bool,

    /// Whether to preserve original filenames in output paths.
    /// Default: false
    #[serde(default)]
    pub preserve_filenames: bool,

    /// How to split extracted segments into output modules.
    /// Default: EntryStrategy::Segment
    #[serde(default)]
    pub entry_strategy: EntryStrategy,

    /// Whether to use explicit file extensions in import paths.
    /// Default: false
    #[serde(default)]
    pub explicit_extensions: bool,

    /// Output mode controlling which build target to emit for.
    /// Default: EmitMode::Lib
    #[serde(default)]
    pub mode: EmitMode,

    /// Optional scope prefix for segment names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// Override the core module import path (default: "@qwik.dev/core").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_module: Option<String>,

    /// List of export names to strip from output.
    /// Used for server/client-specific builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip_exports: Option<Vec<String>>,

    /// List of ctx names to strip (e.g., strip all "useTask$" segments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip_ctx_name: Option<Vec<String>>,

    /// Whether to strip event handler registrations.
    /// Default: false
    #[serde(default)]
    pub strip_event_handlers: bool,

    /// List of ctx names to register (for plugin coordination).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_ctx_name: Option<Vec<String>>,

    /// Whether this build targets SSR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_server: Option<bool>,
}

fn default_true() -> bool {
    true
}

impl Default for TransformModulesOptions {
    fn default() -> Self {
        Self {
            src_dir: ".".to_string(),
            root_dir: None,
            input: vec![],
            source_maps: true,
            minify: MinifyMode::default(),
            transpile_ts: false,
            transpile_jsx: false,
            preserve_filenames: false,
            entry_strategy: EntryStrategy::default(),
            explicit_extensions: false,
            mode: EmitMode::default(),
            scope: None,
            core_module: None,
            strip_exports: None,
            strip_ctx_name: None,
            strip_event_handlers: false,
            reg_ctx_name: None,
            is_server: None,
        }
    }
}

/// A single input file to transform.
///
/// SWC equivalent: TransformModuleInput in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformModuleInput {
    /// The source code content.
    pub code: String,

    /// The file path (relative to src_dir).
    pub path: String,

    /// Optional development path override (for HMR/dev mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev_path: Option<String>,
}

/// Complete result of transforming one or more modules.
///
/// SWC equivalent: TransformOutput in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformOutput {
    /// All output modules: main modules + extracted segments.
    pub modules: Vec<TransformModule>,

    /// Diagnostics (errors, warnings) from transformation.
    pub diagnostics: Vec<Diagnostic>,

    /// Whether any input was TypeScript.
    pub is_type_script: bool,

    /// Whether any input contained JSX.
    pub is_jsx: bool,
}

/// A single output module (either the transformed main module or an extracted segment).
///
/// SWC equivalent: TransformModule in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformModule {
    /// Output file path (relative to src_dir).
    pub path: String,

    /// Whether this module is an entry point (segment modules are entry points).
    pub is_entry: bool,

    /// The generated JavaScript source code.
    pub code: String,

    /// Optional source map (JSON string, base64-encoded if inline).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,

    /// Segment metadata, present only for extracted segment modules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment: Option<SegmentAnalysis>,

    /// Original input file path (before transformation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,

    /// Sort order for deterministic output ordering.
    /// Main modules get order 0; segments get their extraction order.
    /// u64 to match SPEC.md (supports large segment counts).
    #[serde(default)]
    pub order: u64,
}

/// Metadata about an extracted segment (a lazy-loadable code fragment).
///
/// SWC equivalent: HookAnalysis/SegmentAnalysis in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentAnalysis {
    /// Source file this segment was extracted from.
    pub origin: String,

    /// Full segment name including hash (e.g., "renderHeader_zBbHWn4e8Cg").
    pub name: String,

    /// Entry point name, if this segment is a named entry.
    /// Always serialized (null when None) to match SWC golden format.
    pub entry: Option<String>,

    /// Human-readable display name (e.g., "test.tsx_renderHeader").
    pub display_name: String,

    /// The 11-character hash (e.g., "zBbHWn4e8Cg").
    pub hash: String,

    /// Canonical filename for the segment module (e.g., "test.tsx_renderHeader_zBbHWn4e8Cg").
    pub canonical_filename: String,

    /// Output path prefix (empty string if same directory).
    pub path: String,

    /// File extension of the output (e.g., "tsx", "ts", "js").
    pub extension: String,

    /// Parent segment name, if this segment is nested inside another.
    /// Always serialized (null when None) to match SWC golden format.
    pub parent: Option<String>,

    /// Context kind: whether this is an event handler or a function.
    pub ctx_kind: CtxKind,

    /// Context name: the $-suffixed function that created this segment
    /// (e.g., "$", "component$", "useTask$").
    pub ctx_name: String,

    /// Whether this segment captures variables from its enclosing scope.
    pub captures: bool,

    /// Names of captured variables (for inlinedQrl/qrl capture arrays).
    /// Only present when captures is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_names: Option<Vec<String>>,

    /// Source location as [start_byte, end_byte] of the original $-call.
    /// Serializes as a JSON array [start, end] (Rust tuples serialize as arrays).
    pub loc: (u32, u32),

    /// Parameter names after props destructuring transformation.
    /// E.g., `["_rawProps"]` for component$ with destructured props.
    /// Only present for component$ segments with destructured props.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param_names: Option<Vec<String>>,
}

/// Controls how extracted segments are output.
///
/// SWC equivalent: EntryStrategy in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum EntryStrategy {
    /// Each segment becomes a separate file with a lazy import.
    /// This is the default strategy.
    Segment,

    /// Segments stay in the same file with inlinedQrl wrappers.
    Inline,

    /// Segments are hoisted to the top of the module.
    Hoist,

    /// All segments go into a single output file.
    Single,

    /// Group segments by their parent component.
    Component,

    /// Automatically choose the best strategy based on usage patterns.
    Smart,

    /// Group segments by their hook type.
    Hook,
}

impl Default for EntryStrategy {
    fn default() -> Self {
        EntryStrategy::Segment
    }
}

/// Controls output minification.
///
/// SWC equivalent: MinifyMode in types.ts
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MinifyMode {
    /// Apply simplification transforms (dead code elimination, constant folding).
    Simplify,

    /// No minification.
    None,
}

impl Default for MinifyMode {
    fn default() -> Self {
        MinifyMode::Simplify
    }
}

/// Controls the build target output mode.
///
/// SWC equivalent: EmitMode in types.ts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EmitMode {
    /// Library mode -- standard output.
    Lib,

    /// Production mode -- optimized output (uses short `s_{hash}` symbol names).
    Prod,

    /// Development mode -- includes debug info.
    Dev,

    /// Hot Module Replacement mode -- development mode with HMR-specific transforms.
    Hmr,

    /// Test mode -- for unit test environments.
    Test,
}

impl Default for EmitMode {
    fn default() -> Self {
        EmitMode::Lib
    }
}

/// The kind of context that created a segment.
///
/// In spec file JSON, this appears as "eventHandler", "function", or "jsxProp".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CtxKind {
    /// Event handler context (e.g., onClick$, onInput$).
    #[serde(rename = "eventHandler")]
    EventHandler,

    /// Function context (e.g., $, component$, useTask$).
    #[serde(rename = "function")]
    Function,

    /// JSX prop expression context (e.g., inline JSX prop expressions).
    #[serde(rename = "jsxProp")]
    JSXProp,
}

/// A diagnostic message from the transformation process.
///
/// SWC equivalent: Diagnostic in types.ts
/// Field ORDER matches SWC wire format: category, code, file, message, highlights, suggestions, scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// The diagnostic category.
    pub category: DiagnosticCategory,

    /// Machine-readable error code.
    pub code: Option<String>,

    /// File path where the diagnostic originated.
    pub file: String,

    /// Human-readable message.
    pub message: String,

    /// Optional source code highlights.
    /// Note: always serializes as null (not omitted) to match SWC wire format.
    pub highlights: Option<Vec<SourceLocation>>,

    /// Optional fix suggestions.
    /// Note: always serializes as null (not omitted) to match SWC wire format.
    pub suggestions: Option<Vec<String>>,

    /// Scope identifier matching SWC wire format (always "optimizer").
    pub scope: String,
}

/// Severity level of a diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticCategory {
    /// A hard error that prevents successful transformation.
    Error,

    /// A warning that does not prevent transformation.
    Warning,

    /// An error originating from the source code (e.g., syntax error).
    SourceError,
}

/// A source location for diagnostic highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    /// Starting byte offset.
    pub lo: u32,

    /// Ending byte offset.
    pub hi: u32,

    /// Starting line (1-indexed).
    pub start_line: u32,

    /// Starting column (0-indexed).
    pub start_col: u32,

    /// Ending line (1-indexed).
    pub end_line: u32,

    /// Ending column (0-indexed).
    pub end_col: u32,
}

/// Top-level manifest emitted alongside transformed output.
///
/// Maps segment symbol names to their analysis and groups them into bundles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QwikManifest {
    /// Manifest format version (always "1").
    pub version: String,

    /// Map from symbol name to segment metadata.
    pub symbols: HashMap<String, SegmentAnalysis>,

    /// Map from bundle filename to bundle metadata.
    pub bundles: HashMap<String, QwikBundle>,

    /// Map from segment canonical filename to bundle filename.
    pub mapping: HashMap<String, String>,
}

/// Metadata for a single output bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QwikBundle {
    /// Approximate bundle size in bytes.
    pub size: usize,

    /// List of symbol names in this bundle.
    pub symbols: Vec<String>,
}

// ---------------------------------------------------------------------------
// Internal Types (pub(crate))
// ---------------------------------------------------------------------------

/// Result of the collector pass -- everything discovered about the module
/// before transformation begins.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CollectResult {
    /// Set of known $-suffixed imports from @qwik.dev/core (or custom core_module).
    /// Contains LOCAL names. e.g., {"$", "component$", "useTask$"} or {"Component", "onRender"} for aliases.
    pub dollar_imports: HashSet<String>,

    /// Alias map: local_name -> original_imported_name for $-suffixed imports.
    /// Only populated when the local name differs from the imported name.
    /// e.g., {"Component" -> "component$", "onRender" -> "$"}
    pub alias_map: HashMap<String, String>,

    /// Located $-call sites with span info.
    pub dollar_calls: Vec<DollarCallSite>,

    /// All import declarations in the module.
    pub module_imports: Vec<ImportInfo>,

    /// All export declarations in the module.
    pub module_exports: Vec<ExportInfo>,

    /// Names declared at module (top-level) scope.
    /// These are NOT captures -- they're available in the module scope and don't
    /// need serialization through `_captures`. Includes variable declarations,
    /// function declarations, and class declarations at the top level.
    pub module_level_decls: HashSet<String>,
}

/// A located $-call site in the source code.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DollarCallSite {
    /// The name of the callee (e.g., "$", "component$").
    pub callee_name: String,

    /// Byte offset span of the entire call expression.
    pub span: (u32, u32),

    /// The display name derived from the lexical context
    /// (e.g., "Header_component" for `const Header = component$(...)`).
    pub display_name: String,

    /// Whether this is a nested $-call (inside another $-call's body).
    pub is_nested: bool,

    /// The parent $-call's display name, if nested.
    pub parent_name: Option<String>,
}

/// The kind of import specifier (default, namespace, or named).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportKind {
    /// Default import: `import dep3 from "source"`
    Default,
    /// Namespace import: `import * as dep2 from "source"`
    Namespace,
    /// Named import: `import { foo } from "source"` or `import { bar as bbar } from "source"`
    Named,
}

/// Recorded import declaration from the source module.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ImportInfo {
    /// Import source (e.g., "@qwik.dev/core", "./utils").
    pub source: String,

    /// Named import specifiers (e.g., ["$", "component$", "useStore"]).
    /// These are LOCAL names (after any aliasing).
    pub specifiers: Vec<String>,

    /// The kind of each specifier (default, namespace, or named).
    /// Parallel to `specifiers` -- `specifier_kinds[i]` is the kind for `specifiers[i]`.
    pub specifier_kinds: Vec<ImportKind>,

    /// Mapping from local_name -> imported_name for aliased specifiers.
    /// Only contains entries where local != imported (e.g., "myServer" -> "isServer").
    /// Non-aliased specifiers are NOT in this map.
    pub specifier_aliases: HashMap<String, String>,

    /// Whether this imports from the Qwik core module.
    pub is_qwik_core: bool,

    /// Byte offset span of the import declaration.
    pub span: (u32, u32),
}

/// Recorded export declaration from the source module.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ExportInfo {
    /// Exported name (e.g., "renderHeader").
    pub name: String,

    /// Whether this is a re-export (export { X } from '...').
    pub is_reexport: bool,

    /// Byte offset span of the export declaration.
    pub span: (u32, u32),
}

/// Intermediate segment representation recorded during the transform pass.
/// This gets converted to SegmentAnalysis + a segment Program during code_move.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct SegmentData {
    /// The display name (e.g., "test.tsx_Header_component").
    pub display_name: String,

    /// The computed hash (e.g., "J4uyIhaBNR4").
    pub hash: String,

    /// The full segment name (e.g., "Header_component_J4uyIhaBNR4").
    pub name: String,

    /// The callee that created this segment (e.g., "$", "component$").
    pub ctx_name: String,

    /// Context kind (function or event handler).
    pub ctx_kind: CtxKind,

    /// Source file origin.
    pub origin: String,

    /// File extension.
    pub extension: String,

    /// Span of the original $-call expression.
    pub span: (u32, u32),

    /// Parent segment name, if nested.
    pub parent: Option<String>,

    /// Variables from the enclosing scope that are referenced inside this segment.
    /// Used by `SmartStrategy` to decide whether the segment is "pure" (no captures).
    pub scoped_idents: Vec<String>,

    /// Whether the segment body captures outer scope variables.
    pub captures: bool,

    /// Names of captured variables (for inlinedQrl's capture array).
    pub capture_names: Vec<String>,

    /// Imports needed by the segment body (e.g., useStore from @qwik.dev/core).
    pub needed_imports: Vec<ImportInfo>,

    /// Qrl-suffixed import names needed by this segment (e.g., "useStylesQrl").
    /// Populated during finalize_segments when nested $-calls are scoped to parent segments.
    pub segment_qrl_names: Vec<String>,

    /// The extracted function body expression span.
    /// In practice, this will be an index or key into the AST arena.
    pub body_span: (u32, u32),

    /// Parameter names after props destructuring transformation.
    /// E.g., `["_rawProps"]` for component$ with destructured props.
    pub param_names: Vec<String>,

    /// Serialized JavaScript code of the extracted segment body.
    /// For segment strategy, this is the arrow function body serialized to JS
    /// after all transforms (props destructuring, capture analysis) have been applied.
    /// For inline strategy, this is empty (body stays in main module).
    pub body_code: String,

    /// Lazy imports needed by this segment for its child $()-calls.
    /// Each entry: (child_hash, child_import_path).
    pub child_lazy_imports: Vec<(String, String)>,

    /// Whether this segment needs a qrl import (has child $()-calls).
    pub needs_qrl_import: bool,
}

/// Per-module options derived from TransformModulesOptions.
/// Passed to individual module transformations.
///
/// SPEC name: TransformCodeOptions
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TransformCodeOptions {
    pub src_dir: String,
    pub root_dir: Option<String>,
    pub source_maps: bool,
    pub minify: MinifyMode,
    pub transpile_ts: bool,
    pub transpile_jsx: bool,
    pub preserve_filenames: bool,
    pub entry_strategy: EntryStrategy,
    pub explicit_extensions: bool,
    pub mode: EmitMode,
    pub scope: Option<String>,
    /// Resolved to actual value, not Option (defaults to "@qwik.dev/core").
    pub core_module: String,
    pub strip_exports: Vec<String>,
    pub strip_ctx_name: Vec<String>,
    pub strip_event_handlers: bool,
    pub reg_ctx_name: Vec<String>,
    pub is_server: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_roundtrip() {
        let opts = TransformModulesOptions::default();
        let json = serde_json::to_string(&opts).unwrap();
        let deserialized: TransformModulesOptions = serde_json::from_str(&json).unwrap();

        assert_eq!(opts.src_dir, deserialized.src_dir);
        assert_eq!(opts.source_maps, deserialized.source_maps);
        assert!(opts.input.is_empty());
    }

    #[test]
    fn test_camel_case_serialization() {
        let opts = TransformModulesOptions {
            src_dir: "src".to_string(),
            root_dir: Some("/root".to_string()),
            input: vec![TransformModuleInput {
                code: "const x = 1;".to_string(),
                path: "test.tsx".to_string(),
                dev_path: None,
            }],
            source_maps: true,
            minify: MinifyMode::None,
            transpile_ts: false,
            transpile_jsx: true,
            preserve_filenames: false,
            entry_strategy: EntryStrategy::Inline,
            explicit_extensions: false,
            mode: EmitMode::Dev,
            scope: None,
            core_module: None,
            strip_exports: None,
            strip_ctx_name: None,
            strip_event_handlers: false,
            reg_ctx_name: None,
            is_server: Some(true),
        };

        let json = serde_json::to_string_pretty(&opts).unwrap();

        // Verify camelCase field names
        assert!(
            json.contains("\"srcDir\""),
            "Expected srcDir in JSON: {json}"
        );
        assert!(
            json.contains("\"rootDir\""),
            "Expected rootDir in JSON: {json}"
        );
        assert!(
            json.contains("\"sourceMaps\""),
            "Expected sourceMaps in JSON: {json}"
        );
        assert!(
            json.contains("\"transpileTs\""),
            "Expected transpileTs in JSON: {json}"
        );
        assert!(
            json.contains("\"transpileJsx\""),
            "Expected transpileJsx in JSON: {json}"
        );
        assert!(
            json.contains("\"preserveFilenames\""),
            "Expected preserveFilenames in JSON: {json}"
        );
        assert!(
            json.contains("\"entryStrategy\""),
            "Expected entryStrategy in JSON: {json}"
        );
        assert!(
            json.contains("\"explicitExtensions\""),
            "Expected explicitExtensions in JSON: {json}"
        );
        assert!(
            json.contains("\"isServer\""),
            "Expected isServer in JSON: {json}"
        );
        assert!(
            json.contains("\"stripEventHandlers\""),
            "Expected stripEventHandlers in JSON: {json}"
        );

        // Verify it round-trips
        let deserialized: TransformModulesOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.src_dir, "src");
        assert_eq!(deserialized.root_dir, Some("/root".to_string()));
        assert!(deserialized.transpile_jsx);
    }

    #[test]
    fn test_entry_strategy_tagged_serialization() {
        let strategy = EntryStrategy::Segment;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, r#"{"type":"segment"}"#);

        let strategy = EntryStrategy::Inline;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, r#"{"type":"inline"}"#);

        // Round-trip
        let deserialized: EntryStrategy = serde_json::from_str(r#"{"type":"segment"}"#).unwrap();
        assert!(matches!(deserialized, EntryStrategy::Segment));
    }

    #[test]
    fn test_ctx_kind_serialization() {
        let kind = CtxKind::EventHandler;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""eventHandler""#);

        let kind = CtxKind::Function;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""function""#);

        // Round-trip
        let deserialized: CtxKind = serde_json::from_str(r#""function""#).unwrap();
        assert!(matches!(deserialized, CtxKind::Function));
    }

    #[test]
    fn test_ctx_kind_jsx_prop_serialization() {
        let kind = CtxKind::JSXProp;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""jsxProp""#, "JSXProp must serialize as 'jsxProp'");

        // Round-trip
        let deserialized: CtxKind = serde_json::from_str(r#""jsxProp""#).unwrap();
        assert!(matches!(deserialized, CtxKind::JSXProp));
    }

    #[test]
    fn test_minify_mode_serialization() {
        let mode = MinifyMode::Simplify;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""simplify""#);

        let mode = MinifyMode::None;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""none""#);
    }

    #[test]
    fn test_emit_mode_serialization() {
        let mode = EmitMode::Lib;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""lib""#);

        let mode = EmitMode::Prod;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""prod""#);

        let mode = EmitMode::Dev;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""dev""#);

        // New variants required by SPEC FND-03
        let mode = EmitMode::Hmr;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""hmr""#, "Hmr must serialize as 'hmr'");

        let mode = EmitMode::Test;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""test""#, "Test must serialize as 'test'");
    }

    #[test]
    fn test_emit_mode_hmr_roundtrip() {
        let deserialized: EmitMode = serde_json::from_str(r#""hmr""#).unwrap();
        assert_eq!(deserialized, EmitMode::Hmr);
    }

    #[test]
    fn test_emit_mode_test_roundtrip() {
        let deserialized: EmitMode = serde_json::from_str(r#""test""#).unwrap();
        assert_eq!(deserialized, EmitMode::Test);
    }

    #[test]
    fn test_segment_analysis_loc_serialization() {
        let segment = SegmentAnalysis {
            origin: "test.tsx".to_string(),
            name: "renderHeader_zBbHWn4e8Cg".to_string(),
            entry: None,
            display_name: "test.tsx_renderHeader".to_string(),
            hash: "zBbHWn4e8Cg".to_string(),
            canonical_filename: "test.tsx_renderHeader_zBbHWn4e8Cg".to_string(),
            path: "".to_string(),
            extension: "tsx".to_string(),
            parent: None,
            ctx_kind: CtxKind::Function,
            ctx_name: "$".to_string(),
            captures: false,
            capture_names: None,
            loc: (90, 161),
            param_names: None,
        };

        let json = serde_json::to_string(&segment).unwrap();
        // loc should serialize as [90, 161]
        assert!(
            json.contains(r#""loc":[90,161]"#),
            "Expected loc as array: {json}"
        );

        // Verify round-trip
        let deserialized: SegmentAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.loc, (90, 161));
        assert_eq!(deserialized.origin, "test.tsx");
        assert!(matches!(deserialized.ctx_kind, CtxKind::Function));
    }

    #[test]
    fn test_transform_output_roundtrip() {
        let output = TransformOutput {
            modules: vec![TransformModule {
                path: "test.tsx".to_string(),
                is_entry: false,
                code: "const x = 1;".to_string(),
                map: None,
                segment: None,
                orig_path: Some("test.tsx".to_string()),
                order: 0,
            }],
            diagnostics: vec![],
            is_type_script: true,
            is_jsx: false,
        };

        let json = serde_json::to_string(&output).unwrap();
        let deserialized: TransformOutput = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.modules.len(), 1);
        assert_eq!(deserialized.modules[0].path, "test.tsx");
        assert!(deserialized.is_type_script);
        assert!(!deserialized.is_jsx);
        assert!(
            json.contains("\"isTypeScript\""),
            "Expected isTypeScript in JSON: {json}"
        );
        assert!(json.contains("\"isJsx\""), "Expected isJsx in JSON: {json}");
        assert!(
            json.contains("\"isEntry\""),
            "Expected isEntry in JSON: {json}"
        );
    }

    #[test]
    fn test_defaults() {
        assert!(matches!(EntryStrategy::default(), EntryStrategy::Segment));
        assert!(matches!(MinifyMode::default(), MinifyMode::Simplify));
        assert!(matches!(EmitMode::default(), EmitMode::Lib));

        let opts = TransformModulesOptions::default();
        assert_eq!(opts.src_dir, ".");
        assert!(opts.source_maps);
        assert!(!opts.transpile_ts);
        assert!(!opts.transpile_jsx);
        assert!(opts.input.is_empty());
    }

    #[test]
    fn test_transform_module_input_dev_path() {
        let input = TransformModuleInput {
            code: "const x = 1;".to_string(),
            path: "src/component.tsx".to_string(),
            dev_path: Some("src/component.tsx?dev".to_string()),
        };

        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"devPath\""), "Expected devPath in JSON: {json}");

        // Without dev_path, field should be absent
        let input_no_dev = TransformModuleInput {
            code: "const x = 1;".to_string(),
            path: "src/component.tsx".to_string(),
            dev_path: None,
        };
        let json_no_dev = serde_json::to_string(&input_no_dev).unwrap();
        assert!(!json_no_dev.contains("devPath"), "devPath should be absent when None: {json_no_dev}");
    }

    #[test]
    fn test_qwik_manifest_roundtrip() {
        let mut symbols = HashMap::new();
        symbols.insert(
            "renderHeader_zBbHWn4e8Cg".to_string(),
            SegmentAnalysis {
                origin: "test.tsx".to_string(),
                name: "renderHeader_zBbHWn4e8Cg".to_string(),
                entry: None,
                display_name: "test.tsx_renderHeader".to_string(),
                hash: "zBbHWn4e8Cg".to_string(),
                canonical_filename: "test.tsx_renderHeader_zBbHWn4e8Cg".to_string(),
                path: "".to_string(),
                extension: "tsx".to_string(),
                parent: None,
                ctx_kind: CtxKind::Function,
                ctx_name: "$".to_string(),
                captures: false,
                capture_names: None,
                loc: (10, 50),
                param_names: None,
            },
        );

        let mut bundles = HashMap::new();
        bundles.insert(
            "renderHeader_zBbHWn4e8Cg.js".to_string(),
            QwikBundle {
                size: 256,
                symbols: vec!["renderHeader_zBbHWn4e8Cg".to_string()],
            },
        );

        let mut mapping = HashMap::new();
        mapping.insert(
            "test.tsx_renderHeader_zBbHWn4e8Cg".to_string(),
            "renderHeader_zBbHWn4e8Cg.js".to_string(),
        );

        let manifest = QwikManifest {
            version: "1".to_string(),
            symbols,
            bundles,
            mapping,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: QwikManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.version, "1");
        assert_eq!(deserialized.symbols.len(), 1);
        assert_eq!(deserialized.bundles.len(), 1);
        assert_eq!(deserialized.mapping.len(), 1);

        let bundle = deserialized.bundles.get("renderHeader_zBbHWn4e8Cg.js").unwrap();
        assert_eq!(bundle.size, 256);
        assert_eq!(bundle.symbols, vec!["renderHeader_zBbHWn4e8Cg"]);
    }

    #[test]
    fn test_qwik_bundle_roundtrip() {
        let bundle = QwikBundle {
            size: 1024,
            symbols: vec![
                "foo_abc123".to_string(),
                "bar_def456".to_string(),
            ],
        };

        let json = serde_json::to_string(&bundle).unwrap();
        let deserialized: QwikBundle = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.size, 1024);
        assert_eq!(deserialized.symbols.len(), 2);
        assert_eq!(deserialized.symbols[0], "foo_abc123");

        // Verify camelCase fields are present
        assert!(json.contains("\"size\""), "Expected size in JSON: {json}");
        assert!(json.contains("\"symbols\""), "Expected symbols in JSON: {json}");
    }
}
