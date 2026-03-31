//! Main QwikTransform traverse implementation.
//!
//! The core of the optimizer. Implements the `Traverse` trait to walk the AST
//! and apply all Qwik transformations: replace `$()` calls with `qrl()` wrappers,
//! record segments for extraction, rewrite imports, and handle special patterns
//! (component$, useTask$, etc.).

use std::collections::{HashMap, HashSet};

use oxc::ast::ast::*;
use oxc::ast::Comment;
use oxc::span::SPAN;
use oxc_traverse::{Traverse, TraverseCtx};

use crate::collector;
use crate::entry_strategy;
use crate::hash;
use crate::import_rewrite;
use crate::jsx_transform::{
    get_jsx_lambda_span, transform_jsx_element_inner,
    transform_jsx_fragment_inner,
};
use crate::props_destructuring::{self, PropsDestructuringInfo};
use crate::types::{CollectResult, Diagnostic, SegmentData, TransformOptions};
use crate::words;

/// Identifies the kind of $-call detected.
#[derive(Debug, Clone)]
pub(crate) enum DollarCallKind {
    /// Raw $() call: $(() => { ... })
    RawDollar,
    /// Named $-suffixed call: component$(() => { ... })
    Named(String),
}

/// Tracks which imports are needed for the transformed module.
#[derive(Debug, Default)]
pub(crate) struct ImportTracker {
    /// Qrl-suffixed imports needed: "componentQrl", "useStylesQrl", etc.
    pub qrl_imports: Vec<String>,

    /// Whether the module needs `import { qrl }` (segment strategy).
    pub needs_qrl: bool,

    /// Whether the module needs `import { inlinedQrl }` (inline strategy).
    pub needs_inlined_qrl: bool,

    /// Whether the module needs `import { _captures }` (inline + captures).
    pub needs_captures: bool,

    /// Whether the module needs `import { _restProps }` (props destructuring with rest).
    pub needs_rest_props: bool,

    /// Lazy import constants to insert (segment strategy).
    /// Each entry: (hash, import_path).
    pub lazy_imports: Vec<(String, String)>,

    /// Whether the module needs `import { _jsxSorted }` from core.
    pub needs_jsx_sorted: bool,

    /// Whether the module needs `import { _jsxSplit }` from core.
    pub needs_jsx_split: bool,

    /// Whether the module needs `import { _getVarProps }` from core.
    pub needs_get_var_props: bool,

    /// Whether the module needs `import { _getConstProps }` from core.
    pub needs_get_const_props: bool,

    /// Whether the module needs `import { Fragment as _Fragment }` from jsx-runtime.
    pub needs_fragment: bool,

    /// Whether the module needs `import { _wrapProp }` from core.
    pub needs_wrap_prop: bool,

    /// Whether the module needs `import { _fnSignal }` from core.
    pub needs_fn_signal: bool,

    /// Whether the module needs `import { _val }` from core (bind:value).
    pub needs_val: bool,

    /// Whether the module needs `import { _chk }` from core (bind:checked).
    pub needs_chk: bool,

    /// Whether the module needs `import { _noopQrl }` from core (stripped ctx calls).
    pub needs_noop_qrl: bool,

    /// Whether the module needs `import { _qrlSync }` from core (sync$ calls).
    pub needs_qrl_sync: bool,

    /// Whether the module needs `import { createElement as _createElement }` from core.
    pub needs_create_element: bool,

    /// Custom JSX import source for `import { jsx as _jsx }` from `{source}/jsx-runtime`.
    /// When Some, `_jsx` is used instead of `_jsxSorted` for JSX transform output.
    pub custom_jsx_source: Option<String>,

    /// Monotonic counter for generating unique JSX key suffixes like "u6_0", "u6_1".
    pub jsx_key_counter: u32,

    /// Monotonic counter for hoisted function names (_hf0, _hf1, ...).
    pub hoisted_fn_counter: u32,

    /// Deduplication map for hoisted _fnSignal functions.
    /// Key: the arrow function source after parameter substitution (e.g., "(p0) => p0.errors.test").
    /// Value: the _hf index that was assigned.
    /// SWC deduplicates identical hoisted functions by their body string, so when
    /// multiple expressions like store.errors.test share the same parameterized body,
    /// only one _hfN is emitted and reused for all occurrences.
    pub hoisted_fn_dedup: std::collections::HashMap<String, u32>,

    /// Set of identifier names that are considered "immutable" component tags.
    /// Using these as JSX element tags does NOT set jsx_mutable = true.
    /// Built from imports: Fragment, RenderOnce, Link, and any import from ?jsx or .md sources.
    /// Mirrors SWC's immutable_function_cmp.
    pub immutable_function_cmp: HashSet<String>,

    /// Tracks whether the current JSX subtree has been marked as mutable.
    /// Set to true when mutable children or non-immutable component tags are encountered.
    /// Saved/restored around children processing to avoid leaking between siblings.
    /// Mirrors SWC's jsx_mutable.
    pub jsx_mutable: bool,

    /// Set of identifier names known to be const-bound (imports + const declarations).
    /// Used for scope-aware JSX prop/children immutability classification.
    /// Mirrors SWC's ConstCollector which tracks imports and const bindings.
    pub const_bindings: HashSet<String>,

    /// In dev mode, the fileName for JSX dev location metadata.
    /// When Some, `_jsxSorted` calls get an extra `{ fileName, lineNumber, columnNumber }` argument.
    pub jsx_dev_file_name: Option<String>,

    /// Source code for computing line/column from byte offsets (dev mode only).
    /// Stored as a String reference to avoid lifetime issues.
    pub jsx_dev_source_code: Option<String>,

    /// Records the order in which synthetic imports are first encountered during traversal.
    /// Used to emit imports in SWC's encounter order (BTreeMap<Id> by SyntaxContext)
    /// instead of alphabetical order. Each entry is an import name like "componentQrl",
    /// "_wrapProp", "_jsxSorted", etc. Duplicates are prevented by the record method.
    pub synthetic_import_order: Vec<String>,
}

impl ImportTracker {
    /// Record a synthetic import name in encounter order.
    /// Only records on first encounter (deduplicates).
    pub fn record_synthetic_import(&mut self, name: &str) {
        if !self.synthetic_import_order.iter().any(|n| n == name) {
            self.synthetic_import_order.push(name.to_string());
        }
    }
}

/// The core Qwik transform traversal state.
pub(crate) struct QwikTransform {
    options: TransformOptions,
    collected: CollectResult,
    filename: String,
    segments: Vec<SegmentData>,
    diagnostics: Vec<Diagnostic>,
    import_tracker: ImportTracker,
    segment_counter: u32,
    dollar_call_stack: Vec<String>,
    /// Set of span starts for dollar calls that we've already recorded,
    /// so exit_expression can identify them.
    pending_dollar_calls: HashSet<u32>,

    /// Active props destructuring info for the current component$ call.
    /// Set in enter_call_expression, consumed in exit_expression.
    active_props_info: Option<PropsDestructuringInfo>,

    /// Stack of capture tracking state for nested $()-bodies.
    /// Each entry is (body_ident_refs, body_local_decls) for one $()-body.
    /// Pushed on entering a $()-call, popped on exiting.
    capture_stack: Vec<(Vec<String>, HashSet<String>)>,

    /// Stack of "invalid declaration" names (function/class declarations) per $()-body.
    /// In SWC, these go to `invalid_decl` and are NOT captured -- instead C02 diagnostics
    /// are emitted. Matches SWC's partition of decl_stack into (decl_collect, invalid_decl).
    invalid_decl_stack: Vec<HashSet<String>>,

    /// Const bindings with simple literal initializers (number, string, boolean).
    /// Maps binding name to its literal string representation.
    /// SWC inlines these in segment bodies and doesn't capture them.
    const_literal_bindings: HashMap<String, String>,

    /// Serialized body code for each segment (segment strategy only).
    /// Keyed by call span.start for matching to SegmentData.
    segment_body_codes: Vec<(u32, String)>,

    /// Hoisted function declarations to insert at module top level.
    /// Each entry is (fn_declaration_code, str_declaration_code).
    /// e.g., ("const _hf0 = (p0)=>p0.value;", "const _hf0_str = \"p0.value\";")
    hoisted_function_stmts: Vec<(String, String)>,

    /// Set of span starts for segments that are stripped (matching strip_ctx_name).
    stripped_segments: HashSet<u32>,

    /// Set of span starts for sync$() calls.
    pending_sync_calls: HashSet<u32>,

    /// Pending Qrl-suffixed imports for parent segments (nested $-calls).
    /// Each entry: (parent_display_name, qrl_name).
    pending_segment_qrl_imports: Vec<(String, String)>,

    /// Custom JSX import source module path (e.g., "react" from `@jsxImportSource react`).
    /// When Some, JSX event handler `$`-attributes are NOT extracted as segments
    /// because the JSX is not Qwik JSX.
    custom_jsx_import_source: Option<String>,

    /// Set of module-level declaration names that need `_auto_` prefix when
    /// re-exported for segment self-imports. Populated during reclassify and
    /// finalize_segments for names that are in module_level_decls but NOT
    /// in exported_local_names.
    auto_exports: HashSet<String>,

    /// Number of synthetic framework imports in the entry module (set in exit_program).
    /// Used by lib.rs to inject hoisted _hf* stmts after the correct number of imports
    /// (after synthetic imports, before _Fragment/non-dollar/user imports).
    synthetic_import_count: usize,

    /// Original source code, used for extracting JSX lambda body code by span.
    source_code: String,

    /// Source comments from the original parse, used for comment-preserving codegen.
    /// Stored as standard Vec (Comment is Copy) since arena Vec can't outlive the allocator.
    source_comments: Vec<Comment>,

    /// JSX event handler replacement info, keyed by lambda expression span start.
    /// When the JSX transform encounters an event handler attribute whose value
    /// expression has a span start in this map, it replaces the value with
    /// a `qrl()` or `inlinedQrl()` call instead of keeping the raw lambda.
    jsx_event_replacements: HashMap<u32, JsxEventReplacement>,

    /// Scope context stack (mirrors SWC's stack_ctxt).
    /// Accumulates ALL scope names (variable names, function names, JSX element tags,
    /// attribute names, callee names) as the AST is traversed. Display names are
    /// built by joining this stack with "_" and running through `escape_sym()`.
    stack_ctxt: Vec<String>,

    /// Segment name stack for parent field (mirrors SWC's segment_stack).
    /// Stores the segment_name (display_name + hash, e.g., "renderHeader_XXXXXXXXXXXX")
    /// of each enclosing segment. Used to set the `parent` field on child segments.
    segment_stack: Vec<String>,

    /// Deduplication counter for display names (mirrors SWC's segment_names).
    /// When the same display name occurs multiple times, appends "_1", "_2", etc.
    segment_names: HashMap<String, u32>,

    /// Depth markers for variable declarator stack_ctxt push/pop.
    /// Each entry records the stack_ctxt length before the declarator was entered.
    var_decl_ctxt_depths: Vec<usize>,

    /// Depth markers for function declaration stack_ctxt push/pop.
    fn_decl_ctxt_depths: Vec<usize>,

    /// Depth markers for call expression stack_ctxt push/pop.
    call_expr_ctxt_depths: Vec<usize>,

    /// Depth markers for export default declaration stack_ctxt push/pop.
    export_default_ctxt_depths: Vec<usize>,

    /// Stack tracking whether current JSX element is a native HTML element.
    /// Mirrors SWC's `jsx_element_is_native` stack.
    jsx_element_is_native: Vec<bool>,

    /// Depth counter for nested loops. When > 0, QRL calls are inside a loop context.
    loop_depth: u32,

    /// Stack of iteration variables for each loop scope.
    /// Each entry contains the iteration variable names for that loop level.
    iteration_var_stack: Vec<Vec<String>>,

    /// Depth counter for iteration method callbacks (.map/.filter/etc).
    /// Increment on entering an iteration method call, decrement on exit.
    /// When > 0, the current scope is inside an iteration method callback.
    /// Uses a depth counter (not bool) to handle nested iteration methods
    /// correctly: e.g. arr.map(x => x.items.filter(y => ...)) — exiting
    /// the inner .filter() decrements to 1 (still inside .map()), not 0.
    in_callback_depth: u32,

    /// Whether the next JSX element processed is the "root" element in its scope.
    /// When true, the element gets a generated key; when false, native elements get null.
    /// Set to true on entering function/arrow bodies and statement-level scopes
    /// (for/while/if/block/return), set to false after first JSX element processes.
    /// Mirrors SWC's root_jsx_mode.
    root_jsx_mode: bool,

    /// Stack for saving/restoring root_jsx_mode across nested scopes.
    root_jsx_mode_stack: Vec<bool>,

    /// Precomputed JSX key prefix string (e.g., "u6" for test.tsx).
    /// Computed from base64url(DefaultHasher(scope?, rel_path).to_le_bytes())[0..2]
    /// with '-' and '_' chars replaced by '0'.
    jsx_key_prefix: String,

    /// QRL declarations to hoist to the top of the enclosing function body.
    /// Each entry: (lazy_import_ident_name, segment_export_name, capture_names).
    /// Populated when replace_jsx_element_handlers processes a segment-strategy
    /// QRL replacement while loop_depth > 0.
    /// Flushed in exit_function / exit_arrow_function_expression when loop_depth == 0.
    pending_loop_qrl_hoists: Vec<(String, String, Vec<String>)>,

    /// Iteration variables used by each event handler, keyed by lambda span start.
    /// Populated during enter_call_expression when processing JSX event handler
    /// lambdas inside loops. Consumed during replace_jsx_element_handlers for
    /// q:p/q:ps attribute injection at the correct element level.
    iter_var_usage_by_handler: HashMap<u32, Vec<String>>,

    /// Map from component$ call span.start to the set of function/class declaration
    /// names that should be forcibly removed from the component's segment body code.
    /// These are the names from the component's `invalid_decl_stack` frame (C02 names).
    /// SWC removes these declarations from the component body during its transform.
    component_invalid_decls: HashMap<u32, HashSet<String>>,
}

/// Info needed to build a qrl()/inlinedQrl() call for a JSX event handler.
#[derive(Debug, Clone)]
pub(crate) struct JsxEventReplacement {
    /// The segment name (e.g., "test_div_q_e_click_abc123")
    pub segment_name: String,
    /// The segment hash (e.g., "abc123")
    pub hash: String,
    /// Captured variable names
    pub capture_names: Vec<String>,
    /// Whether to use inlinedQrl (true) or qrl (false)
    pub is_inline: bool,
    /// The full display name for dev mode metadata (e.g., "test.tsx_App_component_Cmp_p_q_e_click")
    pub display_name: String,
    /// The body span (lo, hi) for dev mode metadata
    pub body_span: (u32, u32),
}

impl QwikTransform {
    /// Create a new QwikTransform instance.
    pub fn new(
        options: &TransformOptions,
        collected: CollectResult,
        filename: &str,
        source_code: &str,
    ) -> Self {
        // Compute JSX key prefix from file hash (matches SWC transform.ts lines 163-183)
        let jsx_key_prefix = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::Hasher;

            // Normalize Windows backslashes before hashing to match SWC
            let normalized_filename = filename.replace('\\', "/");
            let mut hasher = DefaultHasher::new();
            if let Some(ref scope) = options.scope {
                hasher.write(scope.as_bytes());
            }
            hasher.write(normalized_filename.as_bytes());
            let file_hash = hasher.finish();

            // Base64url encode first 2 chars of LE bytes
            // MUST use URL_SAFE alphabet (matches SWC), NOT standard base64 (+/)
            const CHARS: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let bytes = file_hash.to_le_bytes();
            let b0 = bytes[0] as usize;
            let b1 = bytes[1] as usize;
            let c0 = CHARS[b0 >> 2] as char;
            let c1 = CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char;
            // Replace - and _ with 0 (matching SWC)
            let c0 = if c0 == '-' || c0 == '_' { '0' } else { c0 };
            let c1 = if c1 == '-' || c1 == '_' { '0' } else { c1 };
            format!("{}{}", c0, c1)
        };

        // Build immutable_function_cmp set from imports (mirrors SWC lines 205-232).
        // These are component tags that don't set jsx_mutable = true.
        let mut immutable_function_cmp = HashSet::new();
        // Always include _Fragment (the transform-generated import name)
        immutable_function_cmp.insert("_Fragment".to_string());
        for import in &collected.module_imports {
            let source = &import.source;

            // Fragment from jsx-runtime or jsx-dev-runtime
            if source.contains("jsx-runtime") || source.contains("jsx-dev-runtime") {
                for local_name in &import.specifiers {
                    let imported_name = import
                        .specifier_aliases
                        .get(local_name)
                        .map(|s| s.as_str())
                        .unwrap_or(local_name.as_str());
                    if imported_name == "Fragment" {
                        immutable_function_cmp.insert(local_name.clone());
                    }
                }
            }

            // Fragment, RenderOnce from @qwik.dev/core or @builder.io/qwik
            if source == "@qwik.dev/core" || source == "@builder.io/qwik" {
                for local_name in &import.specifiers {
                    let imported_name = import
                        .specifier_aliases
                        .get(local_name)
                        .map(|s| s.as_str())
                        .unwrap_or(local_name.as_str());
                    if imported_name == "Fragment" || imported_name == "RenderOnce" {
                        immutable_function_cmp.insert(local_name.clone());
                    }
                }
            }

            // Link from @qwik.dev/router or @builder.io/qwik-city
            if source == "@qwik.dev/router" || source == "@builder.io/qwik-city" {
                for local_name in &import.specifiers {
                    let imported_name = import
                        .specifier_aliases
                        .get(local_name)
                        .map(|s| s.as_str())
                        .unwrap_or(local_name.as_str());
                    if imported_name == "Link" {
                        immutable_function_cmp.insert(local_name.clone());
                    }
                }
            }

            // ALL names from ?jsx or .md sources
            if source.ends_with("?jsx") || source.ends_with(".md") {
                for local_name in &import.specifiers {
                    immutable_function_cmp.insert(local_name.clone());
                }
            }
        }

        // Build const_bindings from all import specifier names.
        // Imports are always const in JavaScript -- they cannot be reassigned.
        // This mirrors SWC's ConstCollector which includes all imports.
        let mut const_bindings = HashSet::new();
        for imp in &collected.module_imports {
            for spec in &imp.specifiers {
                const_bindings.insert(spec.clone());
            }
        }

        Self {
            options: options.clone(),
            collected,
            filename: filename.to_string(),
            segments: Vec::new(),
            diagnostics: Vec::new(),
            import_tracker: ImportTracker {
                immutable_function_cmp,
                const_bindings,
                jsx_dev_file_name: if matches!(options.mode, crate::types::EmitMode::Dev) {
                    Some(filename.to_string())
                } else {
                    None
                },
                jsx_dev_source_code: if matches!(options.mode, crate::types::EmitMode::Dev) {
                    Some(source_code.to_string())
                } else {
                    None
                },
                ..ImportTracker::default()
            },
            segment_counter: 0,
            dollar_call_stack: Vec::new(),
            pending_dollar_calls: HashSet::new(),
            active_props_info: None,
            capture_stack: Vec::new(),
            invalid_decl_stack: Vec::new(),
            const_literal_bindings: HashMap::new(),
            segment_body_codes: Vec::new(),
            hoisted_function_stmts: Vec::new(),
            stripped_segments: HashSet::new(),
            pending_sync_calls: HashSet::new(),
            pending_segment_qrl_imports: Vec::new(),
            custom_jsx_import_source: None,
            auto_exports: HashSet::new(),
            synthetic_import_count: 0,
            source_code: source_code.to_string(),
            source_comments: Vec::new(),
            jsx_event_replacements: HashMap::new(),
            stack_ctxt: Vec::new(),
            segment_stack: Vec::new(),
            segment_names: HashMap::new(),
            var_decl_ctxt_depths: Vec::new(),
            fn_decl_ctxt_depths: Vec::new(),
            call_expr_ctxt_depths: Vec::new(),
            export_default_ctxt_depths: Vec::new(),
            jsx_element_is_native: Vec::new(),
            loop_depth: 0,
            iteration_var_stack: Vec::new(),
            in_callback_depth: 0,
            root_jsx_mode: true,
            root_jsx_mode_stack: Vec::new(),
            jsx_key_prefix,
            pending_loop_qrl_hoists: Vec::new(),
            iter_var_usage_by_handler: HashMap::new(),
            component_invalid_decls: HashMap::new(),
        }
    }

    /// Get the segments extracted during traversal.
    pub fn extracted_segments(&self) -> &[SegmentData] {
        &self.segments
    }

    /// Get any diagnostics generated during traversal.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Take the serialized body codes for segment strategy.
    /// Each entry is (span_start, body_code_string).
    pub fn take_segment_body_codes(&mut self) -> Vec<(u32, String)> {
        std::mem::take(&mut self.segment_body_codes)
    }

    /// Get the map of component span_start -> invalid decl names to force-remove.
    pub fn component_invalid_decls(&self) -> &HashMap<u32, HashSet<String>> {
        &self.component_invalid_decls
    }

    /// Get the hoisted function declarations for _fnSignal.
    /// Each entry is (fn_declaration_code, str_declaration_code).
    /// E.g., ("const _hf0 = (p0)=>p0.value;", "const _hf0_str = \"p0.value\";")
    pub fn hoisted_function_stmts(&self) -> &[(String, String)] {
        &self.hoisted_function_stmts
    }

    /// Get the number of synthetic framework imports emitted in exit_program.
    /// Used by lib.rs to inject hoisted stmts after synthetic imports but
    /// before _Fragment/non-dollar/user imports.
    pub fn synthetic_import_count(&self) -> usize {
        self.synthetic_import_count
    }

    /// Get the set of span starts for stripped segments.
    pub fn stripped_segments(&self) -> &HashSet<u32> {
        &self.stripped_segments
    }

    /// Set the custom JSX import source module path (e.g., "react" from `@jsxImportSource react`).
    /// When Some, JSX event handler `$`-attributes are NOT extracted as segments,
    /// and JSX is transformed to `_jsx()` from `{source}/jsx-runtime` instead of `_jsxSorted`.
    pub fn set_custom_jsx_import_source(&mut self, source: Option<String>) {
        self.import_tracker.custom_jsx_source = source.clone();
        self.custom_jsx_import_source = source;
    }

    /// Get the custom JSX import source module path, if any.
    pub fn custom_jsx_import_source(&self) -> Option<&str> {
        self.custom_jsx_import_source.as_deref()
    }

    /// Get the set of auto-exported names (module-level decls that need `_auto_` prefix).
    pub fn auto_exports(&self) -> &HashSet<String> {
        &self.auto_exports
    }

    /// Get current iteration variables from the innermost loop scope only.
    /// SWC uses `self.iteration_var_stack.last()` -- only the innermost loop's
    /// variables are considered for param injection and q:p attributes. Variables
    /// from outer loops that are used in event handlers become captures instead.
    pub(crate) fn current_iteration_vars(&self) -> Vec<String> {
        self.iteration_var_stack
            .last()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Check if a ctx name should be stripped based on strip_ctx_name config.
    fn should_strip_ctx_name(&self, ctx_name: &str) -> bool {
        self.options.strip_ctx_name.iter().any(|stripped| {
            let name_without_dollar = ctx_name.trim_end_matches('$');
            name_without_dollar
                .to_lowercase()
                .contains(&stripped.to_lowercase())
        })
    }

    /// Post-transform processing: populate child segment metadata.
    /// Must be called after traverse_mut completes.
    pub fn finalize_segments(&mut self) {
        let child_info: Vec<(String, String, String)> = self
            .segments
            .iter()
            .filter_map(|seg| {
                if self.stripped_segments.contains(&seg.span.0) {
                    return None;
                }
                seg.parent.as_ref().map(|parent_name| {
                    let canonical = format!("{}_{}", seg.display_name, seg.hash);
                    let import_path = if self.options.explicit_extensions {
                        let ext = self.compute_output_extension();
                        format!("./{}.{}", canonical, ext)
                    } else {
                        format!("./{}", canonical)
                    };
                    (parent_name.clone(), seg.hash.clone(), import_path)
                })
            })
            .collect();

        // Transfer pending Qrl-suffixed imports to parent segments
        let pending_qrl_imports = std::mem::take(&mut self.pending_segment_qrl_imports);
        let self_import_source = self.self_import_source();

        for seg in self.segments.iter_mut() {
            // Parent field now stores segment name with hash (e.g., "renderHeader_XXXXXXXXXXXX"),
            // so match against seg.name (which is also segment name with hash).
            let children: Vec<_> = child_info
                .iter()
                .filter(|(parent, _, _)| parent == &seg.name)
                .collect();

            if !children.is_empty() {
                seg.needs_qrl_import = true;
                for (_, hash, path) in children {
                    seg.child_lazy_imports.push((hash.clone(), path.clone()));
                }
                // Sort by hash to match SWC's BTreeMap<Id> ordering (alphabetical by key).
                seg.child_lazy_imports.sort_by(|a, b| a.0.cmp(&b.0));
            }

            // Assign Qrl-suffixed imports from nested $-calls to their parent segment.
            // If the Qrl name is a locally-defined export (not a framework import),
            // add it as a self-import in needed_imports instead of segment_qrl_names.
            // This matches SWC's behavior where local_idents found in global.exports
            // become self-imports (import from "./filename") rather than core imports.
            for (parent_name, qrl_name) in &pending_qrl_imports {
                if parent_name == &seg.display_name && !seg.segment_qrl_names.contains(qrl_name) {
                    if self.collected.module_level_decls.contains(qrl_name.as_str()) {
                        // Locally-defined Qrl function: import from self module
                        // Track non-user-exported names for _auto_ prefix
                        if !self.collected.exported_local_names.contains(qrl_name) {
                            self.auto_exports.insert(qrl_name.clone());
                        }
                        let already_imported = seg.needed_imports.iter().any(|imp| {
                            imp.specifiers.contains(qrl_name)
                        });
                        if !already_imported {
                            seg.needed_imports.push(crate::types::ImportInfo {
                                source: self_import_source.clone(),
                                specifiers: vec![qrl_name.clone()],
                                specifier_kinds: vec![crate::types::ImportKind::Named],
                                specifier_aliases: std::collections::HashMap::new(),
                                is_qwik_core: false,
                                span: (0, 0),
                                assertion: Vec::new(),
                            });
                        }
                    } else {
                        seg.segment_qrl_names.push(qrl_name.clone());
                    }
                }
            }
        }
    }

    /// Check if a CallExpression is a Qwik $-call.
    ///
    /// Resolves aliases: if the callee is an aliased import (e.g., `Component`
    /// for `component$`), returns the DollarCallKind using the ORIGINAL
    /// imported name, not the alias.
    fn is_dollar_call(&self, call: &CallExpression<'_>) -> Option<DollarCallKind> {
        match &call.callee {
            Expression::Identifier(ident) => {
                let name = ident.name.as_str();
                if self.collected.dollar_imports.contains(name) {
                    let original_name = self
                        .collected
                        .alias_map
                        .get(name)
                        .map(|s| s.as_str())
                        .unwrap_or(name);

                    if original_name == "$" {
                        Some(DollarCallKind::RawDollar)
                    } else {
                        Some(DollarCallKind::Named(original_name.to_string()))
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract the basename with extension (file_name) from the filename path.
    ///
    /// E.g., "src/routes/_repl/[id]/[[...slug]].tsx" -> "[[...slug]].tsx"
    /// E.g., "test.tsx" -> "test.tsx"
    /// E.g., "components\\\\apps\\\\apps.tsx" -> "apps.tsx"
    ///
    /// Handles both Unix `/` and Windows `\\` path separators.
    /// Matches SWC's `path_data.file_name`.
    fn file_name(&self) -> &str {
        // Find the last path separator (either / or \)
        let last_sep = self
            .filename
            .rfind(|c: char| c == '/' || c == '\\')
            .map(|pos| pos + 1)
            .unwrap_or(0);
        &self.filename[last_sep..]
    }

    /// Extract the file stem (basename without extension) from the filename path.
    ///
    /// Uses Rust's Path::file_stem() logic: strips only the LAST extension.
    /// E.g., "[[...slug]].tsx" -> "[[...slug]]"
    /// E.g., "test.tsx" -> "test"
    /// E.g., "404.tsx" -> "404"
    ///
    /// Matches SWC's `path_data.file_stem`.
    fn file_stem(&self) -> String {
        let basename = self.file_name();
        // Strip only the last extension (after the last dot, if the dot is not at position 0)
        if let Some(dot_pos) = basename.rfind('.') {
            if dot_pos > 0 {
                return basename[..dot_pos].to_string();
            }
        }
        basename.to_string()
    }

    /// Build the canonical filename for a segment.
    ///
    /// Uses file_name (basename with extension), not the full path.
    /// Matches SWC's `get_canonical_filename(display_name, symbol_name)`.
    fn build_canonical_filename(&self, display_name: &str, hash: &str) -> String {
        format!("{}_{display_name}_{hash}", self.file_name())
    }

    /// Compute the output file extension based on transpile options.
    ///
    /// Logic: strip what you transpile.
    /// - transpile_ts removes TypeScript: tsx->jsx, ts->js
    /// - transpile_jsx removes JSX: tsx->ts, jsx->js
    /// - both: tsx->js, ts->js
    fn compute_output_extension(&self) -> &str {
        let ext = self.filename.rsplit('.').next().unwrap_or("js");
        match (self.options.transpile_ts, self.options.transpile_jsx, ext) {
            (true, true, "tsx") => "js",
            (true, true, "ts") => "js",
            (true, false, "tsx") => "jsx",
            (true, false, "ts") => "js",
            (false, true, "tsx") => "ts",
            (false, true, "jsx") => "js",
            _ => ext,
        }
    }

    /// Build the segment import path for segment strategy.
    fn build_segment_import_path(&self, canonical_filename: &str) -> String {
        if self.options.explicit_extensions {
            let ext = self.compute_output_extension();
            format!("./{canonical_filename}.{ext}")
        } else {
            format!("./{canonical_filename}")
        }
    }

    /// Detect file extension from filename.
    fn file_extension(&self) -> String {
        self.filename.rsplit('.').next().unwrap_or("js").to_string()
    }

    /// Whether the current emit mode is Dev.
    fn is_dev_mode(&self) -> bool {
        matches!(self.options.mode, crate::types::EmitMode::Dev)
    }

    /// Compute the absolute file path for dev mode metadata.
    /// Matches SWC's `path_data.abs_path.to_slash_lossy()`: `normalize_path(src_dir.join(filename))`
    fn dev_abs_path(&self) -> String {
        let src_dir = &self.options.src_dir;
        let filename = &self.filename;
        if src_dir == "." || src_dir.is_empty() {
            format!("./{}", filename)
        } else {
            let src = src_dir.trim_end_matches('/');
            format!("{}/{}", src, filename)
        }
    }

    /// Build dev metadata for a QRL call, or None if not in dev mode.
    /// - `lo`/`hi`: byte offsets of the $()-call body (0-based, from OXC spans)
    /// - `display_name`: the full display name of the segment
    ///
    /// SWC uses 1-based byte positions (BytePos), so we add 1 to match golden output.
    fn make_qrl_dev_meta(
        &self,
        lo: u32,
        hi: u32,
        display_name: &str,
    ) -> Option<import_rewrite::QrlDevMetadata> {
        if !self.is_dev_mode() {
            return None;
        }
        Some(import_rewrite::QrlDevMetadata {
            file: self.dev_abs_path(),
            lo: lo + 1,
            hi: hi + 1,
            display_name: display_name.to_string(),
        })
    }

    /// Build dev metadata for a _noopQrl call (lo and hi are always 0).
    fn make_noop_dev_meta(
        &self,
        display_name: &str,
    ) -> Option<import_rewrite::QrlDevMetadata> {
        if !self.is_dev_mode() {
            return None;
        }
        Some(import_rewrite::QrlDevMetadata {
            file: self.dev_abs_path(),
            lo: 0,
            hi: 0,
            display_name: display_name.to_string(),
        })
    }

    /// Compute the self-import source path for module-level declaration re-imports.
    ///
    /// When a nested segment references a module-level declaration (const, function,
    /// class), the SWC optimizer re-imports it from the parent module rather than
    /// capturing it. This method returns the import source path (e.g., "./test"
    /// for a file named "test.tsx").
    fn self_import_source(&self) -> String {
        let basename = self.filename.rsplit('/').next().unwrap_or(&self.filename);
        if self.options.explicit_extensions {
            format!("./{}", basename)
        } else {
            let stem = basename.rsplit('.').last().unwrap_or(basename);
            format!("./{}", stem)
        }
    }

    /// Post-process a capture analysis result to convert module-level declarations
    /// from captures into needed_imports (self-imports from the parent module).
    ///
    /// The SWC optimizer handles module-level declarations (const, function, class)
    /// by re-importing them in the segment module rather than serializing/restoring
    /// them via `_captures[]`. This post-processing step implements that behavior.
    fn reclassify_module_level_decl_captures(
        &mut self,
        mut capture_result: collector::CaptureAnalysisResult,
        is_stripped: bool,
    ) -> (collector::CaptureAnalysisResult, Vec<crate::types::ImportInfo>) {
        let self_import_source = self.self_import_source();
        let mut extra_imports: Vec<crate::types::ImportInfo> = Vec::new();

        // Partition capture_names: keep non-module-level-decl names as true captures,
        // convert module-level-decl names to needed_imports (self-imports).
        let mut true_captures = Vec::new();
        for name in capture_result.capture_names.drain(..) {
            if self.collected.module_level_decls.contains(&name) {
                // Track non-user-exported names for _auto_ prefix.
                // Skip for stripped segments -- they don't produce segment files,
                // so their self-imports don't need _auto_ re-exports.
                if !is_stripped && !self.collected.exported_local_names.contains(&name) {
                    self.auto_exports.insert(name.clone());
                }
                extra_imports.push(crate::types::ImportInfo {
                    source: self_import_source.clone(),
                    specifiers: vec![name],
                    specifier_kinds: vec![crate::types::ImportKind::Named],
                    specifier_aliases: std::collections::HashMap::new(),
                    is_qwik_core: false,
                    span: (0, 0),
                    assertion: Vec::new(),
                });
            } else {
                true_captures.push(name);
            }
        }

        capture_result.capture_names = true_captures;
        (capture_result, extra_imports)
    }

    /// Collect all binding names from a BindingPattern into the current
    /// capture stack frame's body_local_decls.
    /// Handles BindingIdentifier, ObjectPattern, and ArrayPattern recursively.
    fn collect_binding_pattern_names(&mut self, pattern: &BindingPattern<'_>) {
        match pattern {
            BindingPattern::BindingIdentifier(ident) => {
                if let Some(frame) = self.capture_stack.last_mut() {
                    frame.1.insert(ident.name.as_str().to_string());
                }
            }
            BindingPattern::ObjectPattern(obj) => {
                for prop in &obj.properties {
                    self.collect_binding_pattern_names(&prop.value);
                }
                if let Some(rest) = &obj.rest {
                    self.collect_binding_pattern_names(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(arr) => {
                for elem in arr.elements.iter().flatten() {
                    self.collect_binding_pattern_names(elem);
                }
                if let Some(rest) = &arr.rest {
                    self.collect_binding_pattern_names(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assign) => {
                self.collect_binding_pattern_names(&assign.left);
            }
        }
    }

    /// Build display name from the stack_ctxt, applying escape_sym, digit prefix,
    /// and deduplication -- mirrors SWC's `register_context_name`.
    ///
    /// Returns `(display_name, full_display_name, segment_hash, segment_name)`.
    fn register_context_name(&mut self) -> (String, String, String, String) {
        // 1. Join stack context
        let mut display_name = if self.stack_ctxt.is_empty() {
            "s_".to_string()
        } else {
            self.stack_ctxt.join("_")
        };

        // 2. Escape non-alphanumeric chars
        display_name = escape_sym(&display_name);

        // 3. Ensure valid identifier start
        if display_name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            display_name = format!("_{}", display_name);
        }

        // 4. Deduplicate (mirrors SWC's segment_names logic)
        let index = match self.segment_names.get_mut(&display_name) {
            Some(count) => {
                *count += 1;
                *count
            }
            None => 0,
        };
        if index == 0 {
            self.segment_names.insert(display_name.clone(), 0);
        } else {
            display_name = format!("{}_{}", display_name, index);
        }

        // 5. Hash -- computed on display_name (WITHOUT filename prefix), matching SWC.
        // SWC hashes: scope? + rel_path + display_name (no file_name prefix).
        // Then AFTER hashing, prepends file_name to display_name for the full display name.
        let segment_hash = hash::compute_segment_hash(
            self.options.scope.as_deref(),
            &self.filename,
            &display_name,
        );

        // 6. Prepend file_name (basename with extension) to display_name
        let full_display_name = format!("{}_{}", self.file_name(), display_name);

        // 7. Build segment_name -- in Dev/Lib modes use "display_name_hash",
        // in Prod mode use "s_hash" (mirrors SWC's register_context_name).
        // SWC uses Dev|Test for full names and Lib|Prod for s_HASH, but since
        // OXC tests default to Lib mode (matching SWC's Test mode behavior),
        // we only use s_HASH for Prod mode.
        let segment_name = if matches!(self.options.mode, crate::types::EmitMode::Prod) {
            format!("s_{}", segment_hash)
        } else {
            hash::format_segment_name(&display_name, &segment_hash)
        };

        (display_name, full_display_name, segment_hash, segment_name)
    }

    /// Record a segment and track imports. Returns the segment data.
    fn record_segment(&mut self, call: &CallExpression<'_>, kind: &DollarCallKind) -> SegmentData {
        let (display_name, full_display_name, segment_hash, segment_name) =
            self.register_context_name();

        let canonical_filename = self.build_canonical_filename(&display_name, &segment_hash);
        let import_path = self.build_segment_import_path(&canonical_filename);

        let ctx_name = match kind {
            DollarCallKind::RawDollar => "$".to_string(),
            DollarCallKind::Named(name) => name.clone(),
        };
        let ctx_kind = words::classify_ctx_kind(&ctx_name);

        // Parent uses segment_stack (segment name WITH hash), matching SWC
        let parent = self.segment_stack.last().cloned();

        let segment = SegmentData {
            display_name: full_display_name.clone(),
            hash: segment_hash.clone(),
            name: segment_name.clone(),
            ctx_name: ctx_name.clone(),
            ctx_kind,
            origin: self.filename.clone(),
            extension: self.file_extension(),
            span: (call.span.start, call.span.end),
            parent,
            captures: false,        // Computed in exit_expression
            capture_names: vec![],  // Computed in exit_expression
            needed_imports: vec![], // Populated by finalize_segments
            segment_qrl_names: vec![],
            body_span: if let Some(first_arg) = call.arguments.first() {
                use oxc::span::GetSpan;
                let s = first_arg.span();
                (s.start, s.end)
            } else {
                (call.span.start, call.span.end)
            },
            param_names: if let Some(first_arg) = call.arguments.first() {
                extract_param_names_from_argument(first_arg)
            } else {
                vec![]
            },
            body_code: String::new(),   // Populated in exit_expression for segment strategy
            child_lazy_imports: vec![], // Populated by finalize_segments
            needs_qrl_import: false,    // Populated by finalize_segments
            stack_ctxt: self.stack_ctxt.clone(),
        };

        let will_be_stripped = match kind {
            DollarCallKind::Named(name) => self.should_strip_ctx_name(name),
            _ => false,
        };

        // Record Qrl-suffixed import name (componentQrl, etc.) during enter.
        // SWC encounters the dollar call name FIRST (fold enters call expression),
        // then folds the body (recording JSX imports), then wraps with qrl/inlinedQrl.
        // So: componentQrl is recorded in ENTER, inlinedQrl/qrl in EXIT.
        if let DollarCallKind::Named(name) = &kind {
            let qrl_name = words::dollar_to_qrl_name(name);
            if self.dollar_call_stack.is_empty() {
                // Top-level $-call: Qrl import goes to main module
                if !self.import_tracker.qrl_imports.contains(&qrl_name) {
                    self.import_tracker.record_synthetic_import(&qrl_name);
                    self.import_tracker.qrl_imports.push(qrl_name);
                }
            } else {
                // Nested $-call: Qrl import goes to parent segment, not main module
                let parent_display_name = self.dollar_call_stack.last().unwrap().clone();
                self.import_tracker.record_synthetic_import(&qrl_name);
                self.pending_segment_qrl_imports
                    .push((parent_display_name, qrl_name));
            }
        }

        if !will_be_stripped {
            let is_inline = entry_strategy::should_inline(&self.options.entry_strategy)
                || matches!(
                    self.options.entry_strategy,
                    crate::types::EntryStrategy::Hoist
                );

            // Set the flags (needed for downstream logic), but defer
            // record_synthetic_import to exit_expression for correct encounter order.
            if is_inline {
                self.import_tracker.needs_inlined_qrl = true;
            } else {
                self.import_tracker.needs_qrl = true;
                self.import_tracker
                    .lazy_imports
                    .push((segment_hash.clone(), import_path));
            }
        }

        self.segment_counter += 1;

        // Push to segment_stack (for parent field of nested segments)
        self.segment_stack.push(segment_name);
        // Push to dollar_call_stack (for finalize_segments parent matching)
        self.dollar_call_stack.push(full_display_name);

        self.segments.push(segment.clone());
        segment
    }

    /// Record a segment for a JSX event handler attribute (e.g., onClick$).
    ///
    /// Unlike `record_segment`, this doesn't require a CallExpression -- it takes
    /// the span and ctx_name directly from JSX attribute info.
    /// The display name is derived from `stack_ctxt` (which should already have
    /// the JSX element name and attribute name pushed).
    pub(crate) fn record_jsx_event_segment(
        &mut self,
        span: (u32, u32),
        ctx_name: &str,
        param_names: Vec<String>,
        is_native_element: bool,
    ) -> SegmentData {
        let (display_name, full_display_name, segment_hash, segment_name) =
            self.register_context_name();

        let canonical_filename = self.build_canonical_filename(&display_name, &segment_hash);
        let import_path = self.build_segment_import_path(&canonical_filename);

        // On native HTML elements, $-suffixed attributes are event handlers.
        // On component elements, they are JSX prop functions (matching SWC's
        // handle_jsx_props_obj which uses is_fn=true → JSXProp for components).
        let ctx_kind = if is_native_element {
            crate::types::CtxKind::EventHandler
        } else {
            crate::types::CtxKind::JSXProp
        };
        // Parent uses segment_stack (segment name WITH hash), matching SWC
        let parent = self.segment_stack.last().cloned();

        let segment = SegmentData {
            display_name: full_display_name.clone(),
            hash: segment_hash.clone(),
            name: segment_name,
            ctx_name: ctx_name.to_string(),
            ctx_kind,
            origin: self.filename.clone(),
            extension: self.file_extension(),
            span,
            parent,
            captures: false,
            capture_names: vec![],
            needed_imports: vec![],
            segment_qrl_names: vec![],
            body_span: span,
            param_names,
            body_code: String::new(),
            child_lazy_imports: vec![],
            needs_qrl_import: false,
            stack_ctxt: self.stack_ctxt.clone(),
        };

        let will_be_stripped = self.should_strip_ctx_name(ctx_name);

        if !will_be_stripped {
            let is_inline = entry_strategy::should_inline(&self.options.entry_strategy)
                || matches!(
                    self.options.entry_strategy,
                    crate::types::EntryStrategy::Hoist
                );

            // Set flags but defer record_synthetic_import to exit_expression
            // for correct encounter order matching SWC.
            if is_inline {
                self.import_tracker.needs_inlined_qrl = true;
            } else {
                self.import_tracker.needs_qrl = true;
                self.import_tracker
                    .lazy_imports
                    .push((segment_hash.clone(), import_path));
            }
        }

        self.segment_counter += 1;
        self.segments.push(segment.clone());
        segment
    }

    /// Pre-scan a JSXElement (recursively) for $-suffixed attributes and create
    /// segments for each. This ensures segment modules are produced for JSX event
    /// handlers like onClick$, onInput$, render$, etc.
    ///
    /// When `strip_event_handlers` is true, no JSX event segments are created.
    /// When the module has a custom `@jsxImportSource`, JSX events are also skipped
    /// since they are not Qwik JSX attributes.
    fn create_jsx_event_segments_recursive(&mut self, element: &JSXElement<'_>) {
        // When strip_event_handlers is enabled, skip all JSX event handler extraction
        if self.options.strip_event_handlers {
            return;
        }
        // When a custom JSX import source is set (e.g., React), JSX $-attributes
        // are not Qwik event handlers -- do not extract them.
        if self.custom_jsx_import_source.is_some() {
            return;
        }

        // Push element name to stack_ctxt (mirrors SWC's fold_jsx_element)
        let (element_name, is_native_element) = match &element.opening_element.name {
            JSXElementName::Identifier(ident) => {
                let name = ident.name.as_str().to_string();
                let is_native = name.chars().next().is_some_and(|c| c.is_lowercase());
                (Some(name), is_native)
            }
            JSXElementName::IdentifierReference(ident) => {
                let name = ident.name.as_str().to_string();
                (Some(name), false) // Component elements are not native
            }
            JSXElementName::NamespacedName(ns) => {
                let name = format!("{}:{}", ns.namespace.name, ns.name.name);
                (Some(name), false)
            }
            _ => (None, false),
        };

        if let Some(ref name) = element_name {
            self.stack_ctxt.push(name.clone());
        }

        // Scan attributes for $-suffixed names (including namespaced like document:onFocus$)
        for attr_item in &element.opening_element.attributes {
            if let JSXAttributeItem::Attribute(attr) = attr_item {
                let (attr_name_str, namespace_prefix) = match &attr.name {
                    JSXAttributeName::Identifier(ident) => (ident.name.as_str().to_string(), None),
                    JSXAttributeName::NamespacedName(ns) => {
                        let name = ns.name.name.as_str();
                        let prefix = ns.namespace.name.as_str();
                        (name.to_string(), Some(prefix.to_string()))
                    }
                };

                if attr_name_str.ends_with('$') {
                    if let Some(value) = &attr.value {
                        if let JSXAttributeValue::ExpressionContainer(container) = value {
                            let expr_span = get_jsx_lambda_span(&container.expression);
                            if let Some(span) = expr_span {
                                // Push the attribute name to stack_ctxt (mirrors SWC's fold_jsx_attr)
                                let attr_ctx_name = if let Some(ref prefix) = namespace_prefix {
                                    // Namespaced: push "ns-name$" format
                                    let full_attr = format!("{}:{}", prefix, attr_name_str);
                                    if is_native_element {
                                        if let Some(html_attr) = jsx_event_to_html_attribute(&full_attr) {
                                            html_attr
                                        } else {
                                            format!("{}-{}", prefix, attr_name_str)
                                        }
                                    } else {
                                        format!("{}-{}", prefix, attr_name_str)
                                    }
                                } else if is_native_element {
                                    // Native element: transform event name
                                    if let Some(html_attr) = jsx_event_to_html_attribute(&attr_name_str) {
                                        html_attr
                                    } else {
                                        attr_name_str.clone()
                                    }
                                } else {
                                    // Component element: push original name
                                    attr_name_str.clone()
                                };
                                self.stack_ctxt.push(attr_ctx_name);

                                let ctx_name = if namespace_prefix.is_some() {
                                    format!(
                                        "{}:{}",
                                        namespace_prefix.as_ref().unwrap(),
                                        attr_name_str
                                    )
                                } else {
                                    attr_name_str.clone()
                                };
                                let mut param_names =
                                    extract_param_names_from_jsx_expr(&container.expression);

                                // Run capture analysis on the JSX event handler lambda.
                                // This determines which variables from the enclosing scope
                                // need to be serialized and restored in the segment module.
                                // We do this BEFORE segment recording so we can use the
                                // body_ident_refs to check iteration variable usage.
                                let (body_ident_refs, body_local_decls) =
                                    analyze_lambda_captures(&self.source_code, span);

                                // When inside a loop, check if the handler uses iteration
                                // variables and transform param_names accordingly
                                // (mirrors SWC's transform_event_handler_with_iter_var).
                                if self.loop_depth > 0 {
                                    let iter_vars = self.current_iteration_vars();
                                    // Use deep scan for iteration variable detection.
                                    // body_ident_refs doesn't descend into nested functions,
                                    // but iteration variables can be captured by nested closures.
                                    // SWC's body_contains_ident does a full deep scan.
                                    let deep_refs = analyze_lambda_deep_ident_refs(
                                        &self.source_code, span,
                                    );
                                    let used_iter_vars: Vec<String> = iter_vars
                                        .iter()
                                        .filter(|v| deep_refs.contains(*v))
                                        .cloned()
                                        .collect();
                                    if !used_iter_vars.is_empty() {
                                        // Ensure at least 2 params (event, element)
                                        // SWC uses "_" for both placeholders
                                        while param_names.len() < 2 {
                                            param_names.push("_".to_string());
                                        }
                                        // Append used iteration variables
                                        for var_name in &used_iter_vars {
                                            param_names.push(var_name.clone());
                                        }
                                        // Record used iteration variables keyed by lambda span
                                        // start for q:p injection at the correct element level
                                        // during replace_jsx_element_handlers.
                                        self.iter_var_usage_by_handler
                                            .insert(span.0, used_iter_vars.clone());
                                    }
                                }

                                // Save iteration variable param names for capture filtering.
                                // param_names is moved into record_jsx_event_segment, so
                                // extract the iter var portion (positions 2+) before the move.
                                let iter_var_param_names: Vec<String> = if param_names.len() > 2 {
                                    param_names[2..].to_vec()
                                } else {
                                    Vec::new()
                                };

                                let seg =
                                    self.record_jsx_event_segment(span, &ctx_name, param_names, is_native_element);
                                let seg_span_0 = seg.span.0;
                                let capture_result = collector::compute_captures(
                                    &body_ident_refs,
                                    &body_local_decls,
                                    &self.collected,
                                );

                                // Filter captures to only include variables that actually
                                // exist in an enclosing scope. When inside $()-bodies,
                                // filter against ALL capture_stack frames' local declarations
                                // (not just the innermost). For nested $() like:
                                //   component$(() => { const state = ...; return $(() => {
                                //     return <div onClick$={() => state.count++} />
                                //   }) })
                                // `state` is in the outer frame, not the inner one.
                                // When NOT inside any $()-body (bare function like
                                // `export default ({data}) => <div onClick$={...}/>`),
                                // keep all captures since compute_captures() already
                                // filtered against module-level declarations/imports.
                                // Collect parent scope's invalid (fn/class) declarations
                                let all_invalid_decls: HashSet<String> = self
                                    .invalid_decl_stack
                                    .iter()
                                    .flat_map(|s| s.iter().cloned())
                                    .collect();

                                let capture_result = if !self.capture_stack.is_empty() {
                                    // Merge all local declarations from all capture stack frames
                                    let all_parent_decls: HashSet<String> = self
                                        .capture_stack
                                        .iter()
                                        .flat_map(|(_, decls)| decls.iter().cloned())
                                        .collect();
                                    let filtered_names: Vec<String> = capture_result
                                        .capture_names
                                        .into_iter()
                                        .filter(|name| {
                                            !all_invalid_decls.contains(name)
                                                && (all_parent_decls.contains(name)
                                                    || self.collected.module_level_decls.contains(name))
                                        })
                                        .collect();
                                    collector::CaptureAnalysisResult {
                                        capture_names: filtered_names,
                                        reemitted_imports: capture_result.reemitted_imports,
                                        diagnostics: capture_result.diagnostics,
                                    }
                                } else {
                                    // No parent $()-body scope -- keep all captures
                                    capture_result
                                };

                                // Reclassify module-level declarations from captures
                                // to needed_imports (self-imports from the parent module).
                                // JSX event handlers are never stripped, so is_stripped=false.
                                let (capture_result, module_decl_imports) =
                                    self.reclassify_module_level_decl_captures(capture_result, false);

                                // Filter iteration variable params from captures.
                                // SWC: scoped_idents.retain(|id| !param_idents.contains(id))
                                // Iteration variables are passed as function params (via q:p),
                                // not captured via _captures. Only applies to JSX event handlers --
                                // for $() calls, analyze_lambda_captures already adds arrow params
                                // to body_local_decls so compute_captures filters them naturally.
                                let capture_result = if !iter_var_param_names.is_empty() {
                                    let iter_var_set: HashSet<&str> =
                                        iter_var_param_names.iter().map(|s| s.as_str()).collect();
                                    collector::CaptureAnalysisResult {
                                        capture_names: capture_result
                                            .capture_names
                                            .into_iter()
                                            .filter(|name| !iter_var_set.contains(name.as_str()))
                                            .collect(),
                                        reemitted_imports: capture_result.reemitted_imports,
                                        diagnostics: capture_result.diagnostics,
                                    }
                                } else {
                                    capture_result
                                };

                                // Filter out const-literal captures and their re-imports.
                                // SWC inlines const literals (e.g., `const STEP_2 = 2`) into
                                // segment bodies instead of capturing them. Also filter
                                // reemitted_imports when a local const literal shadows an import.
                                let capture_result = if !self.const_literal_bindings.is_empty() {
                                    let filtered: Vec<String> = capture_result
                                        .capture_names
                                        .into_iter()
                                        .filter(|name| {
                                            !self.const_literal_bindings.contains_key(name)
                                                || self.collected.module_level_decls.contains(name)
                                        })
                                        .collect();
                                    let filtered_imports: Vec<_> = capture_result
                                        .reemitted_imports
                                        .into_iter()
                                        .filter(|ri| {
                                            !self.const_literal_bindings.contains_key(&ri.local_name)
                                        })
                                        .collect();
                                    collector::CaptureAnalysisResult {
                                        capture_names: filtered,
                                        reemitted_imports: filtered_imports,
                                        diagnostics: capture_result.diagnostics,
                                    }
                                } else {
                                    capture_result
                                };

                                // Convert reemitted imports to ImportInfo for needed_imports
                                let mut needed_imports: Vec<crate::types::ImportInfo> =
                                    capture_result
                                        .reemitted_imports
                                        .iter()
                                        .map(|ri| {
                                            let mut aliases = std::collections::HashMap::new();
                                            if let Some(ref imported) = ri.imported_name {
                                                aliases.insert(
                                                    ri.local_name.clone(),
                                                    imported.clone(),
                                                );
                                            }
                                            crate::types::ImportInfo {
                                                source: ri.source.clone(),
                                                specifiers: vec![ri.local_name.clone()],
                                                specifier_kinds: vec![ri.kind.clone()],
                                                specifier_aliases: aliases,
                                                is_qwik_core: false,
                                                span: (0, 0),
                                                assertion: ri.assertion.clone(),
                                            }
                                        })
                                        .collect();
                                needed_imports.extend(module_decl_imports);

                                // Update the segment with capture info
                                if let Some(seg_mut) = self
                                    .segments
                                    .iter_mut()
                                    .find(|s| s.span.0 == seg_span_0)
                                {
                                    seg_mut.captures =
                                        !capture_result.capture_names.is_empty();
                                    seg_mut.capture_names =
                                        capture_result.capture_names.clone();
                                    seg_mut.needed_imports = needed_imports;

                                    // Remap captures for inline component prop aliases.
                                    // If active_props_info is set (inline component),
                                    // replace destructured prop aliases with the raw props name.
                                    // E.g., capture "data" -> "_rawProps" when ({ data }) => ...
                                    if let Some(ref info) = self.active_props_info {
                                        if !info.prop_keys.is_empty() {
                                            let raw_name = &info.raw_props_name;
                                            let mut changed = false;
                                            for name in seg_mut.capture_names.iter_mut() {
                                                for (_, local_alias) in &info.prop_keys {
                                                    if name == local_alias {
                                                        *name = raw_name.clone();
                                                        changed = true;
                                                        break;
                                                    }
                                                }
                                            }
                                            if changed {
                                                seg_mut.capture_names.sort();
                                                seg_mut.capture_names.dedup();
                                                seg_mut.captures = !seg_mut.capture_names.is_empty();
                                            }
                                        }
                                    }
                                }

                                // Serialize the lambda body code for the segment module.
                                // This is the JSX event handler equivalent of what
                                // exit_expression does for regular $() calls.
                                let is_inline = entry_strategy::should_inline(
                                    &self.options.entry_strategy,
                                ) || matches!(
                                    self.options.entry_strategy,
                                    crate::types::EntryStrategy::Hoist
                                );
                                if !is_inline {
                                    let mut body_code = serialize_jsx_lambda_from_source(
                                        &self.source_code,
                                        span,
                                    );
                                    // Post-process body code for inline component prop aliases.
                                    // Body code is captured from original source text, so it still
                                    // has the original destructured prop aliases (e.g., `data.X`).
                                    // Replace them with `_rawProps.data.X` (or `props.data.X`).
                                    if let Some(ref info) = self.active_props_info {
                                        if !info.prop_keys.is_empty() {
                                            for (original_key, local_alias) in &info.prop_keys {
                                                let replacement = format!("{}.{}", info.raw_props_name, original_key);
                                                body_code = replace_identifier_in_body(&body_code, local_alias, &replacement);
                                            }
                                        }
                                    }
                                    // Inline const-literal values in body code.
                                    // SWC replaces references to const-literal bindings with
                                    // their values (e.g., STEP_2 -> 2). Skip module-level
                                    // consts (they're re-imported, not inlined).
                                    for (name, value) in &self.const_literal_bindings {
                                        if !self.collected.module_level_decls.contains(name) {
                                            body_code = replace_identifier_in_body(&body_code, name, value);
                                        }
                                    }
                                    if !body_code.is_empty() {
                                        self.segment_body_codes
                                            .push((seg_span_0, body_code));
                                    }
                                }

                                // Record replacement info so the JSX transform can
                                // replace the raw lambda with qrl()/inlinedQrl().
                                let seg_info = self.segments.iter()
                                    .find(|s| s.span.0 == seg_span_0)
                                    .cloned();
                                if let Some(seg_info) = seg_info {
                                    self.jsx_event_replacements.insert(
                                        span.0,
                                        JsxEventReplacement {
                                            segment_name: seg_info.name.clone(),
                                            hash: seg_info.hash.clone(),
                                            capture_names: seg_info.capture_names.clone(),
                                            is_inline,
                                            display_name: seg_info.display_name.clone(),
                                            body_span: seg_info.body_span,
                                        },
                                    );
                                }

                                // Pop the attribute name from stack_ctxt
                                self.stack_ctxt.pop();
                            }
                        }
                    }
                }
            }
        }

        self.create_jsx_event_segments_in_children(&element.children);

        // Pop element name from stack_ctxt
        if element_name.is_some() {
            self.stack_ctxt.pop();
        }
    }

    /// Scan JSX children for elements with $-suffixed attributes.
    fn create_jsx_event_segments_in_children<'b>(
        &mut self,
        children: &oxc::allocator::Vec<'b, JSXChild<'b>>,
    ) {
        for child in children {
            match child {
                JSXChild::Element(el) => {
                    self.create_jsx_event_segments_recursive(el);
                }
                JSXChild::Fragment(frag) => {
                    if self.options.transpile_jsx {
                        self.stack_ctxt.push("Fragment".to_string());
                    }
                    self.create_jsx_event_segments_in_children(&frag.children);
                    if self.options.transpile_jsx {
                        self.stack_ctxt.pop();
                    }
                }
                _ => {}
            }
        }
    }

    /// Replace JSX event handler lambda expressions with qrl()/inlinedQrl() calls.
    ///
    /// Walks a JSXElement (recursively including children) and replaces any
    /// `$`-suffixed attribute values whose lambda span matches a registered
    /// segment replacement with the appropriate `qrl(i_hash, "name", [caps])`
    /// or `inlinedQrl(fn, "name", [caps])` call.
    fn replace_jsx_event_handler_values<'a>(
        &mut self,
        expr: &mut Expression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        match expr {
            Expression::JSXElement(el) => {
                self.replace_jsx_element_handlers(el, ctx);
            }
            Expression::JSXFragment(frag) => {
                self.replace_jsx_children_handlers(&mut frag.children, ctx);
            }
            _ => {}
        }
    }

    /// Replace event handler lambdas in a JSXElement and recurse into children.
    /// Also injects q:p/q:ps attributes for iteration variables used by handlers.
    fn replace_jsx_element_handlers<'a>(
        &mut self,
        el: &mut JSXElement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Collect iteration variables used by handlers on THIS element.
        // We accumulate these during attribute processing and inject q:p/q:ps after.
        let mut element_used_iter_vars: Vec<String> = Vec::new();

        // Process this element's attributes
        for attr_item in &mut el.opening_element.attributes {
            if let JSXAttributeItem::Attribute(attr) = attr_item {
                let attr_name = match &attr.name {
                    JSXAttributeName::Identifier(ident) => ident.name.as_str().to_string(),
                    JSXAttributeName::NamespacedName(ns) => {
                        format!("{}:{}", ns.namespace.name, ns.name.name)
                    }
                };

                if !attr_name.ends_with('$') {
                    continue;
                }

                // Check if the value expression has a registered replacement
                let lambda_span_start = attr.value.as_ref().and_then(|val| {
                    if let JSXAttributeValue::ExpressionContainer(container) = val {
                        match &container.expression {
                            JSXExpression::ArrowFunctionExpression(arrow) => {
                                Some(arrow.span.start)
                            }
                            JSXExpression::FunctionExpression(func) => Some(func.span.start),
                            _ => None,
                        }
                    } else {
                        None
                    }
                });

                if let Some(span_start) = lambda_span_start {
                    // Check if this handler uses iteration variables (for q:p injection)
                    if let Some(used_vars) = self.iter_var_usage_by_handler.get(&span_start) {
                        for var_name in used_vars {
                            if !element_used_iter_vars.contains(var_name) {
                                element_used_iter_vars.push(var_name.clone());
                            }
                        }
                    }

                    if let Some(info) = self.jsx_event_replacements.get(&span_start).cloned() {
                        let dev_meta = self.make_qrl_dev_meta(
                            info.body_span.0,
                            info.body_span.1,
                            &info.display_name,
                        );
                        let replacement = if info.is_inline {
                            // inlinedQrl(handler_expr, "name", [captures])
                            let handler_expr =
                                if let Some(val) = std::mem::take(&mut attr.value) {
                                    match val {
                                        JSXAttributeValue::ExpressionContainer(container) => {
                                            let unboxed = container.unbox();
                                            match unboxed.expression {
                                                JSXExpression::ArrowFunctionExpression(arrow) => {
                                                    Expression::ArrowFunctionExpression(arrow)
                                                }
                                                JSXExpression::FunctionExpression(func) => {
                                                    Expression::FunctionExpression(func)
                                                }
                                                _ => ctx
                                                    .ast
                                                    .expression_identifier(SPAN, "undefined"),
                                            }
                                        }
                                        _ => ctx.ast.expression_identifier(SPAN, "undefined"),
                                    }
                                } else {
                                    ctx.ast.expression_identifier(SPAN, "undefined")
                                };
                            import_rewrite::build_inlined_qrl_call(
                                handler_expr,
                                &info.segment_name,
                                &info.capture_names,
                                dev_meta.as_ref(),
                                ctx,
                            )
                        } else if self.loop_depth > 0 {
                            // Inside a loop: hoist QRL to enclosing function body.
                            // Buffer the QRL components and replace inline with
                            // an identifier reference to the segment name.
                            let import_ident = format!("i_{}", info.hash);
                            // Drop the original lambda value
                            let _ = std::mem::take(&mut attr.value);
                            self.pending_loop_qrl_hoists.push((
                                import_ident,
                                info.segment_name.clone(),
                                info.capture_names.clone(),
                            ));
                            // Replace with identifier reference to the hoisted const
                            let seg_atom = ctx.ast.atom(&info.segment_name);
                            ctx.ast.expression_identifier(SPAN, seg_atom)
                        } else {
                            // qrl(i_hash, "name", [captures]) -- segment strategy
                            let import_ident = format!("i_{}", info.hash);
                            // Drop the original lambda value
                            let _ = std::mem::take(&mut attr.value);
                            import_rewrite::build_qrl_call(
                                &import_ident,
                                &info.segment_name,
                                &info.capture_names,
                                dev_meta.as_ref(),
                                ctx,
                            )
                        };

                        // Wrap the replacement in a JSXExpressionContainer
                        let container = ctx.ast.jsx_expression_container(
                            SPAN,
                            JSXExpression::from(replacement),
                        );
                        attr.value = Some(JSXAttributeValue::ExpressionContainer(
                            ctx.ast.alloc(container),
                        ));
                    }
                }
            }
        }

        // Inject q:p/q:ps attributes for iteration variables used by event handlers
        // on this element. This must happen here (before JSX transform) because by the
        // time transform_jsx_element_inner runs, the handler lambdas are already replaced
        // by QRL identifiers and we can't scan them for iteration variable usage.
        if !element_used_iter_vars.is_empty() {
            if element_used_iter_vars.len() == 1 {
                // q:p={iterVar}
                let var_name = &element_used_iter_vars[0];
                let ident_expr = ctx.ast.expression_identifier(SPAN, ctx.ast.atom(var_name));
                let container = ctx.ast.jsx_expression_container(
                    SPAN,
                    JSXExpression::from(ident_expr),
                );
                let ns_name = ctx.ast.jsx_namespaced_name(
                    SPAN,
                    ctx.ast.jsx_identifier(SPAN, "q"),
                    ctx.ast.jsx_identifier(SPAN, "p"),
                );
                let qp_attr = ctx.ast.jsx_attribute(
                    SPAN,
                    JSXAttributeName::NamespacedName(ctx.ast.alloc(ns_name)),
                    Some(JSXAttributeValue::ExpressionContainer(ctx.ast.alloc(container))),
                );
                el.opening_element.attributes.push(
                    JSXAttributeItem::Attribute(ctx.ast.alloc(qp_attr)),
                );
            } else {
                // q:ps={[var1, var2, ...]}
                let mut elements = ctx.ast.vec();
                for var_name in &element_used_iter_vars {
                    let ident = ctx.ast.expression_identifier(SPAN, ctx.ast.atom(var_name));
                    elements.push(ArrayExpressionElement::from(ident));
                }
                let arr = ctx.ast.expression_array(SPAN, elements);
                let container = ctx.ast.jsx_expression_container(
                    SPAN,
                    JSXExpression::from(arr),
                );
                let ns_name = ctx.ast.jsx_namespaced_name(
                    SPAN,
                    ctx.ast.jsx_identifier(SPAN, "q"),
                    ctx.ast.jsx_identifier(SPAN, "ps"),
                );
                let qps_attr = ctx.ast.jsx_attribute(
                    SPAN,
                    JSXAttributeName::NamespacedName(ctx.ast.alloc(ns_name)),
                    Some(JSXAttributeValue::ExpressionContainer(ctx.ast.alloc(container))),
                );
                el.opening_element.attributes.push(
                    JSXAttributeItem::Attribute(ctx.ast.alloc(qps_attr)),
                );
            }
        }

        // Recurse into children (JSXChild elements are part of the parent AST node,
        // not separate expressions, so they don't get their own exit_expression)
        self.replace_jsx_children_handlers(&mut el.children, ctx);
    }

    /// Recurse into JSX children to replace event handler lambdas.
    fn replace_jsx_children_handlers<'a>(
        &mut self,
        children: &mut oxc::allocator::Vec<'a, JSXChild<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        for child in children.iter_mut() {
            match child {
                JSXChild::Element(child_el) => {
                    self.replace_jsx_element_handlers(child_el, ctx);
                }
                JSXChild::Fragment(frag) => {
                    self.replace_jsx_children_handlers(&mut frag.children, ctx);
                }
                _ => {}
            }
        }
    }

    /// When transpile_jsx is false, rename $-suffixed event handler attributes
    /// on native JSX elements to their HTML form (e.g., onClick$ -> q-e:click).
    /// This runs AFTER segment extraction and QRL value replacement, so it
    /// doesn't interfere with those processes.
    fn rename_jsx_event_attrs<'a>(
        expr: &mut Expression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        match expr {
            Expression::JSXElement(el) => {
                Self::rename_jsx_element_event_attrs(el, ctx);
            }
            Expression::JSXFragment(frag) => {
                Self::rename_jsx_children_event_attrs(&mut frag.children, ctx);
            }
            _ => {}
        }
    }

    /// Rename event handler attributes on a single JSXElement and recurse
    /// into its children.
    fn rename_jsx_element_event_attrs<'a>(
        el: &mut JSXElement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Determine if this is a native element (lowercase first char)
        let is_native = match &el.opening_element.name {
            JSXElementName::Identifier(ident) => {
                ident.name.as_str().chars().next().is_some_and(|c| c.is_lowercase())
            }
            JSXElementName::IdentifierReference(ident) => {
                ident.name.as_str().chars().next().is_some_and(|c| c.is_lowercase())
            }
            _ => false,
        };

        if is_native {
            for attr_item in &mut el.opening_element.attributes {
                if let JSXAttributeItem::Attribute(attr) = attr_item {
                    if let JSXAttributeName::Identifier(ident) = &attr.name {
                        if let Some(html_attr) = jsx_event_to_html_attribute(ident.name.as_str()) {
                            if let Some(colon_pos) = html_attr.find(':') {
                                let ns_part = &html_attr[..colon_pos];
                                let name_part = &html_attr[colon_pos + 1..];
                                let ns_atom = ctx.ast.atom(ns_part);
                                let name_atom = ctx.ast.atom(name_part);
                                let ns_ident = JSXIdentifier { span: SPAN, name: ns_atom };
                                let name_ident = JSXIdentifier { span: SPAN, name: name_atom };
                                let ns_name = JSXNamespacedName {
                                    span: SPAN,
                                    namespace: ns_ident,
                                    name: name_ident,
                                };
                                attr.name = JSXAttributeName::NamespacedName(
                                    ctx.ast.alloc(ns_name),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Recurse into children
        Self::rename_jsx_children_event_attrs(&mut el.children, ctx);
    }

    /// Recurse into JSX children to rename event handler attributes.
    fn rename_jsx_children_event_attrs<'a>(
        children: &mut oxc::allocator::Vec<'a, JSXChild<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        for child in children.iter_mut() {
            match child {
                JSXChild::Element(child_el) => {
                    Self::rename_jsx_element_event_attrs(child_el, ctx);
                }
                JSXChild::Fragment(frag) => {
                    Self::rename_jsx_children_event_attrs(&mut frag.children, ctx);
                }
                _ => {}
            }
        }
    }
}

impl<'a> Traverse<'a, ()> for QwikTransform {
    fn enter_program(&mut self, program: &mut Program<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        // Store comments from the parsed program for use in comment-preserving codegen.
        // Comment is Copy, so we clone each one into a standard Vec.
        self.source_comments = program.comments.iter().copied().collect();
    }

    fn enter_call_expression(
        &mut self,
        call: &mut CallExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Check if this is an array iteration method call (.map, .filter, etc.)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let method_name = member.property.name.as_str();
            if matches!(
                method_name,
                "map"
                    | "filter"
                    | "forEach"
                    | "flatMap"
                    | "some"
                    | "every"
                    | "find"
                    | "findIndex"
                    | "reduce"
                    | "reduceRight"
            ) {
                self.loop_depth += 1;
                self.in_callback_depth += 1;
                // Extract callback parameters as iteration variables
                if let Some(first_arg) = call.arguments.first() {
                    let iteration_vars = extract_callback_params(first_arg);
                    self.iteration_var_stack.push(iteration_vars);
                } else {
                    self.iteration_var_stack.push(Vec::new());
                }
            }
        }

        // Push callee name to stack_ctxt, mirroring SWC's fold_call_expr.
        // SWC pushes the callee ident.sym for marker functions and all other
        // ident callees, but NOT for:
        // - Raw $() calls (handle_qsegment returns early without push)
        // - sync$(), inlinedQrl, _fnSignal, jsx functions
        self.call_expr_ctxt_depths.push(self.stack_ctxt.len());

        let kind = self.is_dollar_call(call);

        // Only push callee name when it's NOT a raw $() call.
        // SWC's handle_qsegment returns before the push at line 3186/3226.
        if let Expression::Identifier(ident) = &call.callee {
            let is_raw_dollar = matches!(kind, Some(DollarCallKind::RawDollar));
            if !is_raw_dollar {
                self.stack_ctxt.push(ident.name.as_str().to_string());
            }
        }

        let Some(kind) = kind else {
            // C05: Check for $-suffixed exported functions without Qrl counterpart.
            // SWC's marker_functions includes both core imports AND local exports ending with $.
            // When a $-call is NOT a core import but IS a local export, SWC checks for the
            // corresponding Qrl export. If not found, it emits C05 "MissingQrlImplementation".
            if let Expression::Identifier(ident) = &call.callee {
                let name = ident.name.as_str();
                if name.ends_with('$') && !name.starts_with('_') {
                    // Check if this function is exported from the same file
                    let is_exported = self.collected.exported_local_names.contains(name)
                        || self.collected.module_exports.iter().any(|e| e.name == name);
                    if is_exported {
                        let qrl_name = crate::words::dollar_to_qrl_name(name);
                        let has_qrl_export = self.collected.exported_local_names.contains(&qrl_name)
                            || self.collected.module_exports.iter().any(|e| e.name == qrl_name);
                        if !has_qrl_export {
                            self.diagnostics.push(crate::types::Diagnostic {
                                scope: "optimizer".to_string(),
                                category: crate::types::DiagnosticCategory::Error,
                                code: Some("C05".to_string()),
                                file: self.filename.clone(),
                                message: format!(
                                    "Found '{}' but did not find the corresponding '{}' exported in the same file. Please check that it is exported and spelled correctly",
                                    name, qrl_name
                                ),
                                highlights: Some(vec![compute_highlight_from_span(
                                    &self.source_code,
                                    ident.span.start,
                                    ident.span.end,
                                )]),
                                suggestions: None,
                            });
                        }
                    }
                }
            }
            return;
        };

        if call.arguments.is_empty() {
            if let DollarCallKind::Named(ref name) = kind {
                let qrl_name = crate::words::dollar_to_qrl_name(name);
                if self.dollar_call_stack.is_empty() {
                    // Top-level: Qrl import goes to main module
                    if !self.import_tracker.qrl_imports.contains(&qrl_name) {
                        self.import_tracker.record_synthetic_import(&qrl_name);
                        self.import_tracker.qrl_imports.push(qrl_name);
                    }
                } else {
                    // Nested: Qrl import goes to parent segment
                    let parent_display_name = self.dollar_call_stack.last().unwrap().clone();
                    self.import_tracker.record_synthetic_import(&qrl_name);
                    self.pending_segment_qrl_imports
                        .push((parent_display_name, qrl_name));
                }
            }
            return;
        }

        if let DollarCallKind::Named(ref name) = kind {
            if name == "sync$" {
                self.pending_sync_calls.insert(call.span.start);
                self.capture_stack.push((Vec::new(), HashSet::new()));
                self.invalid_decl_stack.push(HashSet::new());
                return;
            }
            if name == "component$" || name == "useResource$" {
                if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first() {
                    // Build set of import names for const-checking default values
                    let import_names: HashSet<String> = self
                        .collected
                        .module_imports
                        .iter()
                        .flat_map(|imp| imp.specifiers.iter().cloned())
                        .collect();
                    let mut info = props_destructuring::analyze_props_destructuring(
                        &arrow.params,
                        &import_names,
                    );
                    if info.needs_transform {
                        if info.rest_name.is_some() {
                            if !self.import_tracker.needs_rest_props {
                                self.import_tracker.needs_rest_props = true;
                                self.import_tracker.record_synthetic_import("_restProps");
                            }
                        }
                        self.active_props_info = Some(info);
                    } else if name == "component$" {
                        // Non-destructured props param body destructuring detection
                        // only applies to component$ (not useResource$ or other hooks).
                        if let Some(ref param_name) = info.props_param_name {
                            let body_destr = props_destructuring::detect_body_destructuring(
                                &arrow.body.statements,
                                param_name,
                            );
                            if let Some(ref body_info) = body_destr {
                                info.prop_keys = body_info.prop_keys.clone();
                                info.rest_name = body_info.rest_name.clone();
                            }
                            self.active_props_info = Some(info);
                        }
                    }
                }
            }
        }

        self.capture_stack.push((Vec::new(), HashSet::new()));
        self.invalid_decl_stack.push(HashSet::new());

        if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first() {
            for param in &arrow.params.items {
                self.collect_binding_pattern_names(&param.pattern);
            }
            if let Some(rest) = &arrow.params.rest {
                self.collect_binding_pattern_names(&rest.rest.argument);
            }
        }

        // record_segment reads stack_ctxt and pushes to segment_stack + dollar_call_stack
        let _segment = self.record_segment(call, &kind);

        if let DollarCallKind::Named(ref name) = kind {
            if self.should_strip_ctx_name(name) {
                self.stripped_segments.insert(call.span.start);
            }
        }

        self.pending_dollar_calls.insert(call.span.start);
    }

    fn exit_call_expression(
        &mut self,
        call: &mut CallExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Clean up iteration tracking for array methods
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let method_name = member.property.name.as_str();
            if matches!(
                method_name,
                "map"
                    | "filter"
                    | "forEach"
                    | "flatMap"
                    | "some"
                    | "every"
                    | "find"
                    | "findIndex"
                    | "reduce"
                    | "reduceRight"
            ) {
                self.iteration_var_stack.pop();
                self.loop_depth -= 1;
                self.in_callback_depth = self.in_callback_depth.saturating_sub(1);
            }
        }

        // Pop callee name from stack_ctxt
        if let Some(depth) = self.call_expr_ctxt_depths.pop() {
            self.stack_ctxt.truncate(depth);
        }
    }

    fn enter_identifier_reference(
        &mut self,
        ident: &mut IdentifierReference<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // If we're inside a $()-body, collect the identifier name for capture analysis.
        if let Some(frame) = self.capture_stack.last_mut() {
            frame.0.push(ident.name.as_str().to_string());
        }
    }

    fn enter_variable_declaration(
        &mut self,
        decl: &mut VariableDeclaration<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Populate const_bindings from `const` declarations whose initializer is
        // "return-static". This mirrors SWC's fold_var_decl which only marks a const
        // binding as `Var(true)` when `is_const && is_static`.
        //
        // SWC's `is_return_static` classifies an initializer as static when it is:
        //   - A call to a function ending with `$` (component$, $, etc.)
        //   - A call to a function ending with `Qrl` (inlinedQrl, componentQrl, etc.)
        //   - A call to a function starting with `use` (useSignal, useStore, etc.)
        //   - No initializer (None)
        //
        // This means `const x = signal.value + "foo"` is NOT treated as const,
        // while `const sig = useSignal()` IS treated as const.
        if decl.kind == VariableDeclarationKind::Const {
            for declarator in &decl.declarations {
                if is_init_return_static(&declarator.init) {
                    collect_const_binding_names(
                        &declarator.id,
                        &mut self.import_tracker.const_bindings,
                    );
                }
                // Track const bindings with simple literal initializers.
                // SWC inlines these in segment bodies and doesn't capture them.
                if let BindingPattern::BindingIdentifier(ref ident) = declarator.id {
                    if let Some(ref init) = declarator.init {
                        if let Some(literal_str) = get_literal_string(init) {
                            self.const_literal_bindings
                                .insert(ident.name.as_str().to_string(), literal_str);
                        }
                    }
                }
            }
        }
    }

    fn enter_variable_declarator(
        &mut self,
        declarator: &mut VariableDeclarator<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // If we're inside a $()-body, record variable declarations as body-local.
        if !self.capture_stack.is_empty() {
            self.collect_binding_pattern_names(&declarator.id);
        }

        // Push variable name to stack_ctxt (mirrors SWC's fold_var_declarator)
        self.var_decl_ctxt_depths.push(self.stack_ctxt.len());
        if let BindingPattern::BindingIdentifier(ref ident) = declarator.id {
            self.stack_ctxt.push(ident.name.as_str().to_string());
        }
    }

    fn exit_variable_declarator(
        &mut self,
        _declarator: &mut VariableDeclarator<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(depth) = self.var_decl_ctxt_depths.pop() {
            self.stack_ctxt.truncate(depth);
        }
    }

    fn enter_function(&mut self, func: &mut Function<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        // Push function name to stack_ctxt for named function declarations
        // (mirrors SWC's fold_fn_decl)
        self.fn_decl_ctxt_depths.push(self.stack_ctxt.len());
        if let Some(ref id) = func.id {
            let name = id.name.as_str().to_string();
            self.stack_ctxt.push(name.clone());
            // Track function declarations as "invalid" for capture purposes.
            // SWC treats IdentType::Fn as invalid_decl: they're NOT captured
            // but emit C02 diagnostics instead.
            if let Some(frame) = self.invalid_decl_stack.last_mut() {
                frame.insert(name);
            }
        }
        // When inside a $()-body capture frame, collect nested function params
        // as body_local_decls so they are excluded from captures. SWC's scope
        // analysis naturally handles this; OXC's simplified approach needs explicit
        // tracking of nested function/arrow parameters.
        if !self.capture_stack.is_empty() {
            for param in &func.params.items {
                self.collect_binding_pattern_names(&param.pattern);
            }
            if let Some(rest) = &func.params.rest {
                self.collect_binding_pattern_names(&rest.rest.argument);
            }
        }
        // Save and set root_jsx_mode for function bodies (mirrors SWC fold_fn_expr)
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_function(&mut self, func: &mut Function<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if let Some(depth) = self.fn_decl_ctxt_depths.pop() {
            self.stack_ctxt.truncate(depth);
        }
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
        // Flush pending QRL hoists when exiting a function that's not inside a loop.
        // This prepends hoisted const declarations to the function body.
        if self.loop_depth == 0 && !self.pending_loop_qrl_hoists.is_empty() {
            if let Some(ref mut body) = func.body {
                let hoists = std::mem::take(&mut self.pending_loop_qrl_hoists);
                flush_qrl_hoists_to_body(&mut body.statements, &hoists, ctx);
            }
        }
    }

    fn enter_class(&mut self, class: &mut Class<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        // Track class declarations as "invalid" for capture purposes.
        // SWC treats IdentType::Class as invalid_decl: they're NOT captured
        // but emit C02 diagnostics instead (matching function declarations).
        if let Some(ref id) = class.id {
            if let Some(frame) = self.invalid_decl_stack.last_mut() {
                frame.insert(id.name.as_str().to_string());
            }
        }
    }

    fn enter_arrow_function_expression(
        &mut self,
        arrow: &mut ArrowFunctionExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // When inside a $()-body capture frame, collect nested arrow params
        // as body_local_decls so they are excluded from captures. This prevents
        // parameters like `({ aaa }) => aaa` from leaking into parent captures.
        if !self.capture_stack.is_empty() {
            for param in &arrow.params.items {
                self.collect_binding_pattern_names(&param.pattern);
            }
            if let Some(rest) = &arrow.params.rest {
                self.collect_binding_pattern_names(&rest.rest.argument);
            }
        }
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_arrow_function_expression(
        &mut self,
        arrow: &mut ArrowFunctionExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Flush pending QRL hoists when exiting an arrow that's not inside a loop.
        if self.loop_depth == 0 && !self.pending_loop_qrl_hoists.is_empty() {
            let hoists = std::mem::take(&mut self.pending_loop_qrl_hoists);
            flush_qrl_hoists_to_body(&mut arrow.body.statements, &hoists, ctx);
        }
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_export_default_declaration(
        &mut self,
        _decl: &mut ExportDefaultDeclaration<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Push file stem (or folder name for index files) to stack_ctxt
        // (mirrors SWC's fold_export_default_expr)
        self.export_default_ctxt_depths
            .push(self.stack_ctxt.len());

        let mut file_stem = self.file_stem();

        if file_stem == "index" {
            // Use folder name instead (mirrors SWC's rel_dir.file_name())
            // Handle both / and \ path separators
            let dir_part = {
                let last_sep = self
                    .filename
                    .rfind(|c: char| c == '/' || c == '\\')
                    .unwrap_or(0);
                if last_sep > 0 {
                    &self.filename[..last_sep]
                } else {
                    ""
                }
            };
            if !dir_part.is_empty() {
                let folder = dir_part
                    .rsplit(|c: char| c == '/' || c == '\\')
                    .next()
                    .unwrap_or(dir_part);
                if !folder.is_empty() {
                    file_stem = folder.to_string();
                }
            }
        }

        self.stack_ctxt.push(file_stem);

        // Detect inline component pattern: export default ({ data }) => ...
        // SWC rewrites destructured props to _rawProps for inline components too.
        // This sets up active_props_info BEFORE the JSX transform runs (which
        // happens bottom-up in exit hooks), enabling _fnSignal wrapping for
        // prop member expressions.
        if let ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) = &_decl.declaration {
            let import_names: HashSet<String> = self
                .collected
                .module_imports
                .iter()
                .flat_map(|imp| imp.specifiers.iter().cloned())
                .collect();
            let mut info = props_destructuring::analyze_props_destructuring(
                &arrow.params,
                &import_names,
            );
            if info.needs_transform {
                // Parameter destructuring: ({ data }) => ...
                if info.rest_name.is_some() {
                    if !self.import_tracker.needs_rest_props {
                        self.import_tracker.needs_rest_props = true;
                        self.import_tracker.record_synthetic_import("_restProps");
                    }
                }
                self.active_props_info = Some(info);
            } else if let Some(ref param_name) = info.props_param_name {
                // Non-destructured param with possible body destructuring:
                // (props) => { const { data } = props; ... }
                let body_destr = props_destructuring::detect_body_destructuring(
                    &arrow.body.statements,
                    param_name,
                );
                if let Some(ref body_info) = body_destr {
                    info.prop_keys = body_info.prop_keys.clone();
                    info.rest_name = body_info.rest_name.clone();
                }
                self.active_props_info = Some(info);
            }
        }
    }

    fn exit_export_default_declaration(
        &mut self,
        decl: &mut ExportDefaultDeclaration<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(depth) = self.export_default_ctxt_depths.pop() {
            self.stack_ctxt.truncate(depth);
        }

        // Inline component _rawProps rewrite.
        // Mirrors the logic in exit_expression for component$ calls (lines ~2692-2750),
        // adapted for ExportDefaultDeclaration.
        let props_info = self.active_props_info.take();
        if let Some(ref info) = props_info {
            if let ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) = &mut decl.declaration {
                if info.needs_transform {
                    // 1. Replace the destructured parameter with _rawProps
                    if !arrow.params.items.is_empty() {
                        let new_pattern = ctx.ast.binding_pattern_binding_identifier(
                            SPAN,
                            ctx.ast.atom(&info.raw_props_name),
                        );
                        let new_param = ctx.ast.formal_parameter(
                            SPAN,
                            ctx.ast.vec(),
                            new_pattern,
                            None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
                            None::<oxc::allocator::Box<'a, Expression<'a>>>,
                            false,
                            None,
                            false,
                            false,
                        );
                        arrow.params.items[0] = new_param;
                        arrow.params.rest = None;
                    }

                    // 2. If rest pattern: insert const rest = _restProps(_rawProps, [...])
                    if let Some(ref rest_name) = info.rest_name {
                        if !self.import_tracker.needs_rest_props {
                            self.import_tracker.needs_rest_props = true;
                            self.import_tracker.record_synthetic_import("_restProps");
                        }
                        let excluded_keys: Vec<String> =
                            info.prop_keys.iter().map(|(key, _)| key.clone()).collect();
                        let rest_stmt = props_destructuring::build_rest_props_declaration(
                            rest_name,
                            &info.raw_props_name,
                            &excluded_keys,
                            ctx,
                        );
                        let mut old_stmts = ctx.ast.vec();
                        std::mem::swap(&mut arrow.body.statements, &mut old_stmts);
                        let mut new_stmts = ctx.ast.vec_with_capacity(1 + old_stmts.len());
                        new_stmts.push(rest_stmt);
                        for s in old_stmts {
                            new_stmts.push(s);
                        }
                        arrow.body.statements = new_stmts;
                    }

                    // 3. Rewrite body references: data -> _rawProps.data
                    let prop_map: Vec<(String, String)> = info
                        .prop_keys
                        .iter()
                        .map(|(key, local)| (local.clone(), key.clone()))
                        .collect();

                    if !prop_map.is_empty() {
                        props_destructuring::rewrite_body_statements(
                            &mut arrow.body.statements,
                            &prop_map,
                            &info.raw_props_name,
                            &info.prop_defaults,
                            ctx,
                        );
                    }
                } else if info.props_param_name.is_some() && !info.prop_keys.is_empty() {
                    // Body destructuring: (props) => { const { data } = props; ... }
                    // Detect and rewrite, similar to the component$ body destructuring logic.
                    let param_name = info.props_param_name.as_ref().unwrap();
                    let body_destr = props_destructuring::detect_body_destructuring(
                        &arrow.body.statements,
                        param_name,
                    );

                    if let Some(body_info) = body_destr {
                        // Remove the destructuring statement
                        let removed_stmt = arrow.body.statements.remove(body_info.stmt_index);
                        let _ = removed_stmt;

                        // If rest pattern: insert const rest = _restProps(props, [...])
                        if let Some(ref rest_name) = body_info.rest_name {
                            if !self.import_tracker.needs_rest_props {
                                self.import_tracker.needs_rest_props = true;
                                self.import_tracker.record_synthetic_import("_restProps");
                            }
                            let excluded_keys: Vec<String> =
                                body_info.prop_keys.iter().map(|(key, _)| key.clone()).collect();
                            let rest_stmt = props_destructuring::build_rest_props_declaration(
                                rest_name,
                                param_name,
                                &excluded_keys,
                                ctx,
                            );
                            arrow.body.statements.insert(body_info.stmt_index, rest_stmt);
                        }

                        // Rewrite body references: data -> props.data
                        let prop_map: Vec<(String, String)> = body_info
                            .prop_keys
                            .iter()
                            .map(|(key, local)| (local.clone(), key.clone()))
                            .collect();

                        if !prop_map.is_empty() {
                            props_destructuring::rewrite_body_statements(
                                &mut arrow.body.statements,
                                &prop_map,
                                param_name,
                                &info.prop_defaults,
                                ctx,
                            );
                        }
                    }
                }

                // Note: segment body strings and capture names are post-processed
                // inline during create_jsx_event_segments_recursive (when segment
                // bodies and captures are first created), not here. This ensures
                // the JSX transform sees the correct capture names when building
                // qrl() calls in exit_expression.
            }
        }
    }

    fn enter_jsx_element(
        &mut self,
        el: &mut JSXElement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Push element tag name to stack_ctxt (mirrors SWC's fold_jsx_element)
        match &el.opening_element.name {
            JSXElementName::Identifier(ident) => {
                let is_native = ident.name.as_str().chars().next().is_some_and(|c| c.is_lowercase());
                self.stack_ctxt.push(ident.name.as_str().to_string());
                self.jsx_element_is_native.push(is_native);
            }
            JSXElementName::IdentifierReference(ident) => {
                // Component JSX elements (capital first letter) are IdentifierReference in OXC
                self.stack_ctxt.push(ident.name.as_str().to_string());
                self.jsx_element_is_native.push(false);
            }
            _ => {
                // For member expressions, namespaced names, etc.
                self.jsx_element_is_native.push(false);
            }
        }

        // Save root_jsx_mode and set to false for children processing.
        // SWC's handle_jsx (lines 877-878): saves prev, sets root_jsx_mode=false for children.
        // After children are processed, SWC restores root_jsx_mode (line 919).
        // In OXC bottom-up traversal, enter_jsx_element runs top-down, so this correctly
        // ensures children see root_jsx_mode=false. The restore in exit_jsx_element
        // happens after all children's exit_expression calls, so when the parent's
        // exit_expression runs it sees the restored value.
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = false;
    }

    fn exit_jsx_element(
        &mut self,
        el: &mut JSXElement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if matches!(
            el.opening_element.name,
            JSXElementName::Identifier(_) | JSXElementName::IdentifierReference(_)
        ) {
            self.stack_ctxt.pop();
        }
        self.jsx_element_is_native.pop();

        // Restore root_jsx_mode (mirrors SWC handle_jsx line 919: root_jsx_mode = prev)
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_jsx_fragment(
        &mut self,
        _frag: &mut JSXFragment<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Push "Fragment" to stack_ctxt only when JSX is being transpiled.
        // In SWC, fragments become _jsxQ(Fragment, ...) calls after JSX transform,
        // and handle_jsx pushes the first arg "Fragment" to stack_ctxt.
        // When JSX is NOT transpiled, raw <> fragments don't push anything.
        if self.options.transpile_jsx {
            self.stack_ctxt.push("Fragment".to_string());
        }

        // Save/restore root_jsx_mode for fragment children (mirrors SWC save/restore pattern)
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = false;
    }

    fn exit_jsx_fragment(
        &mut self,
        _frag: &mut JSXFragment<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.options.transpile_jsx {
            self.stack_ctxt.pop();
        }
        // Restore root_jsx_mode
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_jsx_attribute(
        &mut self,
        attr: &mut JSXAttribute<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Push attribute name to stack_ctxt (mirrors SWC's fold_jsx_attr)
        let is_native = self.jsx_element_is_native.last().copied().unwrap_or(false);
        match &attr.name {
            JSXAttributeName::Identifier(ident) => {
                if is_native {
                    if let Some(html_attr) = jsx_event_to_html_attribute(ident.name.as_str()) {
                        self.stack_ctxt.push(html_attr);
                    } else {
                        self.stack_ctxt.push(ident.name.as_str().to_string());
                    }
                } else {
                    self.stack_ctxt.push(ident.name.as_str().to_string());
                }
            }
            JSXAttributeName::NamespacedName(ns) => {
                // Push "ns-name" format (e.g., "host-onClick$")
                self.stack_ctxt
                    .push(format!("{}-{}", ns.namespace.name, ns.name.name));
            }
        }
    }

    fn exit_jsx_attribute(
        &mut self,
        _attr: &mut JSXAttribute<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.stack_ctxt.pop();
    }

    // -----------------------------------------------------------------------
    // Loop tracking: enter/exit hooks for for/for-in/for-of/while statements
    // -----------------------------------------------------------------------

    fn enter_for_statement(
        &mut self,
        node: &mut ForStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
        self.loop_depth += 1;
        // Extract iteration variable from init: for (let i = ...) or for (var i = ...)
        let iteration_vars =
            if let Some(ForStatementInit::VariableDeclaration(ref decl)) = node.init {
                decl.declarations
                    .first()
                    .and_then(|d| {
                        if let BindingPattern::BindingIdentifier(ref ident) = d.id {
                            Some(vec![ident.name.to_string()])
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        self.iteration_var_stack.push(iteration_vars);
    }

    fn exit_for_statement(
        &mut self,
        _node: &mut ForStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
        self.loop_depth -= 1;
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_for_in_statement(
        &mut self,
        node: &mut ForInStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
        self.loop_depth += 1;
        let iteration_vars = match &node.left {
            ForStatementLeft::VariableDeclaration(decl) => decl
                .declarations
                .first()
                .and_then(|d| {
                    if let BindingPattern::BindingIdentifier(ref ident) = d.id {
                        Some(vec![ident.name.to_string()])
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                vec![ident.name.to_string()]
            }
            _ => Vec::new(),
        };
        self.iteration_var_stack.push(iteration_vars);
    }

    fn exit_for_in_statement(
        &mut self,
        _node: &mut ForInStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
        self.loop_depth -= 1;
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_for_of_statement(
        &mut self,
        node: &mut ForOfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
        self.loop_depth += 1;
        let iteration_vars = match &node.left {
            ForStatementLeft::VariableDeclaration(decl) => decl
                .declarations
                .first()
                .and_then(|d| {
                    if let BindingPattern::BindingIdentifier(ref ident) = d.id {
                        Some(vec![ident.name.to_string()])
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                vec![ident.name.to_string()]
            }
            _ => Vec::new(),
        };
        self.iteration_var_stack.push(iteration_vars);
    }

    fn exit_for_of_statement(
        &mut self,
        _node: &mut ForOfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
        self.loop_depth -= 1;
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_while_statement(
        &mut self,
        node: &mut WhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
        self.loop_depth += 1;
        // Extract iteration variable from test: while (i < ...) => extract "i"
        let iteration_vars = match &node.test {
            Expression::BinaryExpression(bin) => {
                if let Expression::Identifier(ref ident) = bin.left {
                    vec![ident.name.to_string()]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        };
        self.iteration_var_stack.push(iteration_vars);
    }

    fn exit_while_statement(
        &mut self,
        _node: &mut WhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
        self.loop_depth -= 1;
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_do_while_statement(
        &mut self,
        _stmt: &mut DoWhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_do_while_statement(
        &mut self,
        _stmt: &mut DoWhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_if_statement(
        &mut self,
        _stmt: &mut IfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_if_statement(
        &mut self,
        _stmt: &mut IfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_block_statement(
        &mut self,
        _stmt: &mut BlockStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_block_statement(
        &mut self,
        _stmt: &mut BlockStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn enter_return_statement(
        &mut self,
        _stmt: &mut ReturnStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_return_statement(
        &mut self,
        _stmt: &mut ReturnStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    // SWC's fold_cond_expr sets root_jsx_mode=true for ternary expression branches.
    // This ensures JSX elements inside `cond ? <A/> : <B/>` get auto-generated keys.
    fn enter_conditional_expression(
        &mut self,
        _expr: &mut ConditionalExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_conditional_expression(
        &mut self,
        _expr: &mut ConditionalExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    // SWC's fold_bin_expr sets root_jsx_mode=true for binary/logical expressions.
    // This ensures JSX elements inside `cond && <A/>` get auto-generated keys.
    fn enter_logical_expression(
        &mut self,
        _expr: &mut LogicalExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.root_jsx_mode_stack.push(self.root_jsx_mode);
        self.root_jsx_mode = true;
    }

    fn exit_logical_expression(
        &mut self,
        _expr: &mut LogicalExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if let Some(prev) = self.root_jsx_mode_stack.pop() {
            self.root_jsx_mode = prev;
        }
    }

    fn exit_expression(&mut self, expr: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        // Pre-scan JSX elements for $-suffixed event handler attributes.
        match expr {
            Expression::JSXElement(_) => {
                if let Expression::JSXElement(el) = &*expr {
                    self.create_jsx_event_segments_recursive(el);
                }
            }
            Expression::JSXFragment(_) => {
                if let Expression::JSXFragment(frag) = &*expr {
                    if self.options.transpile_jsx {
                        self.stack_ctxt.push("Fragment".to_string());
                    }
                    self.create_jsx_event_segments_in_children(&frag.children);
                    if self.options.transpile_jsx {
                        self.stack_ctxt.pop();
                    }
                }
            }
            _ => {}
        }

        // Replace JSX event handler lambda expressions with qrl()/inlinedQrl() calls
        // BEFORE the JSX transform runs, so the _jsxSorted output has the correct values.
        if !self.jsx_event_replacements.is_empty() {
            self.replace_jsx_event_handler_values(expr, ctx);
        }

        // When transpile_jsx is false, rename $-suffixed event handler attributes
        // to their HTML form (e.g., onClick$ -> q-e:click as NamespacedName).
        // This MUST run AFTER create_jsx_event_segments_recursive and
        // replace_jsx_event_handler_values, which rely on the $ suffix to identify
        // event handler attributes for segment extraction and QRL wrapping.
        // When transpile_jsx is true, the JSX transform module handles this
        // during _jsxSorted() call construction.
        if !self.options.transpile_jsx {
            Self::rename_jsx_event_attrs(expr, ctx);
        }

        if self.options.transpile_jsx {
            let destr_props: Option<Vec<(String, String)>> =
                self.active_props_info.as_ref().map(|info| {
                    info.prop_keys
                        .iter()
                        .map(|(key, local)| (local.clone(), key.clone()))
                        .collect()
                });
            let destr_props_ref = destr_props.as_deref();
            let props_param_name: Option<String> = self.active_props_info.as_ref()
                .and_then(|info| info.props_param_name.clone());
            let props_param_ref = props_param_name.as_deref();

            // Take hoisted_function_stmts out to avoid borrow conflict with &mut self
            let mut hoisted_stmts = std::mem::take(&mut self.hoisted_function_stmts);
            let module_imports = &self.collected.module_imports;
            let loop_depth = self.loop_depth;
            let iteration_vars = self.current_iteration_vars();

            let root_mode = self.root_jsx_mode;
            let key_prefix = self.jsx_key_prefix.clone();

            match expr {
                Expression::JSXElement(_) => {
                    let placeholder = ctx.ast.expression_null_literal(SPAN);
                    let old_expr = std::mem::replace(expr, placeholder);
                    if let Expression::JSXElement(el) = old_expr {
                        let result = transform_jsx_element_inner(
                            el.unbox(),
                            &mut self.import_tracker,
                            ctx,
                            destr_props_ref,
                            module_imports,
                            &mut hoisted_stmts,
                            loop_depth,
                            &iteration_vars,
                            props_param_ref,
                            root_mode,
                            &key_prefix,
                        );
                        *expr = result;
                    }
                    // SWC saves/restores root_jsx_mode inside handle_jsx (line 877/919),
                    // so root_jsx_mode is unchanged after processing. In bottom-up traversal,
                    // children have already been processed, so we must NOT set root_jsx_mode=false
                    // here (it would affect parent/sibling elements incorrectly).
                    // Children already receive root_jsx_mode=false via the parameter in
                    // transform_jsx_children (line 2123 passes false).
                    self.hoisted_function_stmts = hoisted_stmts;
                    return;
                }
                Expression::JSXFragment(_) => {
                    let placeholder = ctx.ast.expression_null_literal(SPAN);
                    let old_expr = std::mem::replace(expr, placeholder);
                    if let Expression::JSXFragment(frag) = old_expr {
                        let result = transform_jsx_fragment_inner(
                            frag.unbox(),
                            &mut self.import_tracker,
                            ctx,
                            destr_props_ref,
                            module_imports,
                            &mut hoisted_stmts,
                            loop_depth,
                            &iteration_vars,
                            props_param_ref,
                            root_mode,
                            &key_prefix,
                        );
                        *expr = result;
                    }
                    self.hoisted_function_stmts = hoisted_stmts;
                    return;
                }
                _ => {}
            }
            self.hoisted_function_stmts = hoisted_stmts;
        }

        if let Expression::CallExpression(call) = expr {
            // Handle sync$() calls -- replace with _qrlSync(fn, "stringified_fn")
            if self.pending_sync_calls.remove(&call.span.start) {
                self.capture_stack.pop();
                self.invalid_decl_stack.pop();

                let body_expr = if !call.arguments.is_empty() {
                    let placeholder =
                        Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                    let body_arg = std::mem::replace(&mut call.arguments[0], placeholder);
                    Some(argument_to_expression(body_arg, ctx))
                } else {
                    None
                };

                if let Some(fn_expr) = body_expr {
                    let mut codegen = oxc::codegen::Codegen::new();
                    codegen.print_expression(&fn_expr);
                    let fn_string = codegen.into_source_text();
                    let minified = minify_fn_string(&fn_string);

                    let replacement = import_rewrite::build_qrl_sync_call(fn_expr, &minified, ctx);

                    // the top level (not inside a $-body that will be extracted).
                    let is_segment_strategy =
                        !entry_strategy::should_inline(&self.options.entry_strategy)
                            && !matches!(
                                self.options.entry_strategy,
                                crate::types::EntryStrategy::Hoist
                            );
                    let inside_dollar_body = !self.capture_stack.is_empty();
                    if !(is_segment_strategy && inside_dollar_body) {
                        if !self.import_tracker.needs_qrl_sync {
                            self.import_tracker.needs_qrl_sync = true;
                            self.import_tracker.record_synthetic_import("_qrlSync");
                        }
                    }

                    *expr = replacement;
                }
                return;
            }

            if !self.pending_dollar_calls.remove(&call.span.start) {
                return;
            }

            let kind = match self.is_dollar_call(call) {
                Some(k) => k,
                None => return,
            };

            // Pop segment_stack and dollar_call_stack (pushed in record_segment)
            self.segment_stack.pop();
            self.dollar_call_stack.pop();

            let is_component_exit =
                matches!(&kind, DollarCallKind::Named(name) if name == "component$");
            let is_props_rewrite_exit =
                matches!(&kind, DollarCallKind::Named(name) if name == "component$" || name == "useResource$");
            let props_info = if is_props_rewrite_exit {
                self.active_props_info.take()
            } else {
                None
            };
            if let Some(ref info) = props_info {
                if info.needs_transform {
                    // Standard destructured props: replace parameter, rewrite references
                    if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first_mut() {
                        if !arrow.params.items.is_empty() {
                            let new_pattern = ctx.ast.binding_pattern_binding_identifier(
                                SPAN,
                                ctx.ast.atom(&info.raw_props_name),
                            );
                            let new_param = ctx.ast.formal_parameter(
                                SPAN,
                                ctx.ast.vec(),
                                new_pattern,
                                None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
                                None::<oxc::allocator::Box<'a, Expression<'a>>>,
                                false,
                                None,
                                false,
                                false,
                            );
                            arrow.params.items[0] = new_param;
                            arrow.params.rest = None;
                        }

                        if let Some(ref rest_name) = info.rest_name {
                            let excluded_keys: Vec<String> =
                                info.prop_keys.iter().map(|(key, _)| key.clone()).collect();
                            let rest_stmt = props_destructuring::build_rest_props_declaration(
                                rest_name,
                                &info.raw_props_name,
                                &excluded_keys,
                                ctx,
                            );

                            let mut old_stmts = ctx.ast.vec();
                            std::mem::swap(&mut arrow.body.statements, &mut old_stmts);
                            let mut new_stmts = ctx.ast.vec_with_capacity(1 + old_stmts.len());
                            new_stmts.push(rest_stmt);
                            for s in old_stmts {
                                new_stmts.push(s);
                            }
                            arrow.body.statements = new_stmts;
                        }

                        let prop_map: Vec<(String, String)> = info
                            .prop_keys
                            .iter()
                            .map(|(key, local)| (local.clone(), key.clone()))
                            .collect();

                        if !prop_map.is_empty() {
                            props_destructuring::rewrite_body_statements(
                                &mut arrow.body.statements,
                                &prop_map,
                                &info.raw_props_name,
                                &info.prop_defaults,
                                ctx,
                            );
                        }
                    }
                } else if is_component_exit {
                    if let Some(ref param_name) = info.props_param_name {
                    // Non-destructured props param (e.g., `(props) =>`).
                    // Check for body destructuring: `const { "bind:value": bindValue } = props;`
                    // This only applies to component$ (not useResource$ or other hooks).
                    if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first_mut() {
                        let body_destr = props_destructuring::detect_body_destructuring(
                            &arrow.body.statements,
                            param_name,
                        );

                        if let Some(body_info) = body_destr {
                            // Remove the destructuring statement
                            let removed_stmt = arrow.body.statements.remove(body_info.stmt_index);
                            let _ = removed_stmt;

                            // If rest pattern: insert `const rest = _restProps(props, [...])`
                            if let Some(ref rest_name) = body_info.rest_name {
                                if !self.import_tracker.needs_rest_props {
                                    self.import_tracker.needs_rest_props = true;
                                    self.import_tracker.record_synthetic_import("_restProps");
                                }
                                let excluded_keys: Vec<String> =
                                    body_info.prop_keys.iter().map(|(key, _)| key.clone()).collect();
                                let rest_stmt = props_destructuring::build_rest_props_declaration(
                                    rest_name,
                                    param_name,
                                    &excluded_keys,
                                    ctx,
                                );
                                arrow.body.statements.insert(body_info.stmt_index, rest_stmt);
                            }

                            // Build a prop_map for rewriting: (local_alias -> original_key)
                            // In non-JSX contexts, replace alias with props["key"] (computed member)
                            // The JSX contexts are handled by detect_signal_wrap via props_param_name
                            let prop_map: Vec<(String, String)> = body_info
                                .prop_keys
                                .iter()
                                .map(|(key, local)| (local.clone(), key.clone()))
                                .collect();

                            if !prop_map.is_empty() {
                                // Rewrite local alias references in non-JSX body statements.
                                // For body destructuring, replace `bindValue` with `props["bind:value"]`
                                // in non-JSX positions (like useSignal(bindValue) -> useSignal(props["bind:value"]))
                                // JSX positions are handled by _wrapProp via detect_signal_wrap.
                                rewrite_body_destr_references(
                                    &mut arrow.body.statements,
                                    &prop_map,
                                    param_name,
                                    ctx,
                                );
                            }

                        }
                    }
                }
                }

                if let Some(seg) = self
                    .segments
                    .iter_mut()
                    .find(|s| s.span.0 == call.span.start && s.span.1 == call.span.end)
                {
                    seg.param_names = vec![info.raw_props_name.clone()];
                }

                // Reclassify child segment captures: replace prop alias captures
                // with _rawProps. Only for component$ -- useResource$ and other hooks
                // don't have user props that child segments would capture.
                if is_component_exit {
                    let local_aliases: HashSet<String> = info
                        .prop_keys
                        .iter()
                        .map(|(_, local)| local.clone())
                        .collect();

                    let component_span = (call.span.start, call.span.end);
                    // Use segment name (with hash) for parent matching since
                    // seg.parent now stores segment_name, not display_name.
                    let component_segment_name = self
                        .segments
                        .iter()
                        .find(|s| s.span == component_span)
                        .map(|s| s.name.clone());

                    if let Some(parent_name) = component_segment_name {
                        // Collect child segment span starts that need body code updates
                        let mut segments_needing_body_update: Vec<u32> = Vec::new();
                        // Collect (segment_name, reclassified_capture_names) for QRL fixup
                        let mut reclassified_segments: Vec<(String, Vec<String>)> = Vec::new();

                        for seg in self.segments.iter_mut() {
                            if seg.parent.as_ref() != Some(&parent_name) || seg.capture_names.is_empty()
                            {
                                continue;
                            }
                            let mut needs_rawprops = false;
                            seg.capture_names.retain(|name| {
                                if local_aliases.contains(name) {
                                    needs_rawprops = true;
                                    false
                                } else {
                                    true
                                }
                            });
                            if needs_rawprops && !seg.capture_names.contains(&info.raw_props_name) {
                                seg.capture_names.insert(0, info.raw_props_name.clone());
                                // Mark this segment's body code for prop alias replacement
                                segments_needing_body_update.push(seg.span.0);
                            }
                            seg.capture_names.sort();
                            seg.captures = !seg.capture_names.is_empty();
                            // Record reclassified captures for QRL fixup
                            reclassified_segments.push((seg.name.clone(), seg.capture_names.clone()));
                        }

                        // Post-process segment body codes: replace prop alias references
                        // with _rawProps.propName to match SWC's output. The body code was
                        // serialized before props destructuring rewrite, so it still has
                        // original prop names (e.g., `{foo}` instead of `{_rawProps.foo}`).
                        for (span_start, body_code) in self.segment_body_codes.iter_mut() {
                            if segments_needing_body_update.contains(span_start) {
                                for (original_key, local_alias) in &info.prop_keys {
                                    let replacement = format!("{}.{}", info.raw_props_name, original_key);
                                    *body_code = replace_identifier_in_body(body_code, local_alias, &replacement);
                                }
                            }
                        }

                        // Fix QRL capture arrays in the component body AST.
                        // After props rewriting, the QRL calls have member expressions
                        // like [_rawProps.foo] instead of [_rawProps]. Rebuild the captures
                        // array to match the reclassified capture_names.
                        if !reclassified_segments.is_empty() {
                            if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first_mut() {
                                fix_qrl_captures_in_body(
                                    &mut arrow.body.statements,
                                    &reclassified_segments,
                                    ctx,
                                );
                            }
                        }
                    }
                }
            }

            // For component$ exits, capture the invalid_decl names (function/class declarations)
            // from this scope BEFORE popping. These will be used by segment body DCE to
            // force-remove these declarations from the component's body code (matching SWC).
            if is_component_exit {
                if let Some(frame) = self.invalid_decl_stack.last() {
                    if !frame.is_empty() {
                        self.component_invalid_decls
                            .insert(call.span.start, frame.clone());
                    }
                }
            }

            let (body_ident_refs, body_local_decls) = self.capture_stack.pop().unwrap_or_default();
            self.invalid_decl_stack.pop();

            // enclosing function scope.
            let is_top_level_dollar_call = self.capture_stack.is_empty();

            let capture_result =
                collector::compute_captures(&body_ident_refs, &body_local_decls, &self.collected);

            // Emit C02 diagnostics for function/class references captured inside $() scope.
            // SWC partitions declarations into (decl_collect, invalid_decl) where invalid_decl
            // contains function/class declarations. These are NOT captured but emit errors.
            // We check parent scope's invalid_decl_stack to find references to fn/class decls.
            let parent_invalid_decls: HashSet<String> = self.invalid_decl_stack
                .iter()
                .flat_map(|s| s.iter().cloned())
                .collect();
            let mut filtered_captures = Vec::new();
            for name in &capture_result.capture_names {
                if parent_invalid_decls.contains(name) {
                    // Emit C02 diagnostic matching SWC's "FunctionReference" error
                    self.diagnostics.push(crate::types::Diagnostic {
                        scope: "optimizer".to_string(),
                        category: crate::types::DiagnosticCategory::Error,
                        code: Some("C02".to_string()),
                        file: self.filename.clone(),
                        message: format!(
                            "Reference to identifier '{}' can not be used inside a Qrl($) scope because it's a function",
                            name
                        ),
                        highlights: None,
                        suggestions: None,
                    });
                } else {
                    filtered_captures.push(name.clone());
                }
            }
            // For non-top-level $() calls, filter captures to only include identifiers
            // that are actually declared in an enclosing scope. This matches SWC's
            // scope-aware capture analysis which only captures variables from enclosing
            // scopes, not unresolved/global identifiers (e.g., `children` used in JSX
            // without being declared anywhere). This is the same filtering applied to
            // JSX event handler captures (lines ~1267-1291).
            let filtered_captures = if !is_top_level_dollar_call && !self.capture_stack.is_empty() {
                let all_parent_decls: HashSet<String> = self
                    .capture_stack
                    .iter()
                    .flat_map(|(_, decls)| decls.iter().cloned())
                    .collect();
                filtered_captures
                    .into_iter()
                    .filter(|name| {
                        all_parent_decls.contains(name)
                            || self.collected.module_level_decls.contains(name)
                    })
                    .collect()
            } else {
                filtered_captures
            };

            let capture_result = collector::CaptureAnalysisResult {
                capture_names: filtered_captures,
                reemitted_imports: capture_result.reemitted_imports,
                diagnostics: capture_result.diagnostics,
            };

            // Reclassify module-level declarations from captures to needed_imports
            // (self-imports from the parent module). This matches SWC behavior where
            // module-level declarations are re-imported rather than captured.
            // Pass is_stripped to avoid adding _auto_ exports for stripped segments.
            let seg_is_stripped = self.stripped_segments.contains(&call.span.start);
            let (capture_result, module_decl_imports) =
                self.reclassify_module_level_decl_captures(capture_result, seg_is_stripped);

            // Filter out const-literal captures and their re-imports.
            // SWC inlines const literals (e.g., `const STEP_2 = 2`) into segment bodies
            // instead of capturing them. Also filter reemitted_imports when a local
            // const literal shadows an import. Skip module-level consts (handled as re-imports).
            let capture_result = if !self.const_literal_bindings.is_empty() {
                let filtered: Vec<String> = capture_result
                    .capture_names
                    .into_iter()
                    .filter(|name| {
                        !self.const_literal_bindings.contains_key(name)
                            || self.collected.module_level_decls.contains(name)
                    })
                    .collect();
                let filtered_imports: Vec<_> = capture_result
                    .reemitted_imports
                    .into_iter()
                    .filter(|ri| {
                        !self.const_literal_bindings.contains_key(&ri.local_name)
                    })
                    .collect();
                collector::CaptureAnalysisResult {
                    capture_names: filtered,
                    reemitted_imports: filtered_imports,
                    diagnostics: capture_result.diagnostics,
                }
            } else {
                capture_result
            };

            // Convert reemitted_imports into ImportInfo entries for the segment's needed_imports.
            // These imports will be emitted in the segment module by code_move.rs.
            let mut needed_imports: Vec<crate::types::ImportInfo> = capture_result
                .reemitted_imports
                .iter()
                .map(|ri| {
                    let mut aliases = std::collections::HashMap::new();
                    if let Some(ref imported) = ri.imported_name {
                        aliases.insert(ri.local_name.clone(), imported.clone());
                    }
                    crate::types::ImportInfo {
                        source: ri.source.clone(),
                        specifiers: vec![ri.local_name.clone()],
                        specifier_kinds: vec![ri.kind.clone()],
                        specifier_aliases: aliases,
                        is_qwik_core: false,
                        span: (0, 0),
                        assertion: ri.assertion.clone(),
                    }
                })
                .collect();
            needed_imports.extend(module_decl_imports);

            // C03 diagnostics: detect when $() first argument is NOT a function
            // expression but captures local identifiers. SWC emits C03 "Qrl($) scope
            // is not a function, but it's capturing local identifiers: <names>"
            // and clears captures instead of serializing them.
            let first_arg_is_function = call
                .arguments
                .first()
                .is_some_and(|arg| {
                    matches!(
                        arg,
                        Argument::ArrowFunctionExpression(_)
                            | Argument::FunctionExpression(_)
                    )
                });
            let capture_result = if !first_arg_is_function
                && !capture_result.capture_names.is_empty()
                && !is_top_level_dollar_call
            {
                // Emit C03 diagnostic for each captured variable.
                // The highlight span is the first argument (the non-function expression).
                let names_str = capture_result.capture_names.join(", ");
                let highlights = call.arguments.first().map(|arg| {
                    use oxc::span::GetSpan;
                    let arg_span = arg.span();
                    vec![compute_highlight_from_span(
                        &self.source_code,
                        arg_span.start,
                        arg_span.end,
                    )]
                });
                self.diagnostics.push(crate::types::Diagnostic {
                    scope: "optimizer".to_string(),
                    category: crate::types::DiagnosticCategory::Error,
                    code: Some("C03".to_string()),
                    file: self.filename.clone(),
                    message: format!(
                        "Qrl($) scope is not a function, but it's capturing local identifiers: {}",
                        names_str
                    ),
                    highlights,
                    suggestions: None,
                });
                // Clear captures -- non-function expressions don't get captures
                collector::CaptureAnalysisResult {
                    capture_names: vec![],
                    reemitted_imports: capture_result.reemitted_imports,
                    diagnostics: capture_result.diagnostics,
                }
            } else {
                capture_result
            };

            if let Some(seg) = self
                .segments
                .iter_mut()
                .find(|s| s.span.0 == call.span.start && s.span.1 == call.span.end)
            {
                if is_top_level_dollar_call {
                    // Top-level $()-calls never have captures
                    seg.captures = false;
                    seg.capture_names = vec![];
                } else {
                    seg.captures = !capture_result.capture_names.is_empty();
                    seg.capture_names = capture_result.capture_names.clone();
                }
                // Store needed imports for the segment module (applies to all segments,
                // both top-level and nested)
                seg.needed_imports = needed_imports;
            }

            let is_inline = entry_strategy::should_inline(&self.options.entry_strategy)
                || matches!(
                    self.options.entry_strategy,
                    crate::types::EntryStrategy::Hoist
                );
            if !is_top_level_dollar_call && !capture_result.capture_names.is_empty() && is_inline {
                if !self.import_tracker.needs_captures {
                    self.import_tracker.needs_captures = true;
                    self.import_tracker.record_synthetic_import("_captures");
                }
            }

            // Deferred recording of inlinedQrl/qrl import (from enter_call_expression).
            // SWC records qrl/inlinedQrl AFTER folding the body, so JSX imports
            // (_jsxSorted, _wrapProp, etc.) appear before it in encounter order.
            // The flags (needs_inlined_qrl/needs_qrl) were set in enter; we only
            // record the encounter order here in exit for correct positioning.
            let is_stripped_segment = self.stripped_segments.contains(&call.span.start);
            if !is_stripped_segment {
                if is_inline {
                    let name = if self.is_dev_mode() { "inlinedQrlDEV" } else { "inlinedQrl" };
                    self.import_tracker.record_synthetic_import(name);
                } else {
                    let name = if self.is_dev_mode() { "qrlDEV" } else { "qrl" };
                    self.import_tracker.record_synthetic_import(name);
                }
            }

            let segment_info = self
                .segments
                .iter()
                .find(|s| s.span.0 == call.span.start && s.span.1 == call.span.end)
                .cloned();

            let segment_info = match segment_info {
                Some(s) => s,
                None => return,
            };

            let is_stripped = self.stripped_segments.contains(&call.span.start);

            if !is_stripped {
                if !is_inline && !call.arguments.is_empty() {
                    let placeholder =
                        Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                    let body_arg = std::mem::replace(&mut call.arguments[0], placeholder);

                    let body_expr = match body_arg {
                        Argument::SpreadElement(_) => None,
                        _ => Some(argument_to_expression(body_arg, ctx)),
                    };

                    if let Some(expr_val) = body_expr {
                        let mut body_code = codegen_expression_with_comments(
                            expr_val,
                            &self.source_code,
                            &self.source_comments,
                            ctx,
                        );
                        // Inline const-literal values in CHILD segment body codes.
                        // SWC replaces references to const-literal bindings with
                        // their values (e.g., STEP_2 -> 2). Skip module-level
                        // consts (they're re-imported, not inlined).
                        // Only for child segments (not top-level): the declaring
                        // segment keeps the const declaration; child segments inline.
                        if !is_top_level_dollar_call {
                            for (name, value) in &self.const_literal_bindings {
                                if !self.collected.module_level_decls.contains(name) {
                                    body_code =
                                        replace_identifier_in_body(&body_code, name, value);
                                }
                            }
                        }
                        self.segment_body_codes.push((call.span.start, body_code));
                    }
                }
            }

            let replacement = if is_stripped {
                if !self.import_tracker.needs_noop_qrl {
                    self.import_tracker.needs_noop_qrl = true;
                    let name = if self.is_dev_mode() { "_noopQrlDEV" } else { "_noopQrl" };
                    self.import_tracker.record_synthetic_import(name);
                }
                let noop_meta = self.make_noop_dev_meta(&segment_info.display_name);
                import_rewrite::build_noop_qrl_call(
                    &segment_info.name,
                    &capture_result.capture_names,
                    noop_meta.as_ref(),
                    ctx,
                )
            } else if is_inline {
                let body_expr = if !call.arguments.is_empty() {
                    let arg = &mut call.arguments[0];
                    std::mem::replace(
                        arg,
                        Argument::from(ctx.ast.expression_identifier(SPAN, "undefined")),
                    )
                } else {
                    Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"))
                };

                let body_as_expr = match body_expr {
                    Argument::SpreadElement(_) => ctx.ast.expression_identifier(SPAN, "undefined"),
                    _ => argument_to_expression(body_expr, ctx),
                };

                let dev_meta = self.make_qrl_dev_meta(
                    segment_info.body_span.0,
                    segment_info.body_span.1,
                    &segment_info.display_name,
                );
                import_rewrite::build_inlined_qrl_call(
                    body_as_expr,
                    &segment_info.name,
                    &segment_info.capture_names,
                    dev_meta.as_ref(),
                    ctx,
                )
            } else {
                let import_ident = format!("i_{}", segment_info.hash);
                let dev_meta = self.make_qrl_dev_meta(
                    segment_info.body_span.0,
                    segment_info.body_span.1,
                    &segment_info.display_name,
                );
                import_rewrite::build_qrl_call(
                    &import_ident,
                    &segment_info.name,
                    &segment_info.capture_names,
                    dev_meta.as_ref(),
                    ctx,
                )
            };

            // For named $-suffixed calls, wrap with the Qrl-suffixed version
            let final_expr = match &kind {
                DollarCallKind::RawDollar => replacement,
                DollarCallKind::Named(name) => {
                    let qrl_name = words::dollar_to_qrl_name(name);
                    let qrl_atom = ctx.ast.atom(&qrl_name);
                    let qrl_callee = ctx.ast.expression_identifier(SPAN, qrl_atom);
                    let mut args = ctx.ast.vec_with_capacity(call.arguments.len().max(1));
                    args.push(Argument::from(replacement));
                    // Pass through additional arguments (e.g., { tagName: "my-foo" } for component$)
                    for i in 1..call.arguments.len() {
                        let placeholder = Argument::from(
                            ctx.ast.expression_identifier(SPAN, "undefined"),
                        );
                        let extra_arg = std::mem::replace(&mut call.arguments[i], placeholder);
                        args.push(extra_arg);
                    }
                    // Only component$ is tree-shakeable and gets PURE annotation.
                    // Side-effectful wrappers (useStylesQrl, useTaskQrl, etc.) must NOT
                    // have PURE because bundlers would incorrectly remove them.
                    if is_tree_shakeable_dollar_call(name) {
                        ctx.ast.expression_call_with_pure(
                            SPAN,
                            qrl_callee,
                            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                            args,
                            false,
                            true,
                        )
                    } else {
                        ctx.ast.expression_call(
                            SPAN,
                            qrl_callee,
                            None::<oxc::allocator::Box<'a, TSTypeParameterInstantiation<'a>>>,
                            args,
                            false,
                        )
                    }
                }
            };

            *expr = final_expr;
        }
    }

    fn exit_program(&mut self, program: &mut Program<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        let core_module = &self.options.core_module;

        // ---------------------------------------------------------------
        // Phase 1: Swap out old body and separate into categories
        // ---------------------------------------------------------------
        let mut old_body = ctx.ast.vec();
        std::mem::swap(&mut program.body, &mut old_body);

        // Separate old_body into: Qwik-core imports (to strip), non-Qwik imports,
        // and non-import stmts
        let mut non_qwik_imports: std::vec::Vec<Statement<'a>> = std::vec::Vec::new();
        let mut non_import_stmts: std::vec::Vec<Statement<'a>> = std::vec::Vec::new();

        for stmt in old_body {
            if let Statement::ImportDeclaration(ref import_decl) = stmt {
                let source = import_decl.source.value.as_str();
                let is_qwik_core = self
                    .collected
                    .module_imports
                    .iter()
                    .any(|i| i.source == source && i.is_qwik_core);
                if is_qwik_core {
                    continue; // Skip -- Qwik core imports are re-emitted as synthetic imports
                }
                non_qwik_imports.push(stmt);
            } else {
                non_import_stmts.push(stmt);
            }
        }

        // ---------------------------------------------------------------
        // Phase 1b: Simplify unused pure-annotated variable declarations
        // ---------------------------------------------------------------
        // When MinifyMode::Simplify, convert non-exported variable declarations
        // whose init is a PURE-annotated call expression AND whose binding is
        // unreferenced in the module body into expression statements.
        // This matches SWC's tree-shaker/DCE behavior:
        //   `const App = /* @__PURE__ */ componentQrl(...)` -> `componentQrl(...);`
        //   `const Header = /* @__PURE__ */ qrl(...)` -> `qrl(...);`
        // But non-pure calls are kept:
        //   `const renderHeader = component(qrl(...))` -> preserved as-is
        if matches!(self.options.minify, crate::types::MinifyMode::Simplify) {
            simplify_unused_pure_var_decls(&mut non_import_stmts, ctx);
        }

        // ---------------------------------------------------------------
        // Phase 1c: Hoist strategy -- extract inlinedQrl callbacks to named consts
        // ---------------------------------------------------------------
        // For EntryStrategy::Hoist, SWC extracts the first argument of each
        // inlinedQrl() call into a preceding named const declaration:
        //   const Name_hash = (props) => { ... };
        //   export const Name = componentQrl(inlinedQrl(Name_hash, "Name_hash"));
        // OXC currently keeps the callback inline. This step extracts it.
        if matches!(
            self.options.entry_strategy,
            crate::types::EntryStrategy::Hoist
        ) {
            extract_hoist_consts(&mut non_import_stmts, ctx);
        }

        // ---------------------------------------------------------------
        // Phase 2: Collect referenced identifiers from the entry module body
        // ---------------------------------------------------------------
        // Scan non-import statements to find which identifiers are actually
        // referenced in the entry module. This is used to filter synthetic
        // framework imports and non-Qwik user imports that are only needed
        // by segment bodies (which become separate files).
        let referenced_idents = collect_referenced_idents(&non_import_stmts);

        // ---------------------------------------------------------------
        // Phase 3: Build synthetic framework import statements in encounter order
        // ---------------------------------------------------------------
        // SWC uses BTreeMap<Id> where Id = (JsWord, SyntaxContext). SyntaxContext
        // values increase monotonically in encounter order during compilation,
        // so the effective output order follows traversal encounter order, NOT
        // alphabetical. We match this by using synthetic_import_order which
        // records the first-encounter order of each import during traversal.
        //
        // _Fragment is EXCLUDED from this list -- it gets emitted separately
        // AFTER hoisted _hf* stmts and lazy imports (matching SWC's extra_top_items).

        let mut emitted_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut synthetic_imports: std::vec::Vec<(&str, Statement<'a>)> = std::vec::Vec::new();

        // Build a lookup of locally-defined names to skip
        let module_level_decls = &self.collected.module_level_decls;

        for import_name in &self.import_tracker.synthetic_import_order {
            let name = import_name.as_str();

            // Skip _Fragment -- emitted separately after hoisted stmts
            if name == "_Fragment" {
                continue;
            }

            // Skip locally-defined Qrl functions
            if module_level_decls.contains(name) {
                continue;
            }

            // Skip duplicates (pending_segment_qrl_imports may duplicate qrl_imports)
            if !emitted_names.insert(name.to_string()) {
                continue;
            }

            // Build the import statement based on the name
            let (local_name, stmt): (&str, Statement<'a>) = match name {
                "_jsx" => {
                    if let Some(ref source) = self.import_tracker.custom_jsx_source {
                        let jsx_runtime_source = format!("{}/jsx-runtime", source);
                        let s = import_rewrite::build_aliased_import(
                            "jsx", "_jsx", &jsx_runtime_source, ctx,
                        );
                        ("_jsx", s)
                    } else {
                        continue; // custom_jsx_source not set, skip
                    }
                }
                "_jsxSorted" => {
                    let s = import_rewrite::build_named_import("_jsxSorted", core_module, ctx);
                    ("_jsxSorted", s)
                }
                "_jsxSplit" => {
                    let s = import_rewrite::build_named_import("_jsxSplit", core_module, ctx);
                    ("_jsxSplit", s)
                }
                "createElement" => {
                    let s = import_rewrite::build_aliased_import("createElement", "_createElement", core_module, ctx);
                    ("_createElement", s)
                }
                "_getVarProps" => {
                    let s = import_rewrite::build_named_import("_getVarProps", core_module, ctx);
                    ("_getVarProps", s)
                }
                "_getConstProps" => {
                    let s = import_rewrite::build_named_import("_getConstProps", core_module, ctx);
                    ("_getConstProps", s)
                }
                "_wrapProp" => {
                    let s = import_rewrite::build_named_import("_wrapProp", core_module, ctx);
                    ("_wrapProp", s)
                }
                "_fnSignal" => {
                    let s = import_rewrite::build_named_import("_fnSignal", core_module, ctx);
                    ("_fnSignal", s)
                }
                "_val" => {
                    let s = import_rewrite::build_named_import("_val", core_module, ctx);
                    ("_val", s)
                }
                "_chk" => {
                    let s = import_rewrite::build_named_import("_chk", core_module, ctx);
                    ("_chk", s)
                }
                "_captures" => {
                    let s = import_rewrite::build_named_import("_captures", core_module, ctx);
                    ("_captures", s)
                }
                "_restProps" => {
                    let s = import_rewrite::build_named_import("_restProps", core_module, ctx);
                    ("_restProps", s)
                }
                "_qrlSync" => {
                    let s = import_rewrite::build_named_import("_qrlSync", core_module, ctx);
                    ("_qrlSync", s)
                }
                "qrl" | "qrlDEV" | "inlinedQrl" | "inlinedQrlDEV"
                | "_noopQrl" | "_noopQrlDEV" | "_regSymbol" => {
                    let s = import_rewrite::build_named_import(name, core_module, ctx);
                    (name, s)
                }
                _ => {
                    // Qrl-suffixed import (componentQrl, useStylesQrl, etc.)
                    // or segment-level qrl import
                    let s = import_rewrite::build_named_import(name, core_module, ctx);
                    (name, s)
                }
            };

            synthetic_imports.push((local_name, stmt));
        }

        // 3b: Build lazy import declarations (const i_XXX = () => import(...))
        // Sort lazy imports by hash to match SWC's BTreeMap<Id> ordering.
        // Filter to only include lazy imports whose identifier is actually referenced.
        self.import_tracker.lazy_imports.sort_by(|a, b| a.0.cmp(&b.0));
        let mut lazy_imports: std::vec::Vec<Statement<'a>> = std::vec::Vec::new();
        for (hash, import_path) in &self.import_tracker.lazy_imports {
            let ident_name = format!("i_{}", hash);
            if !referenced_idents.contains(ident_name.as_str()) {
                continue; // Only used in segment bodies -- don't emit in entry module
            }
            let stmt = import_rewrite::build_lazy_import_declaration(hash, import_path, ctx);
            lazy_imports.push(stmt);
        }

        // ---------------------------------------------------------------
        // Phase 4: Filter synthetic imports (encounter order preserved)
        // ---------------------------------------------------------------
        // For segment strategy, many framework imports (e.g., _jsxSorted,
        // _wrapProp, _fnSignal) are only used inside segment bodies that
        // become separate files. Remove them from the entry module.
        let filtered_synthetic: std::vec::Vec<Statement<'a>> = synthetic_imports
            .into_iter()
            .filter(|(name, _stmt)| referenced_idents.contains(*name))
            .map(|(_name, stmt)| stmt)
            .collect();

        // Store count for lib.rs hoisted stmts injection positioning
        self.synthetic_import_count = filtered_synthetic.len();

        // ---------------------------------------------------------------
        // Phase 5: Collect and filter non-dollar Qwik core specifiers
        // ---------------------------------------------------------------
        // These are specifiers like `useStore`, `mutable` that were imported
        // alongside $-suffixed ones from @qwik.dev/core.
        // Only emit those that are actually referenced in entry module code.
        // Merge specifiers from the same source into single import statements.
        const BUILD_CONSTANTS: &[&str] = &["isServer", "isBrowser", "isDev"];

        // Group kept specifiers by source: { source => [(imported, local)] }
        let mut grouped_specifiers:
            std::collections::BTreeMap<String, std::vec::Vec<(String, String)>> =
            std::collections::BTreeMap::new();

        for import_info in &self.collected.module_imports {
            if import_info.is_qwik_core {
                for spec_name in &import_info.specifiers {
                    if self.collected.dollar_imports.contains(spec_name) {
                        continue; // Dollar import: stripped
                    }
                    let imported_name = import_info
                        .specifier_aliases
                        .get(spec_name)
                        .map(|s| s.as_str())
                        .unwrap_or(spec_name.as_str());
                    if BUILD_CONSTANTS.contains(&imported_name) {
                        continue; // Build constant: handled by const_replace
                    }

                    // Only emit if referenced in entry module body
                    if !referenced_idents.contains(spec_name.as_str()) {
                        continue; // Only used in segments -- don't emit in entry module
                    }

                    // Group by source for merging
                    grouped_specifiers
                        .entry(import_info.source.clone())
                        .or_default()
                        .push((imported_name.to_string(), spec_name.clone()));
                }
            }
        }

        // Build merged import statements from grouped specifiers.
        // Specifiers are in original source order (order of import_info.specifiers)
        // which matches SWC's preserved order.
        let mut non_dollar_imports: std::vec::Vec<Statement<'a>> = std::vec::Vec::new();
        for (source, specifiers) in grouped_specifiers {
            let stmt =
                import_rewrite::build_multi_specifier_import(&specifiers, &source, ctx);
            non_dollar_imports.push(stmt);
        }

        // ---------------------------------------------------------------
        // Phase 6: Filter non-Qwik user imports from old_body
        // ---------------------------------------------------------------
        // User imports like `import { mongodb } from "mondodb"` should only
        // appear in the entry module if they're actually referenced in the
        // entry module's non-import code.
        let filtered_non_qwik_imports: std::vec::Vec<Statement<'a>> = non_qwik_imports
            .into_iter()
            .filter(|stmt| {
                if let Statement::ImportDeclaration(import_decl) = stmt {
                    // Check if ANY specifier from this import is referenced
                    if let Some(specifiers) = &import_decl.specifiers {
                        for spec in specifiers {
                            let local_name = match spec {
                                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                    s.local.name.as_str()
                                }
                                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                    s.local.name.as_str()
                                }
                                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                    s.local.name.as_str()
                                }
                            };
                            if referenced_idents.contains(local_name) {
                                return true;
                            }
                        }
                        return false; // No specifiers referenced
                    }
                    // Side-effect import (no specifiers): always keep
                    true
                } else {
                    true // Not an import -- keep
                }
            })
            .collect();

        // ---------------------------------------------------------------
        // Phase 7: Assemble body in SWC order
        // ---------------------------------------------------------------
        // SWC order (matched from golden snapshots):
        // 1. Synthetic framework imports (encounter order, WITHOUT _Fragment)
        // 2. Lazy import declarations (const i_XXX = ...)
        //    [hoisted _hf* stmts injected here by lib.rs post-emission]
        // 3. _Fragment import (from jsx-runtime) -- AFTER hoisted stmts
        // 4. Non-dollar Qwik core specifiers (useStore, mutable, etc.) - merged
        // 5. Original non-Qwik imports (filtered)
        // 6. Non-import code (exports, declarations, etc.)
        // 7. _auto_ exports

        // Build _Fragment import if needed (separate from synthetic imports)
        let fragment_import: Option<Statement<'a>> = if self.import_tracker.needs_fragment {
            if referenced_idents.contains("_Fragment") {
                Some(import_rewrite::build_aliased_import(
                    "Fragment",
                    "_Fragment",
                    "@qwik.dev/core/jsx-runtime",
                    ctx,
                ))
            } else {
                None
            }
        } else {
            None
        };

        let total_capacity = filtered_synthetic.len()
            + lazy_imports.len()
            + if fragment_import.is_some() { 1 } else { 0 }
            + non_dollar_imports.len()
            + filtered_non_qwik_imports.len()
            + non_import_stmts.len();
        let mut new_body = ctx.ast.vec_with_capacity(total_capacity);

        // 1: Filtered synthetic framework imports (encounter order)
        for stmt in filtered_synthetic {
            new_body.push(stmt);
        }

        // 2: Lazy import declarations
        for stmt in lazy_imports {
            new_body.push(stmt);
        }

        // 3: _Fragment import (after lazy imports, before non-dollar imports)
        // In the emitted code, hoisted _hf* stmts will be injected by lib.rs
        // between synthetic imports (step 1) and this point, so _Fragment
        // naturally ends up after hoisted stmts.
        if let Some(stmt) = fragment_import {
            new_body.push(stmt);
        }

        // 4: Non-dollar Qwik core specifiers (merged by source)
        for stmt in non_dollar_imports {
            new_body.push(stmt);
        }

        // 5: Filtered non-Qwik user imports
        for stmt in filtered_non_qwik_imports {
            new_body.push(stmt);
        }

        // 6: Non-import code (exports, declarations, expressions)
        for stmt in non_import_stmts {
            new_body.push(stmt);
        }

        // 6: Emit _auto_ exports for module-level declarations that are
        // referenced by segments but NOT user-exported. Each gets:
        //   export { X as _auto_X }
        // Sorted alphabetically for deterministic output (matches SWC).
        if !self.auto_exports.is_empty() {
            let mut auto_export_names: Vec<&String> = self.auto_exports.iter().collect();
            auto_export_names.sort();

            for name in auto_export_names {
                let local = ctx.ast.module_export_name_identifier_reference(SPAN, ctx.ast.atom(name.as_str()));
                let exported_name = format!("_auto_{}", name);
                let exported = ctx.ast.module_export_name_identifier_name(SPAN, ctx.ast.atom(&exported_name));
                let specifier = ctx.ast.export_specifier(SPAN, local, exported, ImportOrExportKind::Value);
                let specifiers = ctx.ast.vec1(specifier);
                let export_decl = ctx.ast.module_declaration_export_named_declaration(
                    SPAN,
                    None, // no declaration
                    specifiers,
                    None, // no source
                    ImportOrExportKind::Value,
                    None::<oxc::allocator::Box<'a, WithClause<'a>>>,
                );
                new_body.push(Statement::from(export_decl));
            }
        }

        program.body = new_body;
    }
}

/// Check if a variable declarator's initializer is "return-static" per SWC semantics.
///
/// Port of SWC's `is_return_static` from `crates/swc-optimizer/core/src/transform.rs:3750`.
/// A `const` declaration is only treated as a const binding (for JSX prop classification)
/// when its initializer is:
///   - A call to a function whose name ends with `$` (component$, $, etc.)
///   - A call to a function whose name ends with `Qrl` (inlinedQrl, componentQrl, etc.)
///   - A call to a function whose name starts with `use` (useSignal, useStore, etc.)
///   - No initializer (None)
///
/// This means `const x = signal.value + "foo"` or `const x = a + b` are NOT static,
/// while `const sig = useSignal()` and `const cmp = component$(() => {})` ARE static.
fn is_init_return_static(init: &Option<Expression<'_>>) -> bool {
    match init {
        Some(Expression::CallExpression(call)) => {
            // Check the callee name
            if let Expression::Identifier(ident) = &call.callee {
                let name = ident.name.as_str();
                return name.ends_with('$')
                    || name.ends_with("Qrl")
                    || name.starts_with("use");
            }
            false
        }
        Some(_) => false,
        None => true,
    }
}

/// Collect all binding names from a binding pattern into a HashSet.
///
/// Used to populate `const_bindings` from `const` declarations.
/// Handles simple identifiers, object/array destructuring patterns,
/// and assignment patterns (defaults).
fn collect_const_binding_names(pattern: &BindingPattern<'_>, set: &mut HashSet<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            set.insert(ident.name.as_str().to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_const_binding_names(&prop.value, set);
            }
            if let Some(rest) = &obj.rest {
                collect_const_binding_names(&rest.argument, set);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_const_binding_names(elem, set);
            }
            if let Some(rest) = &arr.rest {
                collect_const_binding_names(&rest.argument, set);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_const_binding_names(&assign.left, set);
        }
    }
}

/// Flush pending QRL hoists to a function body.
///
/// Prepends `const seg_name = /* @__PURE__ */ qrl(import_ident, "seg_name", [captures]);`
/// declarations to the beginning of `statements`. This implements SWC's behavior of
/// hoisting QRL calls from inside loops to the enclosing function body.
fn flush_qrl_hoists_to_body<'a>(
    statements: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    hoists: &[(String, String, Vec<String>)],
    ctx: &mut TraverseCtx<'a, ()>,
) {
    // Find the insertion point: after variable declarations at the top of the body.
    // SWC places hoisted QRL consts after local variable declarations (useStore, useSignal, etc.)
    // but before function declarations, loops, and return statements.
    let mut insert_idx = 0;
    for (i, stmt) in statements.iter().enumerate() {
        match stmt {
            Statement::VariableDeclaration(_) => {
                insert_idx = i + 1;
            }
            _ => break,
        }
    }

    // Build const declarations and insert in forward order.
    // We increment insert_idx after each insertion so the hoists appear in
    // the same order they were pushed (which matches SWC's source-order hoisting).
    let mut idx = insert_idx;
    for (import_ident, seg_name, captures) in hoists.iter() {
        let qrl_call = import_rewrite::build_qrl_call(import_ident, seg_name, captures, None, ctx);

        let binding = ctx
            .ast
            .binding_pattern_binding_identifier(SPAN, ctx.ast.atom(seg_name.as_str()));
        let declarator = ctx.ast.variable_declarator(
            SPAN,
            VariableDeclarationKind::Const,
            binding,
            None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
            Some(qrl_call),
            false,
        );
        let declaration = ctx.ast.variable_declaration(
            SPAN,
            VariableDeclarationKind::Const,
            ctx.ast.vec1(declarator),
            false,
        );
        let stmt = Statement::from(Declaration::VariableDeclaration(ctx.ast.alloc(declaration)));
        statements.insert(idx, stmt);
        idx += 1;
    }
}

/// Normalize a symbol name by replacing non-alphanumeric chars with `_`,
/// squashing consecutive underscores, and trimming leading/trailing underscores.
///
/// Port of SWC's `escape_sym` from `crates/swc-optimizer/core/src/transform.rs:3320`.
fn escape_sym(str: &str) -> String {
    str.chars()
        .flat_map(|x| match x {
            'A'..='Z' | 'a'..='z' | '0'..='9' => Some(x),
            _ => Some('_'),
        })
        .fold((String::new(), None), |(mut acc, prev), x| {
            if x == '_' {
                if prev.is_none() {
                    (acc, None)
                } else {
                    (acc, Some('_'))
                }
            } else {
                if prev == Some('_') {
                    acc.push('_');
                }
                acc.push(x);
                (acc, Some(x))
            }
        })
        .0
}

/// Simplify unused variable declarations in the module body.
///
/// Simplified tree-shaker: drop unused pure-annotated variable declarations.
///
/// When a non-exported VariableDeclaration has a single declarator whose
/// init is a PURE-annotated CallExpression (`/* @__PURE__ */`), and the
/// declared name is not referenced elsewhere in the module body, convert
/// the VariableDeclaration to an ExpressionStatement (dropping the binding).
///
/// This matches SWC's tree-shaker/DCE behavior for `MinifyMode::Simplify`:
/// - `const App = /* @__PURE__ */ componentQrl(...)` -> `componentQrl(...);`
/// - `const Header = /* @__PURE__ */ qrl(...)` -> `qrl(...);`
///
/// Non-pure calls are NOT affected:
/// - `const renderHeader = component(qrl(...))` -> preserved
fn simplify_unused_pure_var_decls<'a>(
    stmts: &mut std::vec::Vec<Statement<'a>>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    // 1. Collect all referenced identifiers across all statements.
    let all_refs = collect_referenced_idents(stmts);

    // 2. For each bare VariableDeclaration (not wrapped in export),
    //    check if it qualifies for removal.
    let mut i = 0;
    while i < stmts.len() {
        let should_unwrap = if let Statement::VariableDeclaration(ref var_decl) = stmts[i] {
            if var_decl.declarations.len() == 1 {
                if let BindingPattern::BindingIdentifier(ref ident) = var_decl.declarations[0].id {
                    let name = ident.name.as_str();
                    // Init must be a PURE-annotated call expression, or a call
                    // to a known Qwik function that is implicitly side-effect-free
                    // (SWC strips these even without /* @__PURE__ */ annotation).
                    let has_pure_call_init = var_decl.declarations[0]
                        .init
                        .as_ref()
                        .is_some_and(|init| {
                            if let Expression::CallExpression(call) = init {
                                if call.pure {
                                    return true;
                                }
                                // Known Qwik functions are implicitly pure
                                if let Expression::Identifier(callee) = &call.callee {
                                    return matches!(
                                        callee.name.as_str(),
                                        "inlinedQrl" | "inlinedQrlDEV" | "qrl" | "qrlDEV"
                                            | "componentQrl" | "_noopQrl" | "_noopQrlDEV"
                                    );
                                }
                            }
                            false
                        });
                    // Name must not be referenced elsewhere in the module
                    let is_referenced = all_refs.contains(name);
                    has_pure_call_init && !is_referenced
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if should_unwrap {
            let stmt = std::mem::replace(
                &mut stmts[i],
                Statement::EmptyStatement(ctx.ast.alloc(oxc::ast::ast::EmptyStatement { span: SPAN })),
            );
            if let Statement::VariableDeclaration(mut var_decl) = stmt {
                if let Some(init) = var_decl.declarations[0].init.take() {
                    let expr_stmt = ctx.ast.alloc(ExpressionStatement {
                        span: SPAN,
                        expression: init,
                    });
                    stmts[i] = Statement::ExpressionStatement(expr_stmt);
                }
            }
        }
        i += 1;
    }
}

/// Extract inlinedQrl callback expressions to named const declarations for Hoist strategy.
///
/// For EntryStrategy::Hoist, SWC creates a named const for each segment's callback:
/// ```js
/// const Name_hash = (props) => { ... };
/// export const Name = componentQrl(inlinedQrl(Name_hash, "Name_hash"));
/// ```
///
/// This function walks `stmts`, finds `inlinedQrl(callback, "name", ...)` calls,
/// extracts `callback` to `const name = callback;`, replaces callback with an
/// identifier reference to `name`, and inserts the const before the containing statement.
///
/// Port of SWC's `fold_module` lines 2567-2592.
fn extract_hoist_consts<'a>(
    stmts: &mut std::vec::Vec<Statement<'a>>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    // Process statements in reverse order so insertions don't shift indices
    // of statements we haven't processed yet.
    // Collect (insert_index, const_declarations) pairs first, then insert.
    let mut insertions: std::vec::Vec<(usize, std::vec::Vec<Statement<'a>>)> =
        std::vec::Vec::new();

    for i in 0..stmts.len() {
        let mut extracted: std::vec::Vec<Statement<'a>> = std::vec::Vec::new();
        extract_inlined_qrl_from_stmt(&mut stmts[i], &mut extracted, ctx);
        if !extracted.is_empty() {
            insertions.push((i, extracted));
        }
    }

    // Insert in reverse order to maintain correct indices
    for (idx, consts) in insertions.into_iter().rev() {
        for (j, const_stmt) in consts.into_iter().enumerate() {
            stmts.insert(idx + j, const_stmt);
        }
    }
}

/// Recursively find `inlinedQrl(callback, "name", ...)` or `inlinedQrlDEV(callback, "name", ...)`
/// calls in a statement and extract callbacks to named const declarations.
fn extract_inlined_qrl_from_stmt<'a>(
    stmt: &mut Statement<'a>,
    extracted: &mut std::vec::Vec<Statement<'a>>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    // Walk into the statement to find inlinedQrl calls.
    // They can be nested: componentQrl(inlinedQrl(...))
    match stmt {
        Statement::ExportNamedDeclaration(export_decl) => {
            if let Some(ref mut decl) = export_decl.declaration {
                extract_inlined_qrl_from_decl(decl, extracted, ctx);
            }
        }
        Statement::VariableDeclaration(var_decl) => {
            for declarator in var_decl.declarations.iter_mut() {
                if let Some(ref mut init) = declarator.init {
                    extract_inlined_qrl_from_expr(init, extracted, ctx);
                }
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            extract_inlined_qrl_from_expr(&mut expr_stmt.expression, extracted, ctx);
        }
        _ => {}
    }
}

/// Extract from a Declaration node.
fn extract_inlined_qrl_from_decl<'a>(
    decl: &mut Declaration<'a>,
    extracted: &mut std::vec::Vec<Statement<'a>>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    if let Declaration::VariableDeclaration(var_decl) = decl {
        for declarator in var_decl.declarations.iter_mut() {
            if let Some(ref mut init) = declarator.init {
                extract_inlined_qrl_from_expr(init, extracted, ctx);
            }
        }
    }
}

/// Extract from an Expression, recursively descending into call arguments.
fn extract_inlined_qrl_from_expr<'a>(
    expr: &mut Expression<'a>,
    extracted: &mut std::vec::Vec<Statement<'a>>,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match expr {
        Expression::CallExpression(call) => {
            // Check if this is inlinedQrl(...) or inlinedQrlDEV(...)
            let is_inlined_qrl = match &call.callee {
                Expression::Identifier(ident) => {
                    ident.name == "inlinedQrl" || ident.name == "inlinedQrlDEV"
                }
                _ => false,
            };

            if is_inlined_qrl && !call.arguments.is_empty() {
                // Extract the segment name from the second argument (string literal)
                let seg_name = if call.arguments.len() >= 2 {
                    match &call.arguments[1] {
                        Argument::StringLiteral(lit) => Some(lit.value.to_string()),
                        _ => None,
                    }
                } else {
                    None
                };

                if let Some(name) = seg_name {
                    // Extract the first argument (callback expression)
                    let placeholder = Argument::from(
                        ctx.ast.expression_identifier(SPAN, ctx.ast.atom(name.as_str())),
                    );
                    let callback_arg = std::mem::replace(&mut call.arguments[0], placeholder);

                    // Convert Argument to Expression
                    let callback_expr = argument_to_expression(callback_arg, ctx);

                    // Build: const name = callback;
                    let binding = ctx.ast.binding_pattern_binding_identifier(
                        SPAN,
                        ctx.ast.atom(name.as_str()),
                    );
                    let declarator = ctx.ast.variable_declarator(
                        SPAN,
                        VariableDeclarationKind::Const,
                        binding,
                        None::<oxc::allocator::Box<'a, TSTypeAnnotation<'a>>>,
                        Some(callback_expr),
                        false,
                    );
                    let declaration = ctx.ast.variable_declaration(
                        SPAN,
                        VariableDeclarationKind::Const,
                        ctx.ast.vec1(declarator),
                        false,
                    );
                    let const_stmt = Statement::from(Declaration::VariableDeclaration(
                        ctx.ast.alloc(declaration),
                    ));
                    extracted.push(const_stmt);
                }
            } else {
                // Not inlinedQrl -- recurse into arguments
                for arg in call.arguments.iter_mut() {
                    match arg {
                        Argument::SpreadElement(_) => {}
                        _ => {
                            let arg_expr = arg.as_expression_mut();
                            if let Some(e) = arg_expr {
                                extract_inlined_qrl_from_expr(e, extracted, ctx);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Convert a JSX event attribute name to its HTML attribute equivalent.
///
/// Only applies when the attribute name ends with `$` and starts with `on`.
/// Returns `None` if the attribute doesn't match the pattern.
///
/// Port of SWC's `jsx_event_to_html_attribute` from
/// `crates/swc-optimizer/core/src/transform.rs:3347`.
///
/// Examples:
/// - `onClick$` -> `Some("q-e:click")`
/// - `onInput$` -> `Some("q-e:input")`
/// - `window:onClick$` is handled separately (prefix already stripped)
/// - `onClick` (no $) -> `None`
fn jsx_event_to_html_attribute(jsx_event: &str) -> Option<String> {
    if !jsx_event.ends_with('$') {
        return None;
    }

    let (prefix, idx) = get_event_scope_data_from_jsx_event(jsx_event);

    if idx == usize::MAX {
        return None;
    }

    let name = &jsx_event[idx..jsx_event.len() - 1];

    if name == "DOMContentLoaded" {
        return Some(format!("{}-d-o-m-content-loaded", prefix));
    }

    let processed_name = if let Some(stripped) = name.strip_prefix('-') {
        // marker for case sensitive event name
        stripped.to_string()
    } else {
        name.to_lowercase()
    };

    Some(create_event_name(&processed_name, prefix))
}

/// Get the event scope prefix and starting index from a JSX event name.
///
/// Port of SWC's `get_event_scope_data_from_jsx_event`.
fn get_event_scope_data_from_jsx_event(jsx_event: &str) -> (&str, usize) {
    if jsx_event.starts_with("window:on") {
        ("q-w:", 9)
    } else if jsx_event.starts_with("document:on") {
        ("q-d:", 11)
    } else if jsx_event.starts_with("on") {
        ("q-e:", 2)
    } else {
        ("", usize::MAX)
    }
}

/// Create an event name by converting from camelCase to kebab-case.
///
/// Port of SWC's `create_event_name`.
fn create_event_name(name: &str, prefix: &str) -> String {
    let mut result = String::from(prefix);

    for c in name.chars() {
        if c.is_ascii_uppercase() || c == '-' {
            result.push('-');
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }

    result
}

/// Collect all identifier names referenced in a list of statements.
///
/// Unlike `walk_statement_for_captures`, this function:
/// - DOES descend into nested function/arrow bodies (needed for inline strategy)
/// - Returns a HashSet of unique identifier names (not a Vec)
/// - Does NOT track local declarations (only collects references)
/// - Skips import declarations (we only want to see what non-import code references)
///
/// Used by `exit_program` to determine which synthetic framework imports and
/// non-Qwik user imports are actually needed in the entry module.
fn collect_referenced_idents(stmts: &[Statement<'_>]) -> HashSet<String> {
    let mut idents = HashSet::new();
    for stmt in stmts {
        if matches!(stmt, Statement::ImportDeclaration(_)) {
            continue; // Skip import declarations themselves
        }
        collect_idents_from_statement(stmt, &mut idents);
    }
    idents
}

/// Walk a statement collecting all identifier reference names (deep traversal).
fn collect_idents_from_statement(stmt: &Statement<'_>, idents: &mut HashSet<String>) {
    match stmt {
        Statement::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                if let Some(init) = &declarator.init {
                    collect_idents_from_expression(init, idents);
                }
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            collect_idents_from_expression(&expr_stmt.expression, idents);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                collect_idents_from_expression(arg, idents);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_idents_from_statement(s, idents);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_idents_from_expression(&if_stmt.test, idents);
            collect_idents_from_statement(&if_stmt.consequent, idents);
            if let Some(alt) = &if_stmt.alternate {
                collect_idents_from_statement(alt, idents);
            }
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                match init {
                    ForStatementInit::VariableDeclaration(var_decl) => {
                        for declarator in &var_decl.declarations {
                            if let Some(init_expr) = &declarator.init {
                                collect_idents_from_expression(init_expr, idents);
                            }
                        }
                    }
                    _ => {
                        if let Some(expr) = init.as_expression() {
                            collect_idents_from_expression(expr, idents);
                        }
                    }
                }
            }
            if let Some(test) = &for_stmt.test {
                collect_idents_from_expression(test, idents);
            }
            if let Some(update) = &for_stmt.update {
                collect_idents_from_expression(update, idents);
            }
            collect_idents_from_statement(&for_stmt.body, idents);
        }
        Statement::ForInStatement(for_in) => {
            collect_idents_from_expression(&for_in.right, idents);
            collect_idents_from_statement(&for_in.body, idents);
        }
        Statement::ForOfStatement(for_of) => {
            collect_idents_from_expression(&for_of.right, idents);
            collect_idents_from_statement(&for_of.body, idents);
        }
        Statement::WhileStatement(while_stmt) => {
            collect_idents_from_expression(&while_stmt.test, idents);
            collect_idents_from_statement(&while_stmt.body, idents);
        }
        Statement::DoWhileStatement(do_while) => {
            collect_idents_from_statement(&do_while.body, idents);
            collect_idents_from_expression(&do_while.test, idents);
        }
        Statement::FunctionDeclaration(func) => {
            // Descend into function body (unlike capture walker)
            for param in &func.params.items {
                collect_idents_from_binding_pattern(&param.pattern, idents);
            }
            if let Some(body) = &func.body {
                for s in &body.statements {
                    collect_idents_from_statement(s, idents);
                }
            }
        }
        Statement::SwitchStatement(switch) => {
            collect_idents_from_expression(&switch.discriminant, idents);
            for case in &switch.cases {
                if let Some(test) = &case.test {
                    collect_idents_from_expression(test, idents);
                }
                for s in &case.consequent {
                    collect_idents_from_statement(s, idents);
                }
            }
        }
        Statement::ThrowStatement(throw) => {
            collect_idents_from_expression(&throw.argument, idents);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_idents_from_statement(s, idents);
            }
            if let Some(handler) = &try_stmt.handler {
                for s in &handler.body.body {
                    collect_idents_from_statement(s, idents);
                }
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                for s in &finalizer.body {
                    collect_idents_from_statement(s, idents);
                }
            }
        }
        Statement::LabeledStatement(labeled) => {
            collect_idents_from_statement(&labeled.body, idents);
        }
        // ExportNamedDeclaration and ExportDefaultDeclaration
        Statement::ExportNamedDeclaration(export) => {
            // Collect references from export specifiers: `export { X, Y as Z }`
            // The `local` name of each specifier references a module-level binding.
            for spec in &export.specifiers {
                let local_name = spec.local.name().as_str();
                idents.insert(local_name.to_string());
            }
            if let Some(decl) = &export.declaration {
                match decl {
                    Declaration::VariableDeclaration(var_decl) => {
                        for declarator in &var_decl.declarations {
                            if let Some(init) = &declarator.init {
                                collect_idents_from_expression(init, idents);
                            }
                        }
                    }
                    Declaration::FunctionDeclaration(func) => {
                        for param in &func.params.items {
                            collect_idents_from_binding_pattern(&param.pattern, idents);
                        }
                        if let Some(body) = &func.body {
                            for s in &body.statements {
                                collect_idents_from_statement(s, idents);
                            }
                        }
                    }
                    Declaration::ClassDeclaration(class) => {
                        if let Some(super_class) = &class.super_class {
                            collect_idents_from_expression(super_class, idents);
                        }
                        for elem in &class.body.body {
                            collect_idents_from_class_element(elem, idents);
                        }
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDefaultDeclaration(export) => {
            match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                    for param in &func.params.items {
                        collect_idents_from_binding_pattern(&param.pattern, idents);
                    }
                    if let Some(body) = &func.body {
                        for s in &body.statements {
                            collect_idents_from_statement(s, idents);
                        }
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    if let Some(super_class) = &class.super_class {
                        collect_idents_from_expression(super_class, idents);
                    }
                    for elem in &class.body.body {
                        collect_idents_from_class_element(elem, idents);
                    }
                }
                _ => {
                    if let Some(expr) = export.declaration.as_expression() {
                        collect_idents_from_expression(expr, idents);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Walk an expression collecting all identifier reference names (deep traversal).
/// Unlike `walk_expression_for_captures`, this DOES descend into nested functions.
fn collect_idents_from_expression(expr: &Expression<'_>, idents: &mut HashSet<String>) {
    match expr {
        Expression::Identifier(ident) => {
            idents.insert(ident.name.as_str().to_string());
        }
        Expression::CallExpression(call) => {
            collect_idents_from_expression(&call.callee, idents);
            for arg in &call.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        collect_idents_from_expression(&spread.argument, idents);
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            collect_idents_from_expression(expr, idents);
                        }
                    }
                }
            }
        }
        Expression::StaticMemberExpression(member) => {
            collect_idents_from_expression(&member.object, idents);
        }
        Expression::ComputedMemberExpression(member) => {
            collect_idents_from_expression(&member.object, idents);
            collect_idents_from_expression(&member.expression, idents);
        }
        Expression::PrivateFieldExpression(member) => {
            collect_idents_from_expression(&member.object, idents);
        }
        Expression::BinaryExpression(binary) => {
            collect_idents_from_expression(&binary.left, idents);
            collect_idents_from_expression(&binary.right, idents);
        }
        Expression::LogicalExpression(logical) => {
            collect_idents_from_expression(&logical.left, idents);
            collect_idents_from_expression(&logical.right, idents);
        }
        Expression::AssignmentExpression(assign) => {
            collect_idents_from_assignment_target(&assign.left, idents);
            collect_idents_from_expression(&assign.right, idents);
        }
        Expression::UnaryExpression(unary) => {
            collect_idents_from_expression(&unary.argument, idents);
        }
        Expression::UpdateExpression(update) => {
            match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                    idents.insert(ident.name.as_str().to_string());
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    collect_idents_from_expression(&member.object, idents);
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    collect_idents_from_expression(&member.object, idents);
                    collect_idents_from_expression(&member.expression, idents);
                }
                SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                    collect_idents_from_expression(&member.object, idents);
                }
                _ => {}
            }
        }
        Expression::ConditionalExpression(cond) => {
            collect_idents_from_expression(&cond.test, idents);
            collect_idents_from_expression(&cond.consequent, idents);
            collect_idents_from_expression(&cond.alternate, idents);
        }
        Expression::TemplateLiteral(tmpl) => {
            for expr in &tmpl.expressions {
                collect_idents_from_expression(expr, idents);
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        collect_idents_from_expression(&spread.argument, idents);
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(expr) = elem.as_expression() {
                            collect_idents_from_expression(expr, idents);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        if p.computed {
                            if let Some(key_expr) = p.key.as_expression() {
                                collect_idents_from_expression(key_expr, idents);
                            }
                        }
                        collect_idents_from_expression(&p.value, idents);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_idents_from_expression(&spread.argument, idents);
                    }
                }
            }
        }
        // IMPORTANT: Unlike capture walker, we DO descend into nested functions
        // because in inline/hoist mode, the segment code is inside arrow functions.
        Expression::ArrowFunctionExpression(arrow) => {
            for param in &arrow.params.items {
                collect_idents_from_binding_pattern(&param.pattern, idents);
            }
            for s in &arrow.body.statements {
                collect_idents_from_statement(s, idents);
            }
        }
        Expression::FunctionExpression(func) => {
            for param in &func.params.items {
                collect_idents_from_binding_pattern(&param.pattern, idents);
            }
            if let Some(body) = &func.body {
                for s in &body.statements {
                    collect_idents_from_statement(s, idents);
                }
            }
        }
        Expression::ParenthesizedExpression(paren) => {
            collect_idents_from_expression(&paren.expression, idents);
        }
        Expression::SequenceExpression(seq) => {
            for expr in &seq.expressions {
                collect_idents_from_expression(expr, idents);
            }
        }
        Expression::AwaitExpression(await_expr) => {
            collect_idents_from_expression(&await_expr.argument, idents);
        }
        Expression::TaggedTemplateExpression(tagged) => {
            collect_idents_from_expression(&tagged.tag, idents);
            for expr in &tagged.quasi.expressions {
                collect_idents_from_expression(expr, idents);
            }
        }
        Expression::NewExpression(new_expr) => {
            collect_idents_from_expression(&new_expr.callee, idents);
            for arg in &new_expr.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        collect_idents_from_expression(&spread.argument, idents);
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            collect_idents_from_expression(expr, idents);
                        }
                    }
                }
            }
        }
        Expression::YieldExpression(yield_expr) => {
            if let Some(arg) = &yield_expr.argument {
                collect_idents_from_expression(arg, idents);
            }
        }
        Expression::ImportExpression(import_expr) => {
            collect_idents_from_expression(&import_expr.source, idents);
        }
        Expression::ClassExpression(class) => {
            if let Some(super_class) = &class.super_class {
                collect_idents_from_expression(super_class, idents);
            }
            for elem in &class.body.body {
                collect_idents_from_class_element(elem, idents);
            }
        }
        // JSX expressions reference identifiers in element names and attribute values
        Expression::JSXElement(jsx) => {
            collect_idents_from_jsx_element(jsx, idents);
        }
        Expression::JSXFragment(frag) => {
            for child in &frag.children {
                collect_idents_from_jsx_child(child, idents);
            }
        }
        _ => {}
    }
}

/// Walk a JSX element collecting all identifier references.
fn collect_idents_from_jsx_element(elem: &JSXElement<'_>, idents: &mut HashSet<String>) {
    // Element name: <Component ...> references "Component"
    match &elem.opening_element.name {
        JSXElementName::Identifier(id) => {
            // Lowercase = HTML element (div, span), uppercase = component reference
            let name = id.name.as_str();
            if name.starts_with(|c: char| c.is_uppercase()) {
                idents.insert(name.to_string());
            }
        }
        JSXElementName::IdentifierReference(id) => {
            idents.insert(id.name.as_str().to_string());
        }
        JSXElementName::MemberExpression(member) => {
            collect_idents_from_jsx_member_expr(member, idents);
        }
        JSXElementName::NamespacedName(_) => {}
        JSXElementName::ThisExpression(_) => {}
    }

    // Attributes
    for attr in &elem.opening_element.attributes {
        match attr {
            JSXAttributeItem::Attribute(a) => {
                if let Some(value) = &a.value {
                    match value {
                        JSXAttributeValue::ExpressionContainer(expr) => {
                            if let Some(inner) = expr.expression.as_expression() {
                                collect_idents_from_expression(inner, idents);
                            }
                        }
                        _ => {}
                    }
                }
            }
            JSXAttributeItem::SpreadAttribute(spread) => {
                collect_idents_from_expression(&spread.argument, idents);
            }
        }
    }

    // Children
    for child in &elem.children {
        collect_idents_from_jsx_child(child, idents);
    }
}

/// Walk a JSX child collecting identifier references.
fn collect_idents_from_jsx_child(child: &JSXChild<'_>, idents: &mut HashSet<String>) {
    match child {
        JSXChild::Element(elem) => {
            collect_idents_from_jsx_element(elem, idents);
        }
        JSXChild::Fragment(frag) => {
            for c in &frag.children {
                collect_idents_from_jsx_child(c, idents);
            }
        }
        JSXChild::ExpressionContainer(expr) => {
            if let Some(inner) = expr.expression.as_expression() {
                collect_idents_from_expression(inner, idents);
            }
        }
        JSXChild::Spread(spread) => {
            collect_idents_from_expression(&spread.expression, idents);
        }
        JSXChild::Text(_) => {}
    }
}

/// Walk a JSX member expression to collect the root identifier.
fn collect_idents_from_jsx_member_expr(
    member: &JSXMemberExpression<'_>,
    idents: &mut HashSet<String>,
) {
    match &member.object {
        JSXMemberExpressionObject::IdentifierReference(id) => {
            idents.insert(id.name.as_str().to_string());
        }
        JSXMemberExpressionObject::MemberExpression(inner) => {
            collect_idents_from_jsx_member_expr(inner, idents);
        }
        JSXMemberExpressionObject::ThisExpression(_) => {}
    }
}

/// Walk a binding pattern collecting identifier references from default values.
fn collect_idents_from_binding_pattern(
    pattern: &BindingPattern<'_>,
    idents: &mut HashSet<String>,
) {
    match pattern {
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_idents_from_binding_pattern(&prop.value, idents);
            }
            if let Some(rest) = &obj.rest {
                collect_idents_from_binding_pattern(&rest.argument, idents);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_idents_from_binding_pattern(elem, idents);
            }
            if let Some(rest) = &arr.rest {
                collect_idents_from_binding_pattern(&rest.argument, idents);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_idents_from_binding_pattern(&assign.left, idents);
            collect_idents_from_expression(&assign.right, idents);
        }
        _ => {}
    }
}

/// Walk a class element collecting identifier references.
fn collect_idents_from_class_element(elem: &ClassElement<'_>, idents: &mut HashSet<String>) {
    match elem {
        ClassElement::MethodDefinition(method) => {
            if let Some(body) = &method.value.body {
                for s in &body.statements {
                    collect_idents_from_statement(s, idents);
                }
            }
        }
        ClassElement::PropertyDefinition(prop) => {
            if let Some(value) = &prop.value {
                collect_idents_from_expression(value, idents);
            }
        }
        ClassElement::StaticBlock(block) => {
            for s in &block.body {
                collect_idents_from_statement(s, idents);
            }
        }
        _ => {}
    }
}

/// Walk an assignment target collecting identifier references.
fn collect_idents_from_assignment_target(
    target: &AssignmentTarget<'_>,
    idents: &mut HashSet<String>,
) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(ident) => {
            idents.insert(ident.name.as_str().to_string());
        }
        AssignmentTarget::StaticMemberExpression(member) => {
            collect_idents_from_expression(&member.object, idents);
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            collect_idents_from_expression(&member.object, idents);
            collect_idents_from_expression(&member.expression, idents);
        }
        _ => {}
    }
}

/// Deep scan a lambda's source code for ALL identifier references, including inside
/// nested arrow/function expressions. Used for iteration variable detection where
/// variables can be captured by nested closures.
/// Unlike `analyze_lambda_captures` which respects function scoping (doesn't descend
/// into nested functions), this descends everywhere to match SWC's `body_contains_ident`.
fn analyze_lambda_deep_ident_refs(source_code: &str, span: (u32, u32)) -> HashSet<String> {
    let start = span.0 as usize;
    let end = span.1 as usize;
    if start >= source_code.len() || end > source_code.len() || start >= end {
        return HashSet::new();
    }
    let lambda_source = &source_code[start..end];

    let parse_source = format!("var x = {}", lambda_source);
    let alloc = oxc::allocator::Allocator::default();
    let source_ref = alloc.alloc_str(&parse_source);

    let parser = oxc::parser::Parser::new(&alloc, source_ref, oxc::span::SourceType::tsx());
    let parse_result = parser.parse();

    if parse_result.program.body.is_empty() {
        return HashSet::new();
    }

    let mut ident_refs = HashSet::new();

    if let Some(Statement::VariableDeclaration(decl)) = parse_result.program.body.first() {
        if let Some(declarator) = decl.declarations.first() {
            if let Some(ref init) = declarator.init {
                walk_expression_deep_idents(init, &mut ident_refs);
            }
        }
    }

    ident_refs
}

/// Walk an expression tree collecting ALL identifier references, descending into
/// nested arrow/function expressions (unlike `walk_expression_for_captures`).
fn walk_expression_deep_idents(expr: &Expression<'_>, idents: &mut HashSet<String>) {
    match expr {
        Expression::Identifier(ident) => {
            idents.insert(ident.name.as_str().to_string());
        }
        Expression::CallExpression(call) => {
            walk_expression_deep_idents(&call.callee, idents);
            for arg in &call.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        walk_expression_deep_idents(&spread.argument, idents);
                    }
                    _ => {
                        if let Some(e) = arg.as_expression() {
                            walk_expression_deep_idents(e, idents);
                        }
                    }
                }
            }
        }
        Expression::StaticMemberExpression(member) => {
            walk_expression_deep_idents(&member.object, idents);
        }
        Expression::ComputedMemberExpression(member) => {
            walk_expression_deep_idents(&member.object, idents);
            walk_expression_deep_idents(&member.expression, idents);
        }
        Expression::BinaryExpression(binary) => {
            walk_expression_deep_idents(&binary.left, idents);
            walk_expression_deep_idents(&binary.right, idents);
        }
        Expression::LogicalExpression(logical) => {
            walk_expression_deep_idents(&logical.left, idents);
            walk_expression_deep_idents(&logical.right, idents);
        }
        Expression::AssignmentExpression(assign) => {
            walk_assignment_target_deep_idents(&assign.left, idents);
            walk_expression_deep_idents(&assign.right, idents);
        }
        Expression::UnaryExpression(unary) => {
            walk_expression_deep_idents(&unary.argument, idents);
        }
        Expression::UpdateExpression(update) => {
            match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                    idents.insert(ident.name.as_str().to_string());
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    walk_expression_deep_idents(&member.object, idents);
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    walk_expression_deep_idents(&member.object, idents);
                    walk_expression_deep_idents(&member.expression, idents);
                }
                _ => {}
            }
        }
        Expression::ConditionalExpression(cond) => {
            walk_expression_deep_idents(&cond.test, idents);
            walk_expression_deep_idents(&cond.consequent, idents);
            walk_expression_deep_idents(&cond.alternate, idents);
        }
        Expression::TemplateLiteral(tmpl) => {
            for e in &tmpl.expressions {
                walk_expression_deep_idents(e, idents);
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        walk_expression_deep_idents(&spread.argument, idents);
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(e) = elem.as_expression() {
                            walk_expression_deep_idents(e, idents);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        walk_expression_deep_idents(&p.value, idents);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        walk_expression_deep_idents(&spread.argument, idents);
                    }
                }
            }
        }
        // Descend into nested functions (unlike walk_expression_for_captures)
        Expression::ArrowFunctionExpression(arrow) => {
            for stmt in &arrow.body.statements {
                walk_statement_deep_idents(stmt, idents);
            }
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                for stmt in &body.statements {
                    walk_statement_deep_idents(stmt, idents);
                }
            }
        }
        Expression::ParenthesizedExpression(paren) => {
            walk_expression_deep_idents(&paren.expression, idents);
        }
        Expression::SequenceExpression(seq) => {
            for e in &seq.expressions {
                walk_expression_deep_idents(e, idents);
            }
        }
        Expression::AwaitExpression(await_expr) => {
            walk_expression_deep_idents(&await_expr.argument, idents);
        }
        _ => {}
    }
}

/// Walk a statement collecting all identifier references (deep -- descends into nested functions).
fn walk_statement_deep_idents(stmt: &Statement<'_>, idents: &mut HashSet<String>) {
    match stmt {
        Statement::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                if let Some(init) = &declarator.init {
                    walk_expression_deep_idents(init, idents);
                }
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            walk_expression_deep_idents(&expr_stmt.expression, idents);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression_deep_idents(arg, idents);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                walk_statement_deep_idents(s, idents);
            }
        }
        Statement::IfStatement(if_stmt) => {
            walk_expression_deep_idents(&if_stmt.test, idents);
            walk_statement_deep_idents(&if_stmt.consequent, idents);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement_deep_idents(alt, idents);
            }
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                if let Some(expr) = init.as_expression() {
                    walk_expression_deep_idents(expr, idents);
                }
            }
            if let Some(test) = &for_stmt.test {
                walk_expression_deep_idents(test, idents);
            }
            if let Some(update) = &for_stmt.update {
                walk_expression_deep_idents(update, idents);
            }
            walk_statement_deep_idents(&for_stmt.body, idents);
        }
        _ => {}
    }
}

/// Walk an assignment target collecting identifier references (deep scan).
fn walk_assignment_target_deep_idents(target: &AssignmentTarget<'_>, idents: &mut HashSet<String>) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(ident) => {
            idents.insert(ident.name.as_str().to_string());
        }
        AssignmentTarget::StaticMemberExpression(member) => {
            walk_expression_deep_idents(&member.object, idents);
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            walk_expression_deep_idents(&member.object, idents);
            walk_expression_deep_idents(&member.expression, idents);
        }
        _ => {}
    }
}

/// Analyze a JSX lambda's source code to extract identifier references and local declarations.
///
/// Parses the lambda source, walks the resulting AST to collect:
/// - All IdentifierReference names (potential captures)
/// - All locally-declared names (parameters, let/const/var declarations)
///
/// Returns (body_ident_refs, body_local_decls) suitable for passing to `compute_captures()`.
fn analyze_lambda_captures(source_code: &str, span: (u32, u32)) -> (Vec<String>, HashSet<String>) {
    let start = span.0 as usize;
    let end = span.1 as usize;
    if start >= source_code.len() || end > source_code.len() || start >= end {
        return (Vec::new(), HashSet::new());
    }
    let lambda_source = &source_code[start..end];

    // Wrap as variable declaration so OXC can parse it
    let parse_source = format!("var x = {}", lambda_source);
    let alloc = oxc::allocator::Allocator::default();
    let source_ref = alloc.alloc_str(&parse_source);

    let parser = oxc::parser::Parser::new(&alloc, source_ref, oxc::span::SourceType::tsx());
    let parse_result = parser.parse();

    // Only bail on empty program body -- not on parse errors.
    // Some errors are semantic (e.g., `await` in non-async function) but the AST
    // is still well-formed and we can still extract identifier references for
    // capture analysis. Bailing on errors causes missing imports in segments.
    if parse_result.program.body.is_empty() {
        return (Vec::new(), HashSet::new());
    }

    // Extract the arrow/function expression from `var x = <expr>`
    if let Some(Statement::VariableDeclaration(decl)) = parse_result.program.body.first() {
        if let Some(declarator) = decl.declarations.first() {
            if let Some(ref init) = declarator.init {
                let mut ident_refs = Vec::new();
                let mut local_decls = HashSet::new();

                // Collect parameter names as local declarations
                match init {
                    Expression::ArrowFunctionExpression(arrow) => {
                        for param in &arrow.params.items {
                            collect_binding_names_from_pattern(&param.pattern, &mut local_decls);
                        }
                        if let Some(rest) = &arrow.params.rest {
                            collect_binding_names_from_pattern(
                                &rest.rest.argument,
                                &mut local_decls,
                            );
                        }
                        // Walk the body for identifier references and local declarations
                        for stmt in &arrow.body.statements {
                            walk_statement_for_captures(stmt, &mut ident_refs, &mut local_decls);
                        }
                        // For expression bodies (single statement with implicit return),
                        // the AST wraps it as a return statement in the body, so we've
                        // already handled it above.
                    }
                    Expression::FunctionExpression(func) => {
                        for param in &func.params.items {
                            collect_binding_names_from_pattern(&param.pattern, &mut local_decls);
                        }
                        if let Some(body) = &func.body {
                            for stmt in &body.statements {
                                walk_statement_for_captures(
                                    stmt,
                                    &mut ident_refs,
                                    &mut local_decls,
                                );
                            }
                        }
                    }
                    _ => {}
                }

                return (ident_refs, local_decls);
            }
        }
    }

    (Vec::new(), HashSet::new())
}

/// Convert a BindingPattern to a human-readable string for paramNames metadata.
/// Matches SWC's `pat_to_string` (transform.rs:2312-2368).
fn binding_pattern_to_string(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => Some(ident.name.as_str().to_string()),
        BindingPattern::ObjectPattern(obj) => {
            let mut parts = Vec::new();
            for prop in &obj.properties {
                if prop.shorthand {
                    // Shorthand {a} -- equivalent to SWC's ObjectPatProp::Assign
                    if let BindingPattern::BindingIdentifier(ident) = &prop.value {
                        parts.push(ident.name.as_str().to_string());
                    }
                } else {
                    // KeyValue {key: value} -- equivalent to SWC's ObjectPatProp::KeyValue
                    let key_str = match &prop.key {
                        PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                        PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
                        PropertyKey::NumericLiteral(n) => n.value.to_string(),
                        PropertyKey::BigIntLiteral(b) => {
                            b.raw.as_ref().map_or_else(String::new, |r| r.as_str().to_string())
                        }
                        _ => continue, // Computed keys: skip
                    };
                    if let Some(value) = binding_pattern_to_string(&prop.value) {
                        parts.push(format!("{}: {}", key_str, value));
                    }
                }
            }
            // Skip rest properties in object patterns (SWC skips ObjectPatProp::Rest)
            if parts.is_empty() {
                None
            } else {
                Some(format!("{{{}}}", parts.join(", ")))
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            let mut parts = Vec::new();
            for elem in &arr.elements {
                match elem {
                    Some(pat) => {
                        if let Some(name) = binding_pattern_to_string(pat) {
                            parts.push(name);
                        }
                    }
                    None => parts.push(String::new()),
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(format!("[{}]", parts.join(", ")))
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            // Assignment pattern with default: use the left-hand side
            binding_pattern_to_string(&assign.left)
        }
    }
}

/// Extract parameter names from FormalParameters.
/// Matches SWC's `extract_param_names` inner logic (transform.rs:2370-2399).
fn extract_param_names_from_params(params: &FormalParameters<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for param in &params.items {
        if let Some(name) = binding_pattern_to_string(&param.pattern) {
            names.push(name);
        }
    }
    // Handle rest parameter: ...args
    if let Some(rest) = &params.rest {
        if let Some(name) = binding_pattern_to_string(&rest.rest.argument) {
            names.push(format!("...{}", name));
        }
    }
    names
}

/// Extract parameter names from a $() call argument.
/// Handles ArrowFunctionExpression and FunctionExpression arguments.
fn extract_param_names_from_argument(arg: &Argument<'_>) -> Vec<String> {
    match arg {
        Argument::ArrowFunctionExpression(arrow) => extract_param_names_from_params(&arrow.params),
        Argument::FunctionExpression(func) => extract_param_names_from_params(&func.params),
        _ => Vec::new(),
    }
}

/// Extract parameter names from a JSX attribute expression (event handler).
/// Handles ArrowFunctionExpression and FunctionExpression in JSX expression containers.
fn extract_param_names_from_jsx_expr(expr: &JSXExpression<'_>) -> Vec<String> {
    match expr {
        JSXExpression::ArrowFunctionExpression(arrow) => {
            extract_param_names_from_params(&arrow.params)
        }
        JSXExpression::FunctionExpression(func) => extract_param_names_from_params(&func.params),
        _ => Vec::new(),
    }
}

/// Collect binding names from a BindingPattern into a set.
fn collect_binding_names_from_pattern(pattern: &BindingPattern<'_>, names: &mut HashSet<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            names.insert(ident.name.as_str().to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names_from_pattern(&prop.value, names);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_names_from_pattern(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_binding_names_from_pattern(elem, names);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_names_from_pattern(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names_from_pattern(&assign.left, names);
        }
    }
}

/// Walk a statement to collect identifier references and local declarations for capture analysis.
fn walk_statement_for_captures(
    stmt: &Statement<'_>,
    ident_refs: &mut Vec<String>,
    local_decls: &mut HashSet<String>,
) {
    match stmt {
        Statement::VariableDeclaration(var_decl) => {
            for declarator in &var_decl.declarations {
                collect_binding_names_from_pattern(&declarator.id, local_decls);
                if let Some(init) = &declarator.init {
                    walk_expression_for_captures(init, ident_refs);
                }
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            walk_expression_for_captures(&expr_stmt.expression, ident_refs);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression_for_captures(arg, ident_refs);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                walk_statement_for_captures(s, ident_refs, local_decls);
            }
        }
        Statement::IfStatement(if_stmt) => {
            walk_expression_for_captures(&if_stmt.test, ident_refs);
            walk_statement_for_captures(&if_stmt.consequent, ident_refs, local_decls);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement_for_captures(alt, ident_refs, local_decls);
            }
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                match init {
                    ForStatementInit::VariableDeclaration(var_decl) => {
                        for declarator in &var_decl.declarations {
                            collect_binding_names_from_pattern(&declarator.id, local_decls);
                            if let Some(init_expr) = &declarator.init {
                                walk_expression_for_captures(init_expr, ident_refs);
                            }
                        }
                    }
                    _ => {
                        if let Some(expr) = init.as_expression() {
                            walk_expression_for_captures(expr, ident_refs);
                        }
                    }
                }
            }
            if let Some(test) = &for_stmt.test {
                walk_expression_for_captures(test, ident_refs);
            }
            if let Some(update) = &for_stmt.update {
                walk_expression_for_captures(update, ident_refs);
            }
            walk_statement_for_captures(&for_stmt.body, ident_refs, local_decls);
        }
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                local_decls.insert(id.name.as_str().to_string());
            }
            // Don't walk inside function body -- inner functions create their own scope
        }
        _ => {}
    }
}

/// Walk an expression to collect identifier references for capture analysis.
fn walk_expression_for_captures(expr: &Expression<'_>, ident_refs: &mut Vec<String>) {
    match expr {
        Expression::Identifier(ident) => {
            ident_refs.push(ident.name.as_str().to_string());
        }
        Expression::CallExpression(call) => {
            walk_expression_for_captures(&call.callee, ident_refs);
            for arg in &call.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        walk_expression_for_captures(&spread.argument, ident_refs);
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            walk_expression_for_captures(expr, ident_refs);
                        }
                    }
                }
            }
        }
        Expression::StaticMemberExpression(member) => {
            walk_expression_for_captures(&member.object, ident_refs);
            // Don't add the property name as an identifier reference
        }
        Expression::ComputedMemberExpression(member) => {
            walk_expression_for_captures(&member.object, ident_refs);
            walk_expression_for_captures(&member.expression, ident_refs);
        }
        Expression::PrivateFieldExpression(member) => {
            walk_expression_for_captures(&member.object, ident_refs);
        }
        Expression::BinaryExpression(binary) => {
            walk_expression_for_captures(&binary.left, ident_refs);
            walk_expression_for_captures(&binary.right, ident_refs);
        }
        Expression::LogicalExpression(logical) => {
            walk_expression_for_captures(&logical.left, ident_refs);
            walk_expression_for_captures(&logical.right, ident_refs);
        }
        Expression::AssignmentExpression(assign) => {
            // For assignment targets, walk to collect identifiers
            walk_assignment_target_for_captures(&assign.left, ident_refs);
            walk_expression_for_captures(&assign.right, ident_refs);
        }
        Expression::UnaryExpression(unary) => {
            walk_expression_for_captures(&unary.argument, ident_refs);
        }
        Expression::UpdateExpression(update) => {
            match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                    ident_refs.push(ident.name.as_str().to_string());
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    walk_expression_for_captures(&member.object, ident_refs);
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    walk_expression_for_captures(&member.object, ident_refs);
                    walk_expression_for_captures(&member.expression, ident_refs);
                }
                SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                    walk_expression_for_captures(&member.object, ident_refs);
                }
                _ => {}
            }
        }
        Expression::ConditionalExpression(cond) => {
            walk_expression_for_captures(&cond.test, ident_refs);
            walk_expression_for_captures(&cond.consequent, ident_refs);
            walk_expression_for_captures(&cond.alternate, ident_refs);
        }
        Expression::TemplateLiteral(tmpl) => {
            for expr in &tmpl.expressions {
                walk_expression_for_captures(expr, ident_refs);
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        walk_expression_for_captures(&spread.argument, ident_refs);
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(expr) = elem.as_expression() {
                            walk_expression_for_captures(expr, ident_refs);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        walk_expression_for_captures(&p.value, ident_refs);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        walk_expression_for_captures(&spread.argument, ident_refs);
                    }
                }
            }
        }
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
            // Don't descend into nested functions -- they create their own scope
        }
        Expression::ParenthesizedExpression(paren) => {
            walk_expression_for_captures(&paren.expression, ident_refs);
        }
        Expression::SequenceExpression(seq) => {
            for expr in &seq.expressions {
                walk_expression_for_captures(expr, ident_refs);
            }
        }
        Expression::AwaitExpression(await_expr) => {
            walk_expression_for_captures(&await_expr.argument, ident_refs);
        }
        Expression::TaggedTemplateExpression(tagged) => {
            walk_expression_for_captures(&tagged.tag, ident_refs);
            for expr in &tagged.quasi.expressions {
                walk_expression_for_captures(expr, ident_refs);
            }
        }
        Expression::NewExpression(new_expr) => {
            walk_expression_for_captures(&new_expr.callee, ident_refs);
            for arg in &new_expr.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        walk_expression_for_captures(&spread.argument, ident_refs);
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            walk_expression_for_captures(expr, ident_refs);
                        }
                    }
                }
            }
        }
        Expression::YieldExpression(yield_expr) => {
            if let Some(arg) = &yield_expr.argument {
                walk_expression_for_captures(arg, ident_refs);
            }
        }
        Expression::ImportExpression(import_expr) => {
            walk_expression_for_captures(&import_expr.source, ident_refs);
        }
        _ => {}
    }
}

/// Walk an assignment target to collect identifier references.
fn walk_assignment_target_for_captures(
    target: &AssignmentTarget<'_>,
    ident_refs: &mut Vec<String>,
) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(ident) => {
            ident_refs.push(ident.name.as_str().to_string());
        }
        AssignmentTarget::StaticMemberExpression(member) => {
            walk_expression_for_captures(&member.object, ident_refs);
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            walk_expression_for_captures(&member.object, ident_refs);
            walk_expression_for_captures(&member.expression, ident_refs);
        }
        _ => {}
    }
}

/// Serialize a JSX lambda expression to a code string for segment body generation.
///
/// Uses a parse-and-codegen roundtrip: extracts the lambda source code from the
/// original source text using the span, wraps it as `var x = <lambda>`, parses it,
/// and then runs codegen on the expression to produce clean output.
///
/// This avoids unsafe pointer casts between JSXExpression and Expression types
/// (which use different enum layouts despite sharing variant types via inherit_variants!).
fn serialize_jsx_lambda_from_source(source_code: &str, span: (u32, u32)) -> String {
    let start = span.0 as usize;
    let end = span.1 as usize;
    if start >= source_code.len() || end > source_code.len() || start >= end {
        return String::new();
    }
    let lambda_source = &source_code[start..end];

    // Wrap in a variable declaration so OXC can parse it as a complete expression
    let parse_source = format!("var x = {}", lambda_source);
    let alloc = oxc::allocator::Allocator::default();
    let source_ref = alloc.alloc_str(&parse_source);

    let parser = oxc::parser::Parser::new(&alloc, source_ref, oxc::span::SourceType::tsx());
    let parse_result = parser.parse();

    if !parse_result.errors.is_empty() || parse_result.program.body.is_empty() {
        // Fallback: return the raw source text
        return lambda_source.to_string();
    }

    // Extract the expression from `var x = <expr>`
    if let Some(Statement::VariableDeclaration(decl)) = parse_result.program.body.first() {
        if let Some(declarator) = decl.declarations.first() {
            if let Some(ref init) = declarator.init {
                let mut codegen = oxc::codegen::Codegen::new();
                codegen.print_expression(init);
                return codegen.into_source_text();
            }
        }
    }

    lambda_source.to_string()
}

/// Check if a dollar-suffixed call name produces a tree-shakeable wrapper.
///
/// Word-boundary-aware identifier replacement in body code strings.
///
/// Replaces standalone occurrences of `old_name` with `new_name`, respecting
/// word boundaries (preceding/following chars must not be identifier chars).
/// Used to post-process segment body strings for inline component prop alias
/// replacement (e.g., `data.X` -> `_rawProps.data.X`).
fn replace_identifier_in_body(code: &str, old_name: &str, new_name: &str) -> String {
    let mut result = String::with_capacity(code.len());
    let chars: Vec<char> = code.chars().collect();
    let old_chars: Vec<char> = old_name.chars().collect();
    let old_len = old_chars.len();
    let mut i = 0;

    while i < chars.len() {
        if i + old_len <= chars.len() && &chars[i..i + old_len] == old_chars.as_slice() {
            let before_ok = i == 0 || !is_ident_char_body(chars[i - 1]);
            let after_ok = i + old_len >= chars.len() || !is_ident_char_body(chars[i + old_len]);

            if before_ok && after_ok {
                result.push_str(new_name);
                i += old_len;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Check if a character is a valid identifier character (alphanumeric or underscore or $).
fn is_ident_char_body(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Fix QRL capture arrays in a component body after props reclassification.
///
/// After props destructuring rewrite converts `foo` -> `_rawProps.foo` in the body,
/// QRL calls like `qrl(import, "name", [_rawProps.foo, arg0])` have member expressions
/// instead of plain identifiers. This function finds those QRL calls by matching
/// segment names and rebuilds their capture arrays to use the reclassified
/// capture names (e.g., `[_rawProps, arg0]` or just `[_rawProps]`).
fn fix_qrl_captures_in_body<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    reclassified: &[(String, Vec<String>)], // (segment_name, capture_names)
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for stmt in stmts.iter_mut() {
        fix_qrl_captures_in_stmt(stmt, reclassified, ctx);
    }
}

fn fix_qrl_captures_in_stmt<'a>(
    stmt: &mut Statement<'a>,
    reclassified: &[(String, Vec<String>)],
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match stmt {
        Statement::ReturnStatement(ret) => {
            if let Some(ref mut expr) = ret.argument {
                fix_qrl_captures_in_expr(expr, reclassified, ctx);
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            fix_qrl_captures_in_expr(&mut expr_stmt.expression, reclassified, ctx);
        }
        Statement::VariableDeclaration(decl) => {
            for declarator in decl.declarations.iter_mut() {
                if let Some(ref mut init) = declarator.init {
                    fix_qrl_captures_in_expr(init, reclassified, ctx);
                }
            }
        }
        Statement::IfStatement(if_stmt) => {
            fix_qrl_captures_in_stmt(&mut if_stmt.consequent, reclassified, ctx);
            if let Some(ref mut alt) = if_stmt.alternate {
                fix_qrl_captures_in_stmt(alt, reclassified, ctx);
            }
        }
        Statement::BlockStatement(block) => {
            fix_qrl_captures_in_body(&mut block.body, reclassified, ctx);
        }
        _ => {}
    }
}

fn fix_qrl_captures_in_expr<'a>(
    expr: &mut Expression<'a>,
    reclassified: &[(String, Vec<String>)],
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match expr {
        Expression::CallExpression(call) => {
            // Check if this is a qrl/inlinedQrl call with a segment name matching a reclassified segment
            let is_qrl_call = match &call.callee {
                Expression::Identifier(id) => {
                    let name = id.name.as_str();
                    name == "qrl" || name == "qrlDEV" || name == "inlinedQrl" || name == "inlinedQrlDEV"
                }
                _ => false,
            };

            if is_qrl_call && call.arguments.len() >= 3 {
                // The segment name is in the 2nd argument (string literal)
                let seg_name = match &call.arguments[1] {
                    Argument::StringLiteral(s) => Some(s.value.as_str().to_string()),
                    _ => None,
                };

                if let Some(ref name) = seg_name {
                    if let Some((_, capture_names)) = reclassified.iter().find(|(sn, _)| sn == name) {
                        // Rebuild the captures array (3rd argument)
                        let mut elements = ctx.ast.vec_with_capacity(capture_names.len());
                        for cap_name in capture_names {
                            let ident = ctx.ast.expression_identifier(
                                SPAN,
                                ctx.ast.atom(cap_name.as_str()),
                            );
                            elements.push(ArrayExpressionElement::from(ident));
                        }
                        let new_array = ctx.ast.expression_array(SPAN, elements);
                        call.arguments[2] = Argument::from(new_array);
                    }
                }
            }

            // Recurse into call arguments for nested QRL calls (e.g., useTaskQrl(inlinedQrl(...)))
            for arg in call.arguments.iter_mut() {
                match arg {
                    Argument::CallExpression(inner_call) => {
                        // Recurse by wrapping in Expression
                        // Actually, arguments are Argument, not Expression. Handle inline.
                        let is_inner_qrl = match &inner_call.callee {
                            Expression::Identifier(id) => {
                                let n = id.name.as_str();
                                n == "qrl" || n == "qrlDEV" || n == "inlinedQrl" || n == "inlinedQrlDEV"
                            }
                            _ => false,
                        };
                        if is_inner_qrl && inner_call.arguments.len() >= 3 {
                            let inner_seg_name = match &inner_call.arguments[1] {
                                Argument::StringLiteral(s) => Some(s.value.as_str().to_string()),
                                _ => None,
                            };
                            if let Some(ref name) = inner_seg_name {
                                if let Some((_, capture_names)) = reclassified.iter().find(|(sn, _)| sn == name) {
                                    let mut elements = ctx.ast.vec_with_capacity(capture_names.len());
                                    for cap_name in capture_names {
                                        let ident = ctx.ast.expression_identifier(
                                            SPAN,
                                            ctx.ast.atom(cap_name.as_str()),
                                        );
                                        elements.push(ArrayExpressionElement::from(ident));
                                    }
                                    let new_array = ctx.ast.expression_array(SPAN, elements);
                                    inner_call.arguments[2] = Argument::from(new_array);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Recurse into PURE comment wrappers and other expression types
        Expression::SequenceExpression(seq) => {
            for e in seq.expressions.iter_mut() {
                fix_qrl_captures_in_expr(e, reclassified, ctx);
            }
        }
        _ => {}
    }
}

/// Only `component$` produces a side-effect-free wrapper (`componentQrl`).
/// All other wrappers (useStylesQrl, useTaskQrl, useVisibleTaskQrl,
/// serverStuffQrl, serverLoaderQrl, useResourceQrl, etc.) are side-effectful
/// runtime calls that must NOT be annotated with `/*#__PURE__*/`.
fn is_tree_shakeable_dollar_call(name: &str) -> bool {
    name == "component$"
}

/// Minify a function string for sync$ serialization.
/// Removes comments and normalizes whitespace via parse+codegen roundtrip.
/// Uses minified codegen to match SWC's sync QRL string format
/// (no spaces, no newlines, no tabs).
fn minify_fn_string(source: &str) -> String {
    let parse_source = format!("var x = {}", source);
    let parse_alloc = oxc::allocator::Allocator::default();
    let source_for_parse = parse_alloc.alloc_str(&parse_source);

    let parser =
        oxc::parser::Parser::new(&parse_alloc, source_for_parse, oxc::span::SourceType::mjs());
    let parse_result = parser.parse();

    if !parse_result.errors.is_empty() || parse_result.program.body.is_empty() {
        return source.to_string();
    }

    if let Some(Statement::VariableDeclaration(decl)) = parse_result.program.body.first() {
        if let Some(declarator) = decl.declarations.first() {
            if let Some(ref init) = declarator.init {
                let options = oxc::codegen::CodegenOptions {
                    minify: true,
                    ..Default::default()
                };
                let mut codegen = oxc::codegen::Codegen::new().with_options(options);
                codegen.print_expression(init);
                let mut result = codegen.into_source_text();

                // Post-process to match SWC's sync QRL string format:
                // 1. Strip outer parentheses that OXC adds around function expressions
                //    (SWC prints `function(...){}` not `(function(...){})`).
                if (result.starts_with("(function") || result.starts_with("(async function"))
                    && result.ends_with(')')
                {
                    result = result[1..result.len() - 1].to_string();
                }
                // 2. OXC minified mode omits trailing semicolons in block bodies.
                //    SWC includes them. Add `;` before each `}` that follows a
                //    statement (not after `{` which would be an empty block).
                //    Simple heuristic: replace `)}` with `);}` when preceded by
                //    a statement-ending character.
                result = result.replace(")}", ");}");

                return result;
            }
        }
    }

    source.to_string()
}

/// Serialize an expression to a code string, preserving source comments.
///
/// Creates a temporary Program containing the expression as an ExpressionStatement,
/// includes source comments whose `attached_to` positions fall within the expression's
/// span range, and uses `Codegen::build()` to emit the code with comments.
///
/// The result is the expression code without the trailing semicolon/newline that
/// `build()` adds for the ExpressionStatement.
fn codegen_expression_with_comments<'a>(
    expr: Expression<'a>,
    source_text: &str,
    source_comments: &[Comment],
    ctx: &mut TraverseCtx<'a, ()>,
) -> String {
    use oxc::span::{GetSpan, SourceType};

    let expr_span = expr.span();

    // Filter comments to those attached to nodes within the expression's span range.
    let mut comments = ctx.ast.vec_with_capacity(source_comments.len());
    for comment in source_comments {
        if comment.attached_to >= expr_span.start && comment.attached_to < expr_span.end {
            comments.push(*comment);
        }
    }

    // Build a temporary program containing just this expression as an ExpressionStatement.
    let stmt = ctx.ast.statement_expression(expr_span, expr);
    let mut body = ctx.ast.vec_with_capacity(1);
    body.push(stmt);

    let source_in_arena = ctx.ast.allocator.alloc_str(source_text);
    let program = ctx.ast.program(
        expr_span,
        SourceType::mjs(),
        source_in_arena,
        comments,
        None,
        ctx.ast.vec(),
        body,
    );

    let codegen_result = oxc::codegen::Codegen::new().build(&program);
    let code = codegen_result.code;

    // build() emits "expression;\n" for an ExpressionStatement.
    // Strip the trailing ";\n" to get just the expression code.
    code.trim_end().strip_suffix(';').unwrap_or(code.trim_end()).to_string()
}

/// Convert an Argument to an Expression.
///
/// In OXC 0.113, Argument uses inherit_variants! from Expression, meaning
/// all Expression variants are directly on Argument. We need to match and convert.
pub(crate) fn argument_to_expression<'a>(
    arg: Argument<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    match arg {
        Argument::SpreadElement(_) => ctx.ast.expression_identifier(SPAN, "undefined"),
        // Argument inherits all Expression variants
        Argument::ArrayExpression(e) => Expression::ArrayExpression(e),
        Argument::ArrowFunctionExpression(e) => Expression::ArrowFunctionExpression(e),
        Argument::AssignmentExpression(e) => Expression::AssignmentExpression(e),
        Argument::AwaitExpression(e) => Expression::AwaitExpression(e),
        Argument::BinaryExpression(e) => Expression::BinaryExpression(e),
        Argument::CallExpression(e) => Expression::CallExpression(e),
        Argument::ChainExpression(e) => Expression::ChainExpression(e),
        Argument::ClassExpression(e) => Expression::ClassExpression(e),
        Argument::ConditionalExpression(e) => Expression::ConditionalExpression(e),
        Argument::FunctionExpression(e) => Expression::FunctionExpression(e),
        Argument::Identifier(e) => Expression::Identifier(e),
        Argument::ImportExpression(e) => Expression::ImportExpression(e),
        Argument::LogicalExpression(e) => Expression::LogicalExpression(e),
        Argument::MetaProperty(e) => Expression::MetaProperty(e),
        Argument::NewExpression(e) => Expression::NewExpression(e),
        Argument::ObjectExpression(e) => Expression::ObjectExpression(e),
        Argument::ParenthesizedExpression(e) => Expression::ParenthesizedExpression(e),
        Argument::SequenceExpression(e) => Expression::SequenceExpression(e),
        Argument::TaggedTemplateExpression(e) => Expression::TaggedTemplateExpression(e),
        Argument::TemplateLiteral(e) => Expression::TemplateLiteral(e),
        Argument::ThisExpression(e) => Expression::ThisExpression(e),
        Argument::UnaryExpression(e) => Expression::UnaryExpression(e),
        Argument::UpdateExpression(e) => Expression::UpdateExpression(e),
        Argument::YieldExpression(e) => Expression::YieldExpression(e),
        Argument::BooleanLiteral(e) => Expression::BooleanLiteral(e),
        Argument::NullLiteral(e) => Expression::NullLiteral(e),
        Argument::NumericLiteral(e) => Expression::NumericLiteral(e),
        Argument::BigIntLiteral(e) => Expression::BigIntLiteral(e),
        Argument::RegExpLiteral(e) => Expression::RegExpLiteral(e),
        Argument::StringLiteral(e) => Expression::StringLiteral(e),
        // MemberExpression variants (inherited via inherit_variants!)
        Argument::ComputedMemberExpression(e) => Expression::ComputedMemberExpression(e),
        Argument::StaticMemberExpression(e) => Expression::StaticMemberExpression(e),
        Argument::PrivateFieldExpression(e) => Expression::PrivateFieldExpression(e),
        Argument::Super(e) => Expression::Super(e),
        Argument::V8IntrinsicExpression(e) => Expression::V8IntrinsicExpression(e),
        Argument::PrivateInExpression(e) => Expression::PrivateInExpression(e),
        // TypeScript expressions
        Argument::TSAsExpression(e) => Expression::TSAsExpression(e),
        Argument::TSSatisfiesExpression(e) => Expression::TSSatisfiesExpression(e),
        Argument::TSTypeAssertion(e) => Expression::TSTypeAssertion(e),
        Argument::TSNonNullExpression(e) => Expression::TSNonNullExpression(e),
        Argument::TSInstantiationExpression(e) => Expression::TSInstantiationExpression(e),
        // Catch-all for any other inherited variants
        _ => ctx.ast.expression_identifier(SPAN, "undefined"),
    }
}

/// Extract callback parameter names from the first argument of an iteration method.
/// E.g., for `.map((item, index) => ...)`, returns `["item", "index"]`.
fn extract_callback_params(arg: &Argument<'_>) -> Vec<String> {
    match arg {
        Argument::ArrowFunctionExpression(arrow) => arrow
            .params
            .items
            .iter()
            .filter_map(|p| {
                if let BindingPattern::BindingIdentifier(ref ident) = p.pattern {
                    Some(ident.name.to_string())
                } else {
                    None
                }
            })
            .collect(),
        Argument::FunctionExpression(func) => func
            .params
            .items
            .iter()
            .filter_map(|p| {
                if let BindingPattern::BindingIdentifier(ref ident) = p.pattern {
                    Some(ident.name.to_string())
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Rewrite body destructuring alias references in non-JSX statements.
///
/// Replaces occurrences of `alias` with `props["key"]` (computed member access)
/// in variable declarations and expression statements. Does NOT touch return
/// statements since those contain JSX which is handled by detect_signal_wrap.
fn rewrite_body_destr_references<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    prop_map: &[(String, String)], // (local_alias, original_key)
    props_param_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    for stmt in stmts.iter_mut() {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                for declarator in decl.declarations.iter_mut() {
                    if let Some(ref mut init) = declarator.init {
                        rewrite_expr_body_destr(init, prop_map, props_param_name, ctx);
                    }
                }
            }
            Statement::ExpressionStatement(expr_stmt) => {
                rewrite_expr_body_destr(&mut expr_stmt.expression, prop_map, props_param_name, ctx);
            }
            // Don't rewrite return statements -- JSX children are handled by detect_signal_wrap
            _ => {}
        }
    }
}

/// Recursively rewrite identifier references matching body destructuring aliases
/// to `props["key"]` computed member expressions.
fn rewrite_expr_body_destr<'a>(
    expr: &mut Expression<'a>,
    prop_map: &[(String, String)],
    props_param_name: &str,
    ctx: &mut TraverseCtx<'a, ()>,
) {
    match expr {
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            for (local_alias, original_key) in prop_map {
                if local_alias == name {
                    // Replace with props["original_key"]
                    let obj = ctx.ast.expression_identifier(SPAN, ctx.ast.atom(props_param_name));
                    let key_atom = ctx.ast.atom(original_key.as_str());
                    let key_expr = ctx.ast.expression_string_literal(SPAN, key_atom, None);
                    let member = ctx.ast.computed_member_expression(SPAN, obj, key_expr, false);
                    *expr = Expression::ComputedMemberExpression(ctx.ast.alloc(member));
                    return;
                }
            }
        }
        Expression::CallExpression(call) => {
            rewrite_expr_body_destr(&mut call.callee, prop_map, props_param_name, ctx);
            for i in 0..call.arguments.len() {
                let placeholder = Argument::from(ctx.ast.expression_identifier(SPAN, "undefined"));
                let old = std::mem::replace(&mut call.arguments[i], placeholder);
                let mut arg_expr = argument_to_expression(old, ctx);
                rewrite_expr_body_destr(&mut arg_expr, prop_map, props_param_name, ctx);
                call.arguments[i] = Argument::from(arg_expr);
            }
        }
        Expression::BinaryExpression(bin) => {
            rewrite_expr_body_destr(&mut bin.left, prop_map, props_param_name, ctx);
            rewrite_expr_body_destr(&mut bin.right, prop_map, props_param_name, ctx);
        }
        Expression::StaticMemberExpression(mem) => {
            rewrite_expr_body_destr(&mut mem.object, prop_map, props_param_name, ctx);
        }
        Expression::ComputedMemberExpression(mem) => {
            rewrite_expr_body_destr(&mut mem.object, prop_map, props_param_name, ctx);
            rewrite_expr_body_destr(&mut mem.expression, prop_map, props_param_name, ctx);
        }
        Expression::ConditionalExpression(cond) => {
            rewrite_expr_body_destr(&mut cond.test, prop_map, props_param_name, ctx);
            rewrite_expr_body_destr(&mut cond.consequent, prop_map, props_param_name, ctx);
            rewrite_expr_body_destr(&mut cond.alternate, prop_map, props_param_name, ctx);
        }
        Expression::LogicalExpression(log) => {
            rewrite_expr_body_destr(&mut log.left, prop_map, props_param_name, ctx);
            rewrite_expr_body_destr(&mut log.right, prop_map, props_param_name, ctx);
        }
        Expression::UnaryExpression(unary) => {
            rewrite_expr_body_destr(&mut unary.argument, prop_map, props_param_name, ctx);
        }
        Expression::ParenthesizedExpression(paren) => {
            rewrite_expr_body_destr(&mut paren.expression, prop_map, props_param_name, ctx);
        }
        Expression::TemplateLiteral(tmpl) => {
            for e in tmpl.expressions.iter_mut() {
                rewrite_expr_body_destr(e, prop_map, props_param_name, ctx);
            }
        }
        _ => {}
    }
}

// =============================================================================
// Segment body DCE (Dead Code Elimination)
// =============================================================================

/// Apply dead code elimination to a segment body code string.
///
/// This implements SWC's MinifyMode::Simplify behavior for segment bodies:
/// - Strip unused const/let/var declarations (convert to expression statements if init has side effects)
/// - Remove unused function/class declarations
/// - Eliminate if(false) branches and if(true) branches (keep consequent)
/// - Remove empty try/catch blocks
/// - Optionally force-remove named function/class declarations (for invalid_decl_stack C02 names)
///
/// **Not yet implemented** (deferred to Phase 14 -- minor diffs):
/// - Const literal propagation: `const key = "A"; f(key)` -> `f("A")` (SWC MinifyMode::Simplify)
/// - Destructured const chain folding: `const {a} = x; a.b` -> `x.a.b` (SWC MinifyMode::Simplify)
///
/// Note: isBrowser/isServer dead branch elimination is handled by the const_replace pre-pass
/// (const_replace.rs) which runs on the full program AST before segment extraction. The VisitMut
/// walker recurses into all AST nodes including inlinedQrl callback arguments, so the
/// replacement reaches inline strategy entry code as well as segment bodies.
///
/// The function parses the body code by wrapping it as `var __body__ = <body_code>`,
/// transforms the AST, and re-serializes.
pub(crate) fn apply_segment_body_dce(
    body_code: &str,
    force_remove_names: Option<&HashSet<String>>,
) -> String {
    // Quick check: skip if body code is trivial (no declarations or if statements)
    let needs_dce = body_code.contains("const ")
        || body_code.contains("let ")
        || body_code.contains("var ")
        || body_code.contains("function ")
        || body_code.contains("class ")
        || body_code.contains("if ")
        || body_code.contains("if(")
        || body_code.contains("try ")
        || body_code.contains("try{")
        || force_remove_names.map_or(false, |s| !s.is_empty());
    if !needs_dce {
        return body_code.to_string();
    }

    let alloc = oxc::allocator::Allocator::default();
    let parse_source = format!("var __body__ = {}", body_code);
    let source_ref = alloc.alloc_str(&parse_source);

    let parser = oxc::parser::Parser::new(&alloc, source_ref, oxc::span::SourceType::tsx());
    let mut parse_result = parser.parse();

    if !parse_result.errors.is_empty() || parse_result.program.body.is_empty() {
        return body_code.to_string();
    }

    // Extract the arrow/function expression from `var __body__ = <expr>`
    let Some(Statement::VariableDeclaration(var_decl)) = parse_result.program.body.first_mut() else {
        return body_code.to_string();
    };
    let Some(ref mut declarator) = var_decl.declarations.first_mut() else {
        return body_code.to_string();
    };
    let Some(ref mut init_expr) = declarator.init else {
        return body_code.to_string();
    };

    // Get the body statements from the arrow/function expression
    let body_stmts = match init_expr {
        Expression::ArrowFunctionExpression(arrow) => &mut arrow.body.statements,
        Expression::FunctionExpression(func) => {
            if let Some(ref mut body) = func.body {
                &mut body.statements
            } else {
                return body_code.to_string();
            }
        }
        _ => return body_code.to_string(),
    };

    let changed = apply_dce_to_statements(body_stmts, force_remove_names, &alloc);

    if !changed {
        return body_code.to_string();
    }

    // Re-serialize the modified expression
    let codegen_result = oxc::codegen::Codegen::new().build(&parse_result.program);
    let full_code = codegen_result.code;

    // Extract the expression part from "var __body__ = <expr>;\n"
    let prefix = "var __body__ = ";
    if let Some(rest) = full_code.strip_prefix(prefix) {
        // Strip trailing ";\n"
        let trimmed = rest.trim_end();
        let result = if let Some(stripped) = trimmed.strip_suffix(';') {
            stripped
        } else {
            trimmed
        };
        result.to_string()
    } else {
        body_code.to_string()
    }
}

/// Apply DCE to a list of statements. Returns true if any changes were made.
/// This is recursive -- it descends into nested function/arrow bodies to find
/// if(false) branches and unused declarations in nested scopes.
fn apply_dce_to_statements<'a>(
    stmts: &mut oxc::allocator::Vec<'a, Statement<'a>>,
    force_remove_names: Option<&HashSet<String>>,
    alloc: &'a oxc::allocator::Allocator,
) -> bool {
    let mut changed = false;

    // Pass 1: Recursively process nested function/arrow bodies for if(false) elimination
    for stmt in stmts.iter_mut() {
        changed |= apply_dce_recursive_into_stmt(stmt, alloc);
    }

    // Pass 2: Collect all referenced identifiers in this scope
    let referenced = collect_all_references_in_stmts(stmts);

    // Pass 3: Determine which statements to remove/transform
    // We collect actions, then apply them in reverse order to preserve indices
    let mut actions: Vec<(usize, DceAction)> = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            // const x = expr; / let x = expr;
            Statement::VariableDeclaration(decl) => {
                if let Some(action) = check_unused_var_decl(decl, &referenced, force_remove_names) {
                    actions.push((i, action));
                }
            }
            // function f() {}
            Statement::FunctionDeclaration(func) => {
                if let Some(ref id) = func.id {
                    let name = id.name.as_str();
                    let force_remove = force_remove_names.map_or(false, |s| s.contains(name));
                    if force_remove || !referenced.contains(name) {
                        actions.push((i, DceAction::Remove));
                        continue;
                    }
                }
            }
            // class C {}
            Statement::ClassDeclaration(class) => {
                if let Some(ref id) = class.id {
                    let name = id.name.as_str();
                    let force_remove = force_remove_names.map_or(false, |s| s.contains(name));
                    if force_remove || !referenced.contains(name) {
                        // Check if class has side-effectful computed properties
                        if force_remove || !class_has_side_effects(class) {
                            actions.push((i, DceAction::Remove));
                            continue;
                        }
                    }
                }
            }
            // if (false) { ... }
            Statement::IfStatement(if_stmt) => {
                if let Some(test_val) = eval_bool_literal(&if_stmt.test) {
                    if !test_val {
                        // if(false) -- remove entirely or replace with else branch
                        if if_stmt.alternate.is_some() {
                            actions.push((i, DceAction::ReplaceWithAlternate));
                        } else {
                            actions.push((i, DceAction::Remove));
                        }
                    } else {
                        // if(true) -- replace with consequent
                        actions.push((i, DceAction::ReplaceWithConsequent));
                    }
                }
            }
            // try {} catch (e) {} -- remove if both try and catch are empty/trivial
            Statement::TryStatement(try_stmt) => {
                if try_block_is_empty(try_stmt) {
                    actions.push((i, DceAction::Remove));
                }
            }
            _ => {}
        }
    }

    if actions.is_empty() {
        return changed;
    }

    // Apply actions - rebuild the statement list
    let mut action_map: HashMap<usize, DceAction> = actions.into_iter().collect();
    let all_stmts: Vec<Statement<'_>> = stmts.drain(..).collect();
    let mut result: Vec<Statement<'_>> = Vec::new();

    for (i, stmt) in all_stmts.into_iter().enumerate() {
        if let Some(action) = action_map.remove(&i) {
            match action {
                DceAction::Remove => {
                    // Drop the statement
                }
                DceAction::ConvertToExprStmt => {
                    // Convert `const x = expr;` to `expr;`
                    if let Statement::VariableDeclaration(mut decl) = stmt {
                        if let Some(mut declarator) = decl.declarations.pop() {
                            if let Some(init) = declarator.init.take() {
                                result.push(Statement::ExpressionStatement(
                                    oxc::allocator::Box::new_in(
                                        ExpressionStatement {
                                            span: SPAN,
                                            expression: init,
                                        },
                                        alloc,
                                    ),
                                ));
                            }
                        }
                    }
                }
                DceAction::ReplaceWithAlternate => {
                    if let Statement::IfStatement(mut if_stmt) = stmt {
                        if let Some(alternate) = if_stmt.alternate.take() {
                            if let Statement::BlockStatement(block) = alternate {
                                for s in block.unbox().body.into_iter() {
                                    result.push(s);
                                }
                            } else {
                                result.push(alternate);
                            }
                        }
                    }
                }
                DceAction::ReplaceWithConsequent => {
                    if let Statement::IfStatement(if_stmt) = stmt {
                        let consequent = if_stmt.unbox().consequent;
                        if let Statement::BlockStatement(block) = consequent {
                            for s in block.unbox().body.into_iter() {
                                result.push(s);
                            }
                        } else {
                            result.push(consequent);
                        }
                    }
                }
                DceAction::StripUnusedDeclarators(keep_indices) => {
                    // Keep only the declarators at the specified indices
                    if let Statement::VariableDeclaration(mut decl) = stmt {
                        let all_declarators: Vec<_> = decl.declarations.drain(..).collect();
                        for (idx, d) in all_declarators.into_iter().enumerate() {
                            if keep_indices.contains(&idx) {
                                decl.declarations.push(d);
                            }
                        }
                        if !decl.declarations.is_empty() {
                            result.push(Statement::VariableDeclaration(decl));
                        }
                    }
                }
            }
        } else {
            result.push(stmt);
        }
    }

    for stmt in result {
        stmts.push(stmt);
    }

    true
}

/// DCE action to apply to a statement.
enum DceAction {
    /// Remove the statement entirely.
    Remove,
    /// Convert `const x = expr;` to `expr;` (keep init as expression statement).
    ConvertToExprStmt,
    /// Replace if(false){...}else{alt} with alt block contents.
    ReplaceWithAlternate,
    /// Replace if(true){consequent} with consequent block contents.
    ReplaceWithConsequent,
    /// Keep only specific declarators in a multi-declarator var/let/const.
    StripUnusedDeclarators(Vec<usize>),
}

/// Check if a variable declaration has unused bindings and determine the DCE action.
fn check_unused_var_decl(
    decl: &VariableDeclaration<'_>,
    referenced: &HashSet<String>,
    force_remove_names: Option<&HashSet<String>>,
) -> Option<DceAction> {
    let declarators = &decl.declarations;

    if declarators.len() == 1 {
        let d = &declarators[0];
        let names = collect_binding_names(&d.id);
        if names.is_empty() {
            return None;
        }

        let any_referenced = names.iter().any(|n| referenced.contains(n.as_str()));
        let force_remove = force_remove_names
            .map_or(false, |s| names.iter().any(|n| s.contains(n.as_str())));

        if !force_remove && any_referenced {
            return None; // Binding is used, keep it
        }

        // Check if this is a destructuring pattern (object/array).
        // Destructuring access itself is a side effect (property access/iterator protocol).
        // SWC keeps destructured patterns when the init could have side effects:
        //   - `const { a, b } = this;` -- kept (destructuring `this` = property access)
        //   - `let [x, ...y] = stuff;` -- kept (iterator protocol on `stuff`)
        // Only simple identifier bindings get converted to expression statements.
        let is_destructuring = matches!(
            d.id,
            BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_)
        );

        // Binding is unused -- check if init has side effects
        match &d.init {
            None => Some(DceAction::Remove), // `let x;` -- safe to remove
            Some(init) => {
                if init_is_side_effect_free(init) {
                    Some(DceAction::Remove)
                } else if is_destructuring {
                    // Destructuring patterns with side-effectful init: keep as-is.
                    // The destructuring access (property access, iterator) is itself
                    // a side effect that SWC preserves.
                    None
                } else {
                    // Simple binding with side-effectful init: convert to expression
                    Some(DceAction::ConvertToExprStmt)
                }
            }
        }
    } else {
        // Multi-declarator: `let x = 1, y;`
        // Check each declarator individually
        let mut keep_indices = Vec::new();
        let mut any_removed = false;

        for (idx, d) in declarators.iter().enumerate() {
            let names = collect_binding_names(&d.id);
            let any_referenced = names.iter().any(|n| referenced.contains(n.as_str()));
            let force_remove = force_remove_names
                .map_or(false, |s| names.iter().any(|n| s.contains(n.as_str())));

            if force_remove || !any_referenced {
                // Check if init has side effects -- if so, we need to keep it somehow
                // For simplicity, if any declarator in a multi-decl has side effects, keep it
                match &d.init {
                    None => {
                        any_removed = true;
                    }
                    Some(init) => {
                        if init_is_side_effect_free(init) {
                            any_removed = true;
                        } else {
                            keep_indices.push(idx); // Has side effects, keep
                        }
                    }
                }
            } else {
                keep_indices.push(idx);
            }
        }

        if any_removed {
            if keep_indices.is_empty() {
                Some(DceAction::Remove)
            } else {
                Some(DceAction::StripUnusedDeclarators(keep_indices))
            }
        } else {
            None
        }
    }
}

/// Collect binding names from a BindingPatternKind.
fn collect_binding_names(kind: &BindingPattern<'_>) -> Vec<String> {
    let mut names = Vec::new();
    collect_binding_names_inner(kind, &mut names);
    names
}

fn collect_binding_names_inner(kind: &BindingPattern<'_>, names: &mut Vec<String>) {
    match kind {
        BindingPattern::BindingIdentifier(id) => {
            names.push(id.name.to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names_inner(&prop.value, names);
            }
            if let Some(ref rest) = obj.rest {
                collect_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_binding_names_inner(elem, names);
            }
            if let Some(ref rest) = arr.rest {
                collect_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names_inner(&assign.left, names);
        }
    }
}

/// Check if an expression initializer is side-effect-free.
/// Conservative: only returns true for literals, identifiers, and simple expressions
/// that definitely cannot have side effects.
fn init_is_side_effect_free(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,
        Expression::Identifier(_) => false, // Could be a getter
        Expression::UnaryExpression(unary) => init_is_side_effect_free(&unary.argument),
        // Member access can have side effects (getters)
        Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_) => false,
        // Binary expressions: the operands could be getters
        Expression::BinaryExpression(bin) => {
            init_is_side_effect_free(&bin.left) && init_is_side_effect_free(&bin.right)
        }
        // Call expressions always have side effects
        Expression::CallExpression(_) => false,
        // Object/array literals can have computed keys or spread elements
        Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => false,
        // Template literals can have expressions with side effects
        Expression::TemplateLiteral(_) => false,
        // Arrow/function expressions are safe (they don't execute)
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => true,
        // `this` is side-effect-free
        Expression::ThisExpression(_) => false, // Could be undefined in strict mode, but SWC keeps it
        _ => false, // Conservative default
    }
}

/// Check if a class declaration has side-effectful features.
/// Classes with computed property names, decorators, or property initializers
/// that reference external values are considered side-effectful.
fn class_has_side_effects(class: &Class<'_>) -> bool {
    for element in &class.body.body {
        match element {
            ClassElement::PropertyDefinition(prop) => {
                // Instance property with initializer = side effect (runs at instantiation)
                // But computed key = side effect at class definition time
                if prop.computed {
                    return true;
                }
                if prop.value.is_some() {
                    return true;
                }
            }
            ClassElement::StaticBlock(_) => return true,
            _ => {}
        }
    }
    false
}

/// Evaluate an if-statement test to a boolean literal value.
/// Returns Some(true/false) for BooleanLiteral or !true/!false.
fn eval_bool_literal(expr: &Expression<'_>) -> Option<bool> {
    match expr {
        Expression::BooleanLiteral(lit) => Some(lit.value),
        Expression::UnaryExpression(unary) if unary.operator == oxc::ast::ast::UnaryOperator::LogicalNot => {
            eval_bool_literal(&unary.argument).map(|v| !v)
        }
        Expression::NumericLiteral(lit) => {
            // `if (0)` is false, `if (1)` is true
            Some(lit.value != 0.0)
        }
        _ => None,
    }
}

/// Check if a try statement is empty (both try block and catch handler are empty/trivial).
fn try_block_is_empty(try_stmt: &TryStatement<'_>) -> bool {
    // Try block must be empty
    if !try_stmt.block.body.is_empty() {
        return false;
    }
    // If there's a catch handler, its body must be empty
    if let Some(ref handler) = try_stmt.handler {
        if !handler.body.body.is_empty() {
            return false;
        }
    }
    // If there's a finally block, it must be empty
    if let Some(ref finalizer) = try_stmt.finalizer {
        if !finalizer.body.is_empty() {
            return false;
        }
    }
    true
}

/// Recursively descend into nested function/arrow bodies within a statement
/// to apply if(false) elimination and other DCE.
fn apply_dce_recursive_into_stmt<'a>(
    stmt: &mut Statement<'a>,
    alloc: &'a oxc::allocator::Allocator,
) -> bool {
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            apply_dce_recursive_into_expr(&mut expr_stmt.expression, alloc)
        }
        Statement::ReturnStatement(ret) => {
            if let Some(ref mut expr) = ret.argument {
                apply_dce_recursive_into_expr(expr, alloc)
            } else {
                false
            }
        }
        Statement::VariableDeclaration(decl) => {
            let mut changed = false;
            for d in decl.declarations.iter_mut() {
                if let Some(ref mut init) = d.init {
                    changed |= apply_dce_recursive_into_expr(init, alloc);
                }
            }
            changed
        }
        _ => false,
    }
}

/// Recursively descend into expressions to find nested arrow/function bodies
/// and apply DCE (especially if(false) elimination).
fn apply_dce_recursive_into_expr<'a>(
    expr: &mut Expression<'a>,
    alloc: &'a oxc::allocator::Allocator,
) -> bool {
    match expr {
        Expression::ArrowFunctionExpression(arrow) => {
            apply_dce_to_statements(&mut arrow.body.statements, None, alloc)
        }
        Expression::FunctionExpression(func) => {
            if let Some(ref mut body) = func.body {
                apply_dce_to_statements(&mut body.statements, None, alloc)
            } else {
                false
            }
        }
        Expression::CallExpression(call) => {
            let mut changed = false;
            for arg in call.arguments.iter_mut() {
                match arg {
                    Argument::SpreadElement(spread) => {
                        changed |= apply_dce_recursive_into_expr(&mut spread.argument, alloc);
                    }
                    _ => {
                        changed |= apply_dce_recursive_into_expr(arg.to_expression_mut(), alloc);
                    }
                }
            }
            changed
        }
        Expression::SequenceExpression(seq) => {
            let mut changed = false;
            for e in seq.expressions.iter_mut() {
                changed |= apply_dce_recursive_into_expr(e, alloc);
            }
            changed
        }
        Expression::ConditionalExpression(cond) => {
            let mut changed = false;
            changed |= apply_dce_recursive_into_expr(&mut cond.consequent, alloc);
            changed |= apply_dce_recursive_into_expr(&mut cond.alternate, alloc);
            changed
        }
        _ => false,
    }
}

/// Collect all referenced identifier names in a list of statements.
/// This scans all IdentifierReference nodes (not binding sites).
fn collect_all_references_in_stmts(stmts: &oxc::allocator::Vec<'_, Statement<'_>>) -> HashSet<String> {
    let mut refs = HashSet::new();
    for stmt in stmts.iter() {
        collect_refs_in_stmt(stmt, &mut refs);
    }
    refs
}

fn collect_refs_in_stmt(stmt: &Statement<'_>, refs: &mut HashSet<String>) {
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            collect_refs_in_expr(&expr_stmt.expression, refs);
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(ref init) = d.init {
                    collect_refs_in_expr(init, refs);
                }
                // Also collect refs from destructuring patterns' defaults
                collect_refs_in_binding_pattern(&d.id, refs);
            }
        }
        Statement::ReturnStatement(ret) => {
            if let Some(ref expr) = ret.argument {
                collect_refs_in_expr(expr, refs);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_refs_in_expr(&if_stmt.test, refs);
            collect_refs_in_stmt(&if_stmt.consequent, refs);
            if let Some(ref alt) = if_stmt.alternate {
                collect_refs_in_stmt(alt, refs);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_refs_in_stmt(s, refs);
            }
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(ref init) = for_stmt.init {
                match init {
                    ForStatementInit::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            if let Some(ref init_expr) = d.init {
                                collect_refs_in_expr(init_expr, refs);
                            }
                        }
                    }
                    _ => {
                        collect_refs_in_expr(init.to_expression(), refs);
                    }
                }
            }
            if let Some(ref test) = for_stmt.test {
                collect_refs_in_expr(test, refs);
            }
            if let Some(ref update) = for_stmt.update {
                collect_refs_in_expr(update, refs);
            }
            collect_refs_in_stmt(&for_stmt.body, refs);
        }
        Statement::ForInStatement(for_in) => {
            collect_refs_in_expr(&for_in.right, refs);
            collect_refs_in_stmt(&for_in.body, refs);
        }
        Statement::ForOfStatement(for_of) => {
            collect_refs_in_expr(&for_of.right, refs);
            collect_refs_in_stmt(&for_of.body, refs);
        }
        Statement::WhileStatement(while_stmt) => {
            collect_refs_in_expr(&while_stmt.test, refs);
            collect_refs_in_stmt(&while_stmt.body, refs);
        }
        Statement::DoWhileStatement(do_while) => {
            collect_refs_in_stmt(&do_while.body, refs);
            collect_refs_in_expr(&do_while.test, refs);
        }
        Statement::FunctionDeclaration(func) => {
            // Collect refs from function body
            if let Some(ref body) = func.body {
                for s in &body.statements {
                    collect_refs_in_stmt(s, refs);
                }
            }
        }
        Statement::ClassDeclaration(class) => {
            collect_refs_in_class(class, refs);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_refs_in_stmt(s, refs);
            }
            if let Some(ref handler) = try_stmt.handler {
                for s in &handler.body.body {
                    collect_refs_in_stmt(s, refs);
                }
            }
            if let Some(ref finalizer) = try_stmt.finalizer {
                for s in &finalizer.body {
                    collect_refs_in_stmt(s, refs);
                }
            }
        }
        Statement::ThrowStatement(throw_stmt) => {
            collect_refs_in_expr(&throw_stmt.argument, refs);
        }
        Statement::SwitchStatement(switch_stmt) => {
            collect_refs_in_expr(&switch_stmt.discriminant, refs);
            for case in &switch_stmt.cases {
                if let Some(ref test) = case.test {
                    collect_refs_in_expr(test, refs);
                }
                for s in &case.consequent {
                    collect_refs_in_stmt(s, refs);
                }
            }
        }
        Statement::LabeledStatement(labeled) => {
            collect_refs_in_stmt(&labeled.body, refs);
        }
        _ => {}
    }
}

fn collect_refs_in_expr(expr: &Expression<'_>, refs: &mut HashSet<String>) {
    match expr {
        Expression::Identifier(id) => {
            refs.insert(id.name.to_string());
        }
        Expression::CallExpression(call) => {
            collect_refs_in_expr(&call.callee, refs);
            for arg in &call.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        collect_refs_in_expr(&spread.argument, refs);
                    }
                    _ => {
                        collect_refs_in_expr(arg.to_expression(), refs);
                    }
                }
            }
        }
        Expression::StaticMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
        }
        Expression::ComputedMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
            collect_refs_in_expr(&member.expression, refs);
        }
        Expression::BinaryExpression(bin) => {
            collect_refs_in_expr(&bin.left, refs);
            collect_refs_in_expr(&bin.right, refs);
        }
        Expression::LogicalExpression(log) => {
            collect_refs_in_expr(&log.left, refs);
            collect_refs_in_expr(&log.right, refs);
        }
        Expression::UnaryExpression(unary) => {
            collect_refs_in_expr(&unary.argument, refs);
        }
        Expression::UpdateExpression(update) => {
            collect_refs_in_simple_assignment_target(&update.argument, refs);
        }
        Expression::ConditionalExpression(cond) => {
            collect_refs_in_expr(&cond.test, refs);
            collect_refs_in_expr(&cond.consequent, refs);
            collect_refs_in_expr(&cond.alternate, refs);
        }
        Expression::AssignmentExpression(assign) => {
            collect_refs_in_assignment_target(&assign.left, refs);
            collect_refs_in_expr(&assign.right, refs);
        }
        Expression::SequenceExpression(seq) => {
            for e in &seq.expressions {
                collect_refs_in_expr(e, refs);
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        if p.computed {
                            collect_refs_in_expr(&p.key.to_expression(), refs);
                        }
                        collect_refs_in_expr(&p.value, refs);
                    }
                    ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_refs_in_expr(&spread.argument, refs);
                    }
                }
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        collect_refs_in_expr(&spread.argument, refs);
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        collect_refs_in_expr(elem.to_expression(), refs);
                    }
                }
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            for s in &arrow.body.statements {
                collect_refs_in_stmt(s, refs);
            }
        }
        Expression::FunctionExpression(func) => {
            if let Some(ref body) = func.body {
                for s in &body.statements {
                    collect_refs_in_stmt(s, refs);
                }
            }
        }
        Expression::TemplateLiteral(tmpl) => {
            for e in &tmpl.expressions {
                collect_refs_in_expr(e, refs);
            }
        }
        Expression::TaggedTemplateExpression(tagged) => {
            collect_refs_in_expr(&tagged.tag, refs);
            for e in &tagged.quasi.expressions {
                collect_refs_in_expr(e, refs);
            }
        }
        Expression::NewExpression(new_expr) => {
            collect_refs_in_expr(&new_expr.callee, refs);
            for arg in &new_expr.arguments {
                match arg {
                    Argument::SpreadElement(spread) => {
                        collect_refs_in_expr(&spread.argument, refs);
                    }
                    _ => {
                        collect_refs_in_expr(arg.to_expression(), refs);
                    }
                }
            }
        }
        Expression::AwaitExpression(await_expr) => {
            collect_refs_in_expr(&await_expr.argument, refs);
        }
        Expression::YieldExpression(yield_expr) => {
            if let Some(ref arg) = yield_expr.argument {
                collect_refs_in_expr(arg, refs);
            }
        }
        Expression::ParenthesizedExpression(paren) => {
            collect_refs_in_expr(&paren.expression, refs);
        }
        Expression::ClassExpression(class) => {
            collect_refs_in_class(class, refs);
        }
        Expression::ChainExpression(chain) => {
            match &chain.expression {
                ChainElement::CallExpression(call) => {
                    collect_refs_in_expr(&call.callee, refs);
                    for arg in &call.arguments {
                        match arg {
                            Argument::SpreadElement(spread) => {
                                collect_refs_in_expr(&spread.argument, refs);
                            }
                            _ => {
                                collect_refs_in_expr(arg.to_expression(), refs);
                            }
                        }
                    }
                }
                ChainElement::StaticMemberExpression(member) => {
                    collect_refs_in_expr(&member.object, refs);
                }
                ChainElement::ComputedMemberExpression(member) => {
                    collect_refs_in_expr(&member.object, refs);
                    collect_refs_in_expr(&member.expression, refs);
                }
                ChainElement::PrivateFieldExpression(pfe) => {
                    collect_refs_in_expr(&pfe.object, refs);
                }
                _ => {}
            }
        }
        // JSX expressions -- walk into children and attributes
        Expression::JSXElement(jsx) => {
            collect_refs_in_jsx_element(jsx, refs);
        }
        Expression::JSXFragment(frag) => {
            for child in &frag.children {
                collect_refs_in_jsx_child(child, refs);
            }
        }
        _ => {}
    }
}

fn collect_refs_in_jsx_element(jsx: &JSXElement<'_>, refs: &mut HashSet<String>) {
    // Collect refs from opening element tag name and attributes
    if let JSXElementName::Identifier(id) = &jsx.opening_element.name {
        // Only add if starts with uppercase (component reference)
        if id.name.as_str().chars().next().map_or(false, |c| c.is_uppercase()) {
            refs.insert(id.name.to_string());
        }
    }
    if let JSXElementName::IdentifierReference(id) = &jsx.opening_element.name {
        refs.insert(id.name.to_string());
    }
    for attr in &jsx.opening_element.attributes {
        match attr {
            JSXAttributeItem::Attribute(a) => {
                if let Some(ref val) = a.value {
                    match val {
                        JSXAttributeValue::ExpressionContainer(container) => {
                            if let Some(expr) = container.expression.as_expression() {
                                collect_refs_in_expr(expr, refs);
                            }
                        }
                        JSXAttributeValue::Element(el) => {
                            collect_refs_in_jsx_element(el, refs);
                        }
                        JSXAttributeValue::Fragment(frag) => {
                            for child in &frag.children {
                                collect_refs_in_jsx_child(child, refs);
                            }
                        }
                        _ => {}
                    }
                }
            }
            JSXAttributeItem::SpreadAttribute(spread) => {
                collect_refs_in_expr(&spread.argument, refs);
            }
        }
    }
    // Children
    for child in &jsx.children {
        collect_refs_in_jsx_child(child, refs);
    }
}

fn collect_refs_in_jsx_child(child: &JSXChild<'_>, refs: &mut HashSet<String>) {
    match child {
        JSXChild::ExpressionContainer(container) => {
            if let Some(expr) = container.expression.as_expression() {
                collect_refs_in_expr(expr, refs);
            }
        }
        JSXChild::Element(el) => {
            collect_refs_in_jsx_element(el, refs);
        }
        JSXChild::Fragment(frag) => {
            for c in &frag.children {
                collect_refs_in_jsx_child(c, refs);
            }
        }
        JSXChild::Spread(spread) => {
            collect_refs_in_expr(&spread.expression, refs);
        }
        _ => {} // Text, empty
    }
}

fn collect_refs_in_class(class: &Class<'_>, refs: &mut HashSet<String>) {
    if let Some(ref super_class) = class.super_class {
        collect_refs_in_expr(super_class, refs);
    }
    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                if method.computed {
                    collect_refs_in_expr(&method.key.to_expression(), refs);
                }
                if let Some(ref body) = method.value.body {
                    for s in &body.statements {
                        collect_refs_in_stmt(s, refs);
                    }
                }
            }
            ClassElement::PropertyDefinition(prop) => {
                if prop.computed {
                    collect_refs_in_expr(&prop.key.to_expression(), refs);
                }
                if let Some(ref val) = prop.value {
                    collect_refs_in_expr(val, refs);
                }
            }
            ClassElement::StaticBlock(block) => {
                for s in &block.body {
                    collect_refs_in_stmt(s, refs);
                }
            }
            ClassElement::AccessorProperty(prop) => {
                if prop.computed {
                    collect_refs_in_expr(&prop.key.to_expression(), refs);
                }
                if let Some(ref val) = prop.value {
                    collect_refs_in_expr(val, refs);
                }
            }
            ClassElement::TSIndexSignature(_) => {}
        }
    }
}

fn collect_refs_in_assignment_target(target: &AssignmentTarget<'_>, refs: &mut HashSet<String>) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            refs.insert(id.name.to_string());
        }
        AssignmentTarget::StaticMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
            collect_refs_in_expr(&member.expression, refs);
        }
        _ => {}
    }
}

fn collect_refs_in_simple_assignment_target(target: &SimpleAssignmentTarget<'_>, refs: &mut HashSet<String>) {
    match target {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
            refs.insert(id.name.to_string());
        }
        SimpleAssignmentTarget::StaticMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
        }
        SimpleAssignmentTarget::ComputedMemberExpression(member) => {
            collect_refs_in_expr(&member.object, refs);
            collect_refs_in_expr(&member.expression, refs);
        }
        SimpleAssignmentTarget::PrivateFieldExpression(pfe) => {
            collect_refs_in_expr(&pfe.object, refs);
        }
        _ => {}
    }
}

fn collect_refs_in_binding_pattern(kind: &BindingPattern<'_>, refs: &mut HashSet<String>) {
    match kind {
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                if prop.computed {
                    collect_refs_in_expr(&prop.key.to_expression(), refs);
                }
                collect_refs_in_binding_pattern(&prop.value, refs);
            }
            if let Some(ref rest) = obj.rest {
                collect_refs_in_binding_pattern(&rest.argument, refs);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_refs_in_binding_pattern(elem, refs);
            }
            if let Some(ref rest) = arr.rest {
                collect_refs_in_binding_pattern(&rest.argument, refs);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_refs_in_binding_pattern(&assign.left, refs);
            collect_refs_in_expr(&assign.right, refs);
        }
        BindingPattern::BindingIdentifier(_) => {}
    }
}

/// Extract the string representation of a simple literal expression.
/// Returns `Some(string)` for numeric, string, and boolean literals.
/// Returns `None` for complex expressions (calls, member access, etc.).
///
/// SWC inlines these const-literal values into segment bodies instead of
/// capturing them, so we need to know their string representations.
fn get_literal_string(expr: &Expression<'_>) -> Option<String> {
    match expr {
        Expression::NumericLiteral(lit) => {
            // Use raw value if available (preserves original formatting),
            // otherwise format the f64 value.
            if let Some(raw) = &lit.raw {
                Some(raw.to_string())
            } else {
                // Format: integers as integers, floats as floats
                let v = lit.value;
                if v == (v as i64) as f64 && v.abs() < (i64::MAX as f64) {
                    Some(format!("{}", v as i64))
                } else {
                    Some(format!("{}", v))
                }
            }
        }
        Expression::StringLiteral(lit) => {
            // Include quotes for string literals
            Some(format!("\"{}\"", lit.value))
        }
        Expression::BooleanLiteral(lit) => {
            Some(if lit.value { "true" } else { "false" }.to_string())
        }
        _ => None,
    }
}

/// Compute a diagnostic highlight SourceLocation from OXC 0-based byte offsets.
///
/// Converts OXC 0-based spans to SWC-compatible SourceLocation format:
/// - lo/hi: 1-based byte offsets (OXC offset + 1)
/// - startLine/endLine: 1-based line numbers
/// - startCol: 1-based column (0-based col + 1)
/// - endCol: 0-based column at the exclusive end position (SWC's exclusive hi
///   cancels with the 1-based adjustment, per SWC's SourceLocation::from)
fn compute_highlight_from_span(source: &str, lo: u32, hi: u32) -> crate::types::SourceLocation {
    let (start_line, start_col_0) = byte_offset_to_line_col(source, lo as usize);
    let (end_line, end_col_0) = byte_offset_to_line_col(source, hi as usize);
    crate::types::SourceLocation {
        lo: lo + 1,
        hi: hi + 1,
        start_line,
        start_col: start_col_0 + 1,
        end_line,
        end_col: end_col_0,
    }
}

/// Convert a 0-based byte offset in source to (1-based line, 0-based column).
fn byte_offset_to_line_col(source: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}
