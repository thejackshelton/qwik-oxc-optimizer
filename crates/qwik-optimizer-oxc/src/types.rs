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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmitMode {
    /// Library mode -- standard output.
    Lib,

    /// Production mode -- optimized output.
    Prod,

    /// Development mode -- includes debug info.
    Dev,
}

impl Default for EmitMode {
    fn default() -> Self {
        EmitMode::Lib
    }
}

/// The kind of context that created a segment.
///
/// In spec file JSON, this appears as "eventHandler" or "function".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CtxKind {
    /// Event handler context (e.g., onClick$, onInput$).
    #[serde(rename = "eventHandler")]
    EventHandler,

    /// Function context (e.g., $, component$, useTask$).
    #[serde(rename = "function")]
    Function,

    /// JSX prop context: function expression in a $-suffixed JSX prop on a component element.
    /// Serialized as "jSXProp" to match SWC's camelCase serialization of the JSX prefix.
    #[serde(rename = "jSXProp")]
    JSXProp,
}

/// A diagnostic message from the transformation process.
///
/// SWC equivalent: Diagnostic in types.ts
/// Field order matches SWC's serialization: category, code, file, message, highlights, suggestions, scope
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
    pub highlights: Option<Vec<SourceLocation>>,

    /// Optional fix suggestions.
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

    /// All import declarations in the module.
    pub module_imports: Vec<ImportInfo>,

    /// All export declarations in the module.
    pub module_exports: Vec<ExportInfo>,

    /// Names declared at module (top-level) scope.
    /// These are NOT captures -- they're available in the module scope and don't
    /// need serialization through `_captures`. Includes variable declarations,
    /// function declarations, and class declarations at the top level.
    pub module_level_decls: HashSet<String>,

    /// Local binding names that are user-exported (via export const/function/class
    /// or export { X }). Used to determine which module-level decls need `_auto_`
    /// prefix when re-exported for segment self-imports.
    /// Does NOT include names from `export default` declarations.
    pub exported_local_names: HashSet<String>,
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

    /// Import assertion/attribute clause, e.g., `with { type: "json" }`.
    /// Stored as key-value pairs: `[("type", "json")]`.
    pub assertion: Vec<(String, String)>,
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

    /// Stack context names at the time of segment creation.
    /// Used by Smart/Component entry strategies to compute entry field.
    pub stack_ctxt: Vec<String>,
}

/// Per-module options derived from TransformModulesOptions.
/// Passed to individual module transformations.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TransformOptions {
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
