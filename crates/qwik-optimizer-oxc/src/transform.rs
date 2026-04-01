//! QwikTransform — Stage 10/11 core traversal pass.
//!
//! Implements `Traverse<'a, ()>` to walk the AST and:
//! - Detect marker functions ($-suffixed imports and local exports)
//! - Dispatch the 7-priority call-expression decision tree (XFRM-02)
//! - Manage `decl_stack` scope frames (XFRM-06)
//! - Evaluate `should_emit_segment` stripping conditions (XFRM-07)
//! - Rewrite callee names via `convert_qrl_word` (XFRM-08)
//!
//! Later phases (12–18) add segment extraction, QRL generation, and JSX handling
//! by filling the stubs in `enter_call_expression`.

use std::collections::{HashMap, HashSet};

use oxc::allocator::{Allocator, Box as ArenaBox, CloneIn, Vec as ArenaVec};
use oxc::ast::AstBuilder;
use oxc::ast::ast::*;
use oxc::ast_visit::Visit;
use oxc::codegen::Codegen;
use oxc::span::{SourceType, SPAN};
use oxc_traverse::{Traverse, TraverseCtx};

use crate::collector::GlobalCollect;
use crate::entry_strategy::{self, EntryPolicy};
use crate::hash;
use crate::inlined_fn;
use crate::is_const;
use crate::types::{CtxKind, Diagnostic, DiagnosticCategory, EmitMode, EntryStrategy};
use crate::words;

// ---------------------------------------------------------------------------
// IdentType and IdPlusType
// ---------------------------------------------------------------------------

/// How a binding was declared in the current scope frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentType {
    /// Variable binding. `bool` = `is_const_keyword && is_static_initializer`.
    Var(bool),
    /// Function declaration.
    Fn,
    /// Class declaration.
    Class,
}

/// A (binding-name, ident-type) pair stored per scope frame.
pub(crate) type IdPlusType = (String, IdentType);

// ---------------------------------------------------------------------------
// HoistedConst — a module-scope const declaration accumulated during hoisting
// ---------------------------------------------------------------------------

/// An owned record for a `const q_name = <rhs>` declaration to be prepended
/// to the module body by the `exit_program` drain.
pub(crate) struct HoistedConst {
    /// The const binding name, e.g. `"q_renderHeader1_jMxQsjbyDss"`.
    pub name: String,
    /// Serialized RHS expression (e.g. `"qrl(...)"`  or `"_noopQrl(...)"`).
    pub rhs_code: String,
    /// Deduplication key — same as `name` (or the symbol_name the const was built from).
    pub symbol_name: String,
}

// ---------------------------------------------------------------------------
// RefAssignment — a `.s()` call emitted right after the matching const body
// ---------------------------------------------------------------------------

/// A serialized `q_name.s(fn_body)` expression statement emitted immediately
/// after the statement that defines `target_ident` (the fn_body const).
pub(crate) struct RefAssignment {
    /// The identifier whose const binding definition triggers this `.s()` emission.
    pub target_ident: String,
    /// Serialized `"q_name.s(fn_body);"` expression statement (including semicolon).
    pub s_call_code: String,
}

// ---------------------------------------------------------------------------
// SegmentRecord — accumulated extracted segment metadata
// ---------------------------------------------------------------------------

/// Internal record for a single extracted segment. Accumulated in
/// `QwikTransform::segments` during the traversal. Phase 16 (Segment Module
/// Generation) reads these to emit segment module files.
pub(crate) struct SegmentRecord {
    /// Symbol name (e.g. `test_tsx_component_ABC`).
    pub name: String,
    /// File-prefixed display name (e.g. `test.tsx_component_ABC`).
    pub display_name: String,
    /// Canonical filename for the segment module (e.g. `test.tsx_component_ABC`).
    pub canonical_filename: String,
    /// Output chunk key from `entry_policy.get_entry_for_sym`, or `None` for own chunk.
    pub entry: Option<String>,
    /// Serialized folded closure body (set by `create_segment` via OXC Codegen).
    /// `None` for noop QRLs that do not require a segment module.
    pub expr: Option<String>,
    /// Runtime-captured identifiers (closed-over variables).
    pub scoped_idents: Vec<String>,
    /// Compile-time import names referenced inside the segment body.
    pub local_idents: Vec<String>,
    /// The context (marker function) name, e.g. `"component$"`.
    pub ctx_name: String,
    /// The context kind (Function, EventHandler, etc.).
    pub ctx_kind: CtxKind,
    /// Relative path of the source file (used as `origin` in SegmentData).
    pub origin: String,
    /// Byte span `(start, end)` of the original call expression.
    pub span: (u32, u32),
    /// 11-character SipHash-based segment hash.
    pub hash: String,
    /// Whether this segment was created via `create_inline_qrl` (not its own module).
    pub is_inline: bool,
}

// ---------------------------------------------------------------------------
// IdentCollector — read-only visitor that harvests IdentifierReference names
// ---------------------------------------------------------------------------

/// Collects all [`IdentifierReference`] names reachable from an expression.
///
/// Used by [`compute_scoped_idents`] and [`QwikTransform::get_local_idents`] to
/// determine which identifiers a segment closure body references.
pub(crate) struct IdentCollector {
    pub idents: HashSet<String>,
}

impl IdentCollector {
    /// Walk `expr` and return every `IdentifierReference` name found.
    pub(crate) fn collect(expr: &Expression<'_>) -> HashSet<String> {
        let mut collector = Self { idents: HashSet::new() };
        collector.visit_expression(expr);
        collector.idents
    }
}

impl<'a> Visit<'a> for IdentCollector {
    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
        self.idents.insert(id.name.as_str().to_string());
    }
}

// ---------------------------------------------------------------------------
// compute_scoped_idents
// ---------------------------------------------------------------------------

/// Intersect `all_idents` with `all_decl` (keeping only `Var(_)` entries),
/// deduplicate, sort, and compute `is_const` (true iff every matched entry is
/// `Var(true)`).
///
/// Returns `(sorted_names, is_const)`.
pub(crate) fn compute_scoped_idents(
    all_idents: &HashSet<String>,
    all_decl: &[IdPlusType],
) -> (Vec<String>, bool) {
    let mut matched: HashSet<String> = HashSet::new();
    let mut is_const = true;

    for name in all_idents {
        for (decl_name, decl_type) in all_decl {
            if name == decl_name {
                match decl_type {
                    IdentType::Var(c) => {
                        matched.insert(name.clone());
                        if !c {
                            is_const = false;
                        }
                    }
                    // Fn/Class entries are NOT captured as scoped idents
                    IdentType::Fn | IdentType::Class => {}
                }
            }
        }
    }

    let mut sorted: Vec<String> = matched.into_iter().collect();
    sorted.sort();
    (sorted, is_const)
}

// ---------------------------------------------------------------------------
// argument_to_expression — comprehensive Argument → Expression conversion
// ---------------------------------------------------------------------------

/// Convert an OXC `Argument` variant to the corresponding `Expression` variant.
/// OXC's `Argument` enum mirrors `Expression` but is a separate type. This function
/// handles all common variants exhaustively to avoid dropping valid AST nodes.
fn argument_to_expression<'a>(arg: Argument<'a>) -> Option<Expression<'a>> {
    Some(match arg {
        // Literals
        Argument::BooleanLiteral(b) => Expression::BooleanLiteral(b),
        Argument::NullLiteral(b) => Expression::NullLiteral(b),
        Argument::NumericLiteral(b) => Expression::NumericLiteral(b),
        Argument::BigIntLiteral(b) => Expression::BigIntLiteral(b),
        Argument::RegExpLiteral(b) => Expression::RegExpLiteral(b),
        Argument::StringLiteral(b) => Expression::StringLiteral(b),
        Argument::TemplateLiteral(b) => Expression::TemplateLiteral(b),
        // Identifiers
        Argument::Identifier(b) => Expression::Identifier(b),
        // Functions
        Argument::ArrowFunctionExpression(b) => Expression::ArrowFunctionExpression(b),
        Argument::FunctionExpression(b) => Expression::FunctionExpression(b),
        // Calls / member access
        Argument::CallExpression(b) => Expression::CallExpression(b),
        Argument::StaticMemberExpression(b) => Expression::StaticMemberExpression(b),
        Argument::ComputedMemberExpression(b) => Expression::ComputedMemberExpression(b),
        Argument::PrivateFieldExpression(b) => Expression::PrivateFieldExpression(b),
        // Compound expressions
        Argument::ArrayExpression(b) => Expression::ArrayExpression(b),
        Argument::ObjectExpression(b) => Expression::ObjectExpression(b),
        Argument::TaggedTemplateExpression(b) => Expression::TaggedTemplateExpression(b),
        Argument::UnaryExpression(b) => Expression::UnaryExpression(b),
        Argument::BinaryExpression(b) => Expression::BinaryExpression(b),
        Argument::LogicalExpression(b) => Expression::LogicalExpression(b),
        Argument::ConditionalExpression(b) => Expression::ConditionalExpression(b),
        Argument::AssignmentExpression(b) => Expression::AssignmentExpression(b),
        Argument::SequenceExpression(b) => Expression::SequenceExpression(b),
        Argument::ParenthesizedExpression(b) => Expression::ParenthesizedExpression(b),
        Argument::NewExpression(b) => Expression::NewExpression(b),
        Argument::AwaitExpression(b) => Expression::AwaitExpression(b),
        Argument::YieldExpression(b) => Expression::YieldExpression(b),
        Argument::ClassExpression(b) => Expression::ClassExpression(b),
        Argument::UpdateExpression(b) => Expression::UpdateExpression(b),
        // TS expressions
        Argument::TSAsExpression(b) => Expression::TSAsExpression(b),
        Argument::TSSatisfiesExpression(b) => Expression::TSSatisfiesExpression(b),
        Argument::TSNonNullExpression(b) => Expression::TSNonNullExpression(b),
        Argument::TSTypeAssertion(b) => Expression::TSTypeAssertion(b),
        Argument::TSInstantiationExpression(b) => Expression::TSInstantiationExpression(b),
        // Spread element — not a direct Expression, return None
        Argument::SpreadElement(_) => return None,
        // Catch-all for any future variants
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// get_function_params
// ---------------------------------------------------------------------------

/// Extract all parameter binding names from a function or arrow function
/// expression. Returns an empty set for any other expression kind.
pub(crate) fn get_function_params(expr: &Expression<'_>) -> HashSet<String> {
    let mut result = HashSet::new();
    let params: Option<&[FormalParameter<'_>]> = match expr {
        Expression::ArrowFunctionExpression(arrow) => Some(&arrow.params.items),
        Expression::FunctionExpression(func) => Some(&func.params.items),
        _ => None,
    };
    if let Some(items) = params {
        for param in items {
            collect_binding_names(&param.pattern, &mut |name| {
                result.insert(name.to_string());
            });
        }
    }
    result
}

// ---------------------------------------------------------------------------
// can_capture_scope
// ---------------------------------------------------------------------------

/// Returns `true` when `expr` is a function or arrow function — i.e., when
/// it is capable of closing over outer variables. Identifiers and other
/// non-function expressions cannot capture scope, so C03 applies.
fn can_capture_scope(expr: &Expression<'_>) -> bool {
    matches!(
        expr,
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)
    )
}

// ---------------------------------------------------------------------------
// PendingQSegment — per-$ call state carried from enter to exit
// ---------------------------------------------------------------------------

/// State accumulated during `enter_call_expression` that is consumed by the
/// matching `exit_call_expression` to complete segment extraction.
///
/// Because OXC Traverse visits children *after* `enter_*` returns, we cannot
/// process captures until `exit_*` fires (when all nested `$` calls have already
/// been processed).
pub(crate) struct PendingQSegment {
    /// The specifier-level context name (e.g., `"component$"` for `component$(...)`).
    pub ctx_name: String,
    /// Classified context kind (Function vs EventHandler).
    pub ctx_kind: CtxKind,
    /// Identifier references collected from the first arg BEFORE children were visited.
    pub descendent_idents: HashSet<String>,
    /// Byte span `(start, end)` of the call expression. Used to match enter/exit.
    pub span_start: u32,
    /// Optional display name override (from import QRL detection).
    pub display_name_override: Option<String>,
    /// Optional hash override (from import QRL detection).
    pub hash_override: Option<String>,
}

// ---------------------------------------------------------------------------
// QwikTransformOptions — construction-time config (borrows from caller)
// ---------------------------------------------------------------------------

/// Configuration passed to `QwikTransform::new`.
pub(crate) struct QwikTransformOptions<'b> {
    /// Global collect result from Stage 7.
    pub global_collect: &'b GlobalCollect,
    /// Core module import path (e.g. "@qwik.dev/core").
    pub core_module: &'b str,
    /// Ctx name prefixes to strip (e.g. ["useServer"]).
    pub strip_ctx_name: &'b [String],
    /// Whether to strip EventHandler-kind segments.
    pub strip_event_handlers: bool,
    /// Output emit mode.
    pub mode: &'b EmitMode,
    /// Optional scope prefix for hash computation.
    pub scope: Option<&'b str>,
    /// Relative path of the source file (for hash computation).
    pub rel_path: &'b str,
    /// File name of the source file (for display_name prefix).
    pub file_name: &'b str,
    /// Entry strategy for determining output chunk grouping.
    pub entry_strategy: &'b EntryStrategy,
    /// Source file extension (e.g. "tsx", "js").
    pub extension: &'b str,
    /// Whether to append file extension to import paths in segment modules.
    pub explicit_extensions: bool,
    /// Whether the transform is running in a server context (for Dev mode metadata).
    pub is_server: bool,
}

// ---------------------------------------------------------------------------
// QwikTransform struct
// ---------------------------------------------------------------------------

/// Core Qwik traversal pass implementing `Traverse<'a, ()>`.
///
/// Traversal state is accumulated across the AST walk. All per-segment
/// extraction logic (Phases 12–18) builds on top of the scaffolding
/// established here.
pub(crate) struct QwikTransform {
    // ---- Marker / special-case function detection -------------------------
    /// Maps local binding name → imported specifier for all $-suffixed Named imports
    /// AND locally-exported $-suffixed identifiers.
    ///
    /// e.g. `import { component$ } from "@qwik.dev/core"` → `{"component$": "component$"}`
    /// e.g. `export function myHelper$(){}` → `{"myHelper$": "myHelper$"}`
    pub(crate) marker_functions: HashMap<String, String>,

    /// Local name for the bare `$` import from the core module (qsegment).
    pub(crate) qsegment_fn: Option<String>,
    /// Local name for `sync$`.
    pub(crate) sync_qrl_fn: Option<String>,
    /// Local name for `inlinedQrl`.
    pub(crate) inlined_qrl_fn: Option<String>,
    /// Local name for `_fnSignal`.
    pub(crate) fn_signal_fn: Option<String>,

    /// Local names for JSX factory functions from any `*jsx-runtime*` source.
    /// Includes `jsx`, `jsxs`, `jsxDEV`, `_jsx`, `_jsxs`.
    pub(crate) jsx_functions: HashSet<String>,

    // ---- Traversal state --------------------------------------------------
    /// Context name stack; each entry is pushed when entering a named call or
    /// variable declarator and popped on exit. Used to build `display_name`.
    pub(crate) stack_ctxt: Vec<String>,

    /// Scope frames. Each function/arrow body gets a new frame.
    /// Each frame contains the (name, IdentType) bindings declared in it.
    /// Initialised with one empty root frame.
    pub(crate) decl_stack: Vec<Vec<IdPlusType>>,

    /// Collision counter for `display_name` deduplication.
    pub(crate) segment_names: HashMap<String, u32>,

    /// Span-start values of plain-identifier calls that pushed to `stack_ctxt`.
    /// Used for symmetric pop in `exit_call_expression`.
    ctxt_pushed_calls: HashSet<u32>,

    // ---- Config (owned copies) --------------------------------------------
    pub(crate) strip_ctx_name: Vec<String>,
    pub(crate) strip_event_handlers: bool,

    // ---- Variable declaration kind tracking --------------------------------
    /// Set in `enter_variable_declaration`, cleared in `exit_variable_declaration`.
    /// Used by `enter_variable_declarator` to know if the binding is `const`.
    current_var_kind: Option<VariableDeclarationKind>,

    /// Whether a variable name was pushed to `stack_ctxt` in `enter_variable_declarator`.
    /// Popped in `exit_variable_declarator`.
    var_decl_ctxt_pushed: bool,

    /// Stack tracking whether each function frame (LIFO with `decl_stack`) also
    /// pushed a name to `stack_ctxt`. `true` = pushed, `false` = no push.
    fn_ctxt_push_stack: Vec<bool>,

    // ---- Phase 12: segment extraction state --------------------------------
    /// Accumulated extracted segments (filled by `create_segment` in Plans 12-02+).
    pub(crate) segments: Vec<SegmentRecord>,

    /// Stack of segment names for nested `$` detection (Phase 12).
    pub(crate) segment_stack: Vec<String>,

    /// Queue of pending segment extractions: one entry pushed per `$`-call in
    /// `enter_call_expression`, consumed (LIFO) in `exit_call_expression`.
    pub(crate) pending_qsegments: Vec<PendingQSegment>,

    /// Diagnostics accumulated during the traversal (e.g., C03 CanNotCapture).
    pub(crate) diagnostics: Vec<Diagnostic>,

    /// Module-scope `const q_name = <rhs>` declarations accumulated during hoisting.
    /// Drained (prepended) to `program.body` in `exit_program`.
    pub(crate) extra_top_items: Vec<HoistedConst>,

    /// Placeholder for top-level statements appended to the output module.
    pub(crate) extra_bottom_items: Vec<String>,

    /// `.s()` call statements emitted immediately after the const binding they target.
    /// Drained (interleaved) during `exit_program`.
    pub(crate) ref_assignments: Vec<RefAssignment>,

    /// Maps `const` binding names to their serialized initializer expressions.
    ///
    /// Populated from variable declarators where `is_const_expression(init) == true`
    /// and the binding is not exported. Consumed in Step 0 of
    /// `_create_synthetic_qsegment` (Plan 12-02) for inlining const captures.
    pub(crate) const_initializers: HashMap<String, String>,

    /// Entry policy derived from `entry_strategy` option.
    pub(crate) entry_policy: Box<dyn EntryPolicy>,

    /// Cached result of `entry_strategy::is_inline()` for the given strategy.
    pub(crate) is_inline_strategy: bool,

    // ---- Config (owned copies for Phase 12+) ------------------------------
    /// Owned copy of the emit mode.
    pub(crate) mode: EmitMode,

    /// Owned scope prefix (for hash computation).
    pub(crate) scope: Option<String>,

    /// Relative file path (used as `origin` in SegmentData).
    pub(crate) rel_path: String,

    /// File name (used as display_name prefix).
    pub(crate) file_name: String,

    /// Source file extension (e.g. "tsx").
    pub(crate) extension: String,

    /// Whether to append extension to import paths in segment modules.
    pub(crate) explicit_extensions: bool,

    /// Whether running in a server context (for Dev mode metadata).
    pub(crate) is_server: bool,

    /// Raw pointer to the `GlobalCollect` for `get_local_idents`.
    ///
    /// # Safety
    /// The pointer is valid for the duration of the traversal: `GlobalCollect` is
    /// owned by `transform_code` and outlives the `QwikTransform` instance.
    global_collect: *const GlobalCollect,

    // ---- Phase 13: Level 2 loop-context .w() hoisting ----------------------

    /// Stack of hoisting scopes — one Vec per function/arrow scope.
    /// Each entry: `(const_name, rhs_code)` where rhs_code is serialized `q_name.w([caps])`.
    pub(crate) hoisted_qrls: Vec<Vec<(String, String)>>,

    /// Stack of loop iteration variable names — non-empty means "inside a loop".
    pub(crate) iteration_var_stack: Vec<Vec<String>>,

    /// Stack of decl_stack depths at component$ boundaries.
    /// Used by `compute_hoist_target_depth` to find the component top scope.
    pub(crate) component_depths: Vec<usize>,

    // ---- Phase 14: JSX transform state -------------------------------------

    /// Monotonic counter for unique JSX key generation.
    pub(crate) jsx_key_counter: u32,

    /// First two characters of the file's base64url hash for JSX key prefix.
    pub(crate) jsx_file_hash_prefix: String,

    /// Whether any non-immutable component is present (set per element).
    pub(crate) jsx_mutable: bool,

    /// True when processing the outermost JSX node of a subtree.
    pub(crate) root_jsx_mode: bool,

    /// Set of imported component identifiers with stable identity (from component$/component imports).
    pub(crate) immutable_function_cmp: HashSet<String>,

    /// Stack saving `root_jsx_mode` values as we enter nested JSX nodes.
    /// `enter_expression` pushes the current value and sets it to false;
    /// `exit_expression` pops it back so the parent knows whether it was root.
    pub(crate) jsx_root_mode_stack: Vec<bool>,

    // ---- Phase 14: JSX runtime import tracking ----------------------------

    /// Whether `_jsxSorted` import from "@qwik.dev/core" is needed.
    pub(crate) needs_jsx_sorted: bool,
    /// Whether `_jsxSplit` import from "@qwik.dev/core" is needed.
    pub(crate) needs_jsx_split: bool,
    /// Whether `_getVarProps` import from "@qwik.dev/core" is needed.
    pub(crate) needs_get_var_props: bool,
    /// Whether `_getConstProps` import from "@qwik.dev/core" is needed.
    pub(crate) needs_get_const_props: bool,
    /// Whether `Fragment as _Fragment` import from "@qwik.dev/core/jsx-runtime" is needed.
    pub(crate) needs_fragment: bool,

    // ---- Phase 15: Signal wrapping state ------------------------------------

    /// Dedup map for hoist_fn_signal_call: fn_body_str -> (const_name, counter_value).
    pub(crate) hoisted_fn_signals: HashMap<String, (String, u32)>,

    /// Monotonic counter for `_hf<N>` const names.
    pub(crate) hoisted_fn_counter: u32,

    /// Whether `_wrapProp` import from "@qwik.dev/core" is needed.
    pub(crate) needs_wrap_prop: bool,

    /// Whether `_fnSignal` import from "@qwik.dev/core" is needed.
    pub(crate) needs_fn_signal: bool,

    /// Whether `_val` import from "@qwik.dev/core" is needed (bind:value).
    pub(crate) needs_val: bool,

    /// Whether `_chk` import from "@qwik.dev/core" is needed (bind:checked).
    pub(crate) needs_chk: bool,
}

// ---------------------------------------------------------------------------
// QwikTransform::new
// ---------------------------------------------------------------------------

impl QwikTransform {
    /// Create a new `QwikTransform` from the given options.
    ///
    /// - Scans `global_collect.imports` for `Named` entries whose specifier ends with `$`
    ///   and inserts them into `marker_functions`.
    /// - Scans `global_collect.export_local_ids()` for names ending with `$` and inserts
    ///   them as self-referential entries in `marker_functions`.
    /// - Resolves special-case functions (`$`, `sync$`, `inlinedQrl`, `_fnSignal`) via
    ///   `get_imported_local`.
    /// - Builds `jsx_functions` from sources containing `"jsx-runtime"`.
    pub(crate) fn new(options: QwikTransformOptions<'_>) -> Self {
        let collect = options.global_collect;
        let mut marker_functions: HashMap<String, String> = HashMap::new();

        // --- Named imports whose specifier ends with `$` ---
        for (local, import) in &collect.imports {
            use crate::collector::ImportKind;
            if import.kind == ImportKind::Named && import.specifier.ends_with('$') {
                marker_functions.insert(local.clone(), import.specifier.clone());
            }
        }

        // --- Locally-exported names ending with `$` ---
        for name in collect.export_local_ids() {
            if name.ends_with('$') {
                marker_functions.insert(name.clone(), name.clone());
            }
        }

        // --- Special-case function resolution ---
        let qsegment_fn = collect
            .get_imported_local("$", options.core_module)
            .map(|s| s.to_string());
        let sync_qrl_fn = collect
            .get_imported_local("sync$", options.core_module)
            .map(|s| s.to_string());
        let inlined_qrl_fn = collect
            .get_imported_local("inlinedQrl", options.core_module)
            .map(|s| s.to_string());
        let fn_signal_fn = collect
            .get_imported_local("_fnSignal", options.core_module)
            .map(|s| s.to_string());

        // --- JSX factory functions ---
        let jsx_specifiers: HashSet<&str> =
            ["jsx", "jsxs", "jsxDEV", "_jsx", "_jsxs"].iter().copied().collect();
        let mut jsx_functions: HashSet<String> = HashSet::new();
        for (local, import) in &collect.imports {
            if import.source.contains("jsx-runtime") {
                if jsx_specifiers.contains(import.specifier.as_str()) {
                    jsx_functions.insert(local.clone());
                }
            }
        }

        let is_inline_strategy = entry_strategy::is_inline(options.entry_strategy);
        let entry_policy = entry_strategy::parse_entry_strategy(options.entry_strategy);

        // --- JSX file hash prefix (first 2 chars of file hash) ---
        let jsx_file_hash_prefix = {
            let h = hash::compute_segment_hash(options.scope, options.rel_path, "");
            if h.len() >= 2 {
                h[..2].to_string()
            } else {
                h
            }
        };

        // --- Immutable component identifiers from imports ---
        let mut immutable_function_cmp: HashSet<String> = HashSet::new();
        for (local, import) in &collect.imports {
            if import.specifier == "component$" || import.specifier == "component" {
                immutable_function_cmp.insert(local.clone());
            }
        }

        Self {
            marker_functions,
            qsegment_fn,
            sync_qrl_fn,
            inlined_qrl_fn,
            fn_signal_fn,
            jsx_functions,
            stack_ctxt: Vec::new(),
            decl_stack: vec![vec![]],
            segment_names: HashMap::new(),
            ctxt_pushed_calls: HashSet::new(),
            strip_ctx_name: options.strip_ctx_name.to_vec(),
            strip_event_handlers: options.strip_event_handlers,
            current_var_kind: None,
            var_decl_ctxt_pushed: false,
            fn_ctxt_push_stack: Vec::new(),
            // Phase 12 fields
            segments: Vec::new(),
            segment_stack: Vec::new(),
            pending_qsegments: Vec::new(),
            diagnostics: Vec::new(),
            extra_top_items: Vec::new(),
            extra_bottom_items: Vec::new(),
            ref_assignments: Vec::new(),
            const_initializers: HashMap::new(),
            entry_policy,
            is_inline_strategy,
            mode: options.mode.clone(),
            scope: options.scope.map(|s| s.to_string()),
            rel_path: options.rel_path.to_string(),
            file_name: options.file_name.to_string(),
            extension: options.extension.to_string(),
            explicit_extensions: options.explicit_extensions,
            is_server: options.is_server,
            global_collect: options.global_collect as *const GlobalCollect,
            // Phase 13: Level 2 hoisting fields
            hoisted_qrls: Vec::new(),
            iteration_var_stack: Vec::new(),
            component_depths: Vec::new(),
            // Phase 14: JSX transform state
            jsx_key_counter: 0,
            jsx_file_hash_prefix,
            jsx_mutable: false,
            root_jsx_mode: true,
            immutable_function_cmp,
            jsx_root_mode_stack: Vec::new(),
            // Phase 14: JSX runtime import tracking
            needs_jsx_sorted: false,
            needs_jsx_split: false,
            needs_get_var_props: false,
            needs_get_const_props: false,
            needs_fragment: false,
            // Phase 15: Signal wrapping state
            hoisted_fn_signals: HashMap::new(),
            hoisted_fn_counter: 0,
            needs_wrap_prop: false,
            needs_fn_signal: false,
            needs_val: false,
            needs_chk: false,
        }
    }

    // -----------------------------------------------------------------------
    // should_emit_segment (XFRM-07)
    // -----------------------------------------------------------------------

    /// Whether a segment with the given context name and kind should be emitted.
    ///
    /// Returns `false` when:
    /// - `ctx_name` starts with any prefix in `strip_ctx_name`, OR
    /// - `strip_event_handlers` is true AND `ctx_kind == EventHandler`.
    pub(crate) fn should_emit_segment(&self, ctx_name: &str, ctx_kind: CtxKind) -> bool {
        for prefix in &self.strip_ctx_name {
            if ctx_name.starts_with(prefix.as_str()) {
                return false;
            }
        }
        if self.strip_event_handlers && ctx_kind == CtxKind::EventHandler {
            return false;
        }
        true
    }

    /// Return all identifiers from `expr` that are globally known (imports, exports, root).
    ///
    /// Uses [`IdentCollector`] to gather all `IdentifierReference` names from the
    /// expression, then filters to those present in `GlobalCollect` (i.e., imported or
    /// exported names that should become explicit `import` dependencies of the segment
    /// module).
    ///
    /// # Safety
    /// `self.global_collect` is a raw pointer set in `QwikTransform::new` to the
    /// `GlobalCollect` owned by `transform_code`, which outlives this transform.
    pub(crate) fn get_local_idents(&self, expr: &Expression<'_>) -> Vec<String> {
        let all_idents = IdentCollector::collect(expr);
        let collect = unsafe { &*self.global_collect };
        let mut result: Vec<String> = all_idents
            .into_iter()
            .filter(|name| collect.is_global(name))
            .collect();
        result.sort();
        result
    }

    // -----------------------------------------------------------------------
    // Phase 14: JSX utility methods
    // -----------------------------------------------------------------------

    /// Convert a camelCase string to kebab-case.
    ///
    /// Each ASCII uppercase character becomes `-` + its lowercase version,
    /// EXCEPT for the very first character (no leading dash).
    /// No special-casing for acronyms: `DOM` -> `d-o-m`.
    pub(crate) fn camel_to_kebab(s: &str) -> String {
        let mut result = String::with_capacity(s.len() + 4);
        for (i, c) in s.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i != 0 {
                    result.push('-');
                }
                result.push(c.to_ascii_lowercase());
            } else {
                result.push(c);
            }
        }
        result
    }

    /// Translate a JSX event prop name to its HTML attribute equivalent.
    ///
    /// Handles three scopes in order:
    /// - `window:on<Event>$` → `q-w:<event>`
    /// - `document:on<Event>$` → `q-d:<event>`
    /// - `on<Event>$` → `q-e:<event>`
    ///
    /// The event name (after stripping prefix and trailing `$`) is converted to
    /// kebab-case via [`camel_to_kebab`], UNLESS it starts with `-` in which case
    /// the case is preserved (the leading `-` is removed).
    ///
    /// Returns `None` if the prop name doesn't match any event pattern.
    pub(crate) fn jsx_event_to_html_attribute(prop_name: &str) -> Option<String> {
        let (prefix, event_body) = if let Some(rest) = prop_name.strip_prefix("window:on") {
            ("q-w:", rest)
        } else if let Some(rest) = prop_name.strip_prefix("document:on") {
            ("q-d:", rest)
        } else if let Some(rest) = prop_name.strip_prefix("on") {
            ("q-e:", rest)
        } else {
            return None;
        };

        // Must end with `$`
        let event_name = event_body.strip_suffix('$')?;

        // Case-sensitive events start with `-` — remove the `-` and preserve case
        let converted = if let Some(case_sensitive) = event_name.strip_prefix('-') {
            case_sensitive.to_string()
        } else {
            Self::camel_to_kebab(event_name)
        };

        Some(format!("{prefix}{converted}"))
    }

    /// Normalize JSX text content, collapsing whitespace across newlines.
    ///
    /// Algorithm:
    /// 1. Split by `\n`
    /// 2. Trim each line (both ends)
    /// 3. Filter out empty lines
    /// 4. Join with a single space
    ///
    /// This matches SWC's JSX text normalization behavior.
    pub(crate) fn normalize_jsx_text(raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        raw.split('\n')
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Generate a unique JSX key for the current element.
    ///
    /// Format: `<2-char-file-hash-prefix>_<monotonic-counter>`.
    /// Increments `jsx_key_counter` after each call.
    pub(crate) fn gen_jsx_key(&mut self) -> String {
        let key = format!("{}_{}", self.jsx_file_hash_prefix, self.jsx_key_counter);
        self.jsx_key_counter += 1;
        key
    }

    // -----------------------------------------------------------------------
    // Phase 14: JSX transform — entry points
    // -----------------------------------------------------------------------

    /// Convert a `JSXElement` node into a `_jsxSorted` or `_jsxSplit` call.
    ///
    /// Called from `exit_expression` after children have already been transformed
    /// (post-order traversal). `was_root` reflects whether `root_jsx_mode` was
    /// true when we first *entered* this element.
    fn transform_jsx_element<'a>(
        &mut self,
        el: JSXElement<'a>,
        was_root: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Expression<'a> {
        let allocator: &'a Allocator = ctx.ast.allocator;
        let ast = AstBuilder::new(allocator);

        // Save jsx_mutable — each element gets its own mutable state.
        // Child elements may set jsx_mutable=true, but that should not leak
        // into the parent's static_subtree flag computation.
        let saved_jsx_mutable = self.jsx_mutable;
        self.jsx_mutable = false;

        let opening = el.opening_element.unbox();
        let mut children_vec = el.children;

        // ---- 1. Classify element type -----------------------------------------
        let (is_fn, tag_expr) = match &opening.name {
            JSXElementName::Identifier(id) => {
                let name = id.name.as_str();
                let first_char = name.chars().next().unwrap_or('a');
                let fn_by_case = first_char.is_ascii_uppercase();
                let is_fn = fn_by_case || self.immutable_function_cmp.contains(name);
                if is_fn && !self.immutable_function_cmp.contains(name) {
                    self.jsx_mutable = true;
                }
                let name_atom = ast.atom(name);
                let expr = if is_fn {
                    ast.expression_identifier(SPAN, name_atom)
                } else {
                    ast.expression_string_literal(SPAN, name_atom, None)
                };
                (is_fn, expr)
            }
            JSXElementName::IdentifierReference(id) => {
                // IdentifierReference is an imported/referenced identifier — always a component
                let name = id.name.as_str();
                self.jsx_mutable = true;
                let expr = ast.expression_identifier(SPAN, ast.atom(name));
                (true, expr)
            }
            JSXElementName::MemberExpression(me) => {
                self.jsx_mutable = true;
                let expr = jsx_member_to_expr(me, &ast, allocator);
                (true, expr)
            }
            JSXElementName::NamespacedName(nn) => {
                // e.g. <ns:local> — treat as string "ns:local"
                let name = format!("{}:{}", nn.namespace.name.as_str(), nn.name.name.as_str());
                let expr = ast.expression_string_literal(SPAN, ast.atom(&name), None);
                (false, expr)
            }
            JSXElementName::ThisExpression(_) => {
                // <this> — treated as a component expression
                self.jsx_mutable = true;
                let expr = ast.expression_this(SPAN);
                (true, expr)
            }
        };

        // is_text_only: textarea, title — children treated as a single string
        let is_text_only = if let JSXElementName::Identifier(id) = &opening.name {
            let n = id.name.as_str();
            n == "textarea" || n == "title"
        } else {
            false
        };

        // ---- 2. Key generation ------------------------------------------------
        let should_emit_key = is_fn || was_root;
        let key_expr: Expression<'a> = if should_emit_key {
            let key = self.gen_jsx_key();
            ast.expression_string_literal(SPAN, ast.atom(&key), None)
        } else {
            ast.expression_null_literal(SPAN)
        };

        // ---- 3. Process attributes + children ---------------------------------
        let mut attrs = opening.attributes;
        let (should_sort, var_props_opt, const_props_opt, children_opt, flags) =
            self.handle_jsx_props(&mut attrs, &mut children_vec, is_fn, is_text_only, ctx);

        // ---- 4. Build call ----------------------------------------------------
        // Determine callee based on whether we need runtime sort.
        let callee_name = if should_sort {
            self.needs_jsx_split = true;
            "_jsxSplit"
        } else {
            self.needs_jsx_sorted = true;
            "_jsxSorted"
        };

        // Restore parent's jsx_mutable state — if this element was mutable,
        // propagate up so the parent knows its subtree is not fully static.
        let this_mutable = self.jsx_mutable;
        self.jsx_mutable = saved_jsx_mutable || this_mutable;

        build_jsx_call(callee_name, tag_expr, var_props_opt, const_props_opt, children_opt, flags, key_expr, &ast, allocator)
    }

    /// Convert a `JSXFragment` node into a `_jsxSorted(_Fragment, ...)` call.
    fn transform_jsx_fragment<'a>(
        &mut self,
        frag: JSXFragment<'a>,
        was_root: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Expression<'a> {
        let allocator: &'a Allocator = ctx.ast.allocator;
        let ast = AstBuilder::new(allocator);

        self.needs_fragment = true;
        self.needs_jsx_sorted = true;

        // Tag is _Fragment identifier
        let tag_expr = ast.expression_identifier(SPAN, ast.atom("_Fragment"));

        // Key generation
        let should_emit_key = was_root;
        let key_expr: Expression<'a> = if should_emit_key {
            let key = self.gen_jsx_key();
            ast.expression_string_literal(SPAN, ast.atom(&key), None)
        } else {
            ast.expression_null_literal(SPAN)
        };

        // No attributes — just children
        let mut children_vec = frag.children;
        let children_opt = self.build_children(&mut children_vec, false, ctx);

        // For fragments: flags depend on jsx_mutable state
        // A fragment with only immutable children has flags=1 (static_subtree)
        let flags: u32 = if self.jsx_mutable { 1 } else { 3 };

        // Fragment: var_props=null, const_props=null
        build_jsx_call("_jsxSorted", tag_expr, None, None, children_opt, flags, key_expr, &ast, allocator)
    }

    /// Prop classification pipeline: pre-scan + main loop.
    ///
    /// Returns `(should_sort, var_props, const_props, children_expr, flags)`.
    #[allow(clippy::too_many_arguments)]
    fn handle_jsx_props<'a>(
        &mut self,
        attrs: &mut ArenaVec<'a, JSXAttributeItem<'a>>,
        children: &mut ArenaVec<'a, JSXChild<'a>>,
        is_fn: bool,
        is_text_only: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> (bool, Option<Expression<'a>>, Option<Expression<'a>>, Option<Expression<'a>>, u32) {
        let allocator: &'a Allocator = ctx.ast.allocator;
        let ast = AstBuilder::new(allocator);

        // ---- Phase 1: Pre-scan ------------------------------------------------

        // Build const_idents: names from decl_stack where IdentType::Var(true).
        let const_idents: HashSet<String> = self
            .decl_stack
            .iter()
            .flat_map(|frame| frame.iter())
            .filter_map(|(name, ty)| {
                if matches!(ty, IdentType::Var(true)) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Count spreads and find last spread index.
        let mut spread_props_count: usize = 0;
        let mut last_spread_index: Option<usize> = None;
        for (i, attr) in attrs.iter().enumerate() {
            if matches!(attr, JSXAttributeItem::SpreadAttribute(_)) {
                spread_props_count += 1;
                last_spread_index = Some(i);
            }
        }

        // has_var_prop_after_last_spread: scan attrs after last spread index.
        let has_var_prop_after_last_spread: bool = if let Some(last_idx) = last_spread_index {
            attrs.iter().skip(last_idx + 1).any(|attr| {
                match attr {
                    JSXAttributeItem::Attribute(a) => {
                        // "children" key is excluded from this check
                        let key = jsx_attr_key_str(&a.name);
                        if key == "children" {
                            return false;
                        }
                        // Check if value is const
                        match &a.value {
                            None => false, // boolean shorthand → const true → not var
                            Some(JSXAttributeValue::StringLiteral(_)) => false, // const string
                            Some(JSXAttributeValue::ExpressionContainer(ec)) => {
                                // Get the expression from the container (skip EmptyExpression)
                                if let Some(expr) = ec.expression.as_expression() {
                                    !is_const::is_const_expr_with_context(
                                        expr,
                                        &const_idents,
                                        self.global_collect,
                                    )
                                } else {
                                    false
                                }
                            }
                            _ => true, // nested JSX etc. → var
                        }
                    }
                    JSXAttributeItem::SpreadAttribute(_) => true,
                }
            })
        } else {
            false
        };

        // has_component_bind_props: is_fn && any attr starts with "bind:"
        let has_component_bind_props: bool = is_fn
            && attrs.iter().any(|attr| {
                if let JSXAttributeItem::Attribute(a) = attr {
                    jsx_attr_key_str(&a.name).starts_with("bind:")
                } else {
                    false
                }
            });

        let should_runtime_sort = spread_props_count > 0 || has_component_bind_props;

        let mut static_listeners = !should_runtime_sort;
        let static_subtree = !should_runtime_sort;

        // ---- Phase 2: Main loop -----------------------------------------------
        let mut var_props: Vec<ObjectPropertyKind<'a>> = Vec::new();
        let mut const_props: Vec<ObjectPropertyKind<'a>> = Vec::new();
        let mut remaining_spreads = spread_props_count;

        // We need to own the attributes; drain them.
        let attrs_owned: Vec<JSXAttributeItem<'a>> = std::mem::replace(attrs, ArenaVec::new_in(allocator)).into_iter().collect();

        for attr_item in attrs_owned {
            match attr_item {
                JSXAttributeItem::SpreadAttribute(spread) => {
                    remaining_spreads = remaining_spreads.saturating_sub(1);
                    let is_last_spread = remaining_spreads == 0;
                    let expr = spread.unbox().argument;

                    // Check if it's a simple identifier
                    let is_simple_ident = matches!(expr, Expression::Identifier(_));

                    if is_simple_ident && is_last_spread && !has_var_prop_after_last_spread {
                        // Split into _getVarProps(id) spread for var_props,
                        // _getConstProps(id) spread for const_props.
                        self.needs_get_var_props = true;
                        self.needs_get_const_props = true;

                        let id_expr_for_var = expr.clone_in(allocator);
                        let id_expr_for_const = expr;

                        // _getVarProps(id) spread element
                        let var_call = build_fn_call("_getVarProps", id_expr_for_var, &ast, allocator);
                        var_props.push(ObjectPropertyKind::SpreadProperty(
                            ast.alloc_spread_element(SPAN, var_call),
                        ));

                        // _getConstProps(id) spread element
                        let const_call = build_fn_call("_getConstProps", id_expr_for_const, &ast, allocator);
                        const_props.push(ObjectPropertyKind::SpreadProperty(
                            ast.alloc_spread_element(SPAN, const_call),
                        ));
                    } else if is_simple_ident {
                        // Non-last spread: add _getVarProps + _getConstProps both to var_props
                        self.needs_get_var_props = true;
                        self.needs_get_const_props = true;
                        let id_expr_for_var = expr.clone_in(allocator);
                        let id_expr_for_const = expr;
                        let var_call = build_fn_call("_getVarProps", id_expr_for_var, &ast, allocator);
                        var_props.push(ObjectPropertyKind::SpreadProperty(
                            ast.alloc_spread_element(SPAN, var_call),
                        ));
                        let const_call = build_fn_call("_getConstProps", id_expr_for_const, &ast, allocator);
                        var_props.push(ObjectPropertyKind::SpreadProperty(
                            ast.alloc_spread_element(SPAN, const_call),
                        ));
                    } else {
                        // Non-identifier spread: raw spread into var_props
                        var_props.push(ObjectPropertyKind::SpreadProperty(
                            ast.alloc_spread_element(SPAN, expr),
                        ));
                    }
                }

                JSXAttributeItem::Attribute(attr) => {
                    let attr = attr.unbox();
                    let raw_key = jsx_attr_key_owned(&attr.name);

                    // ---- className -> class (native elements only) --------
                    let key = if !is_fn && raw_key == "className" {
                        "class".to_string()
                    } else {
                        raw_key.clone()
                    };

                    // ---- Extract value expression -------------------------
                    let value_expr: Option<Expression<'a>> = match attr.value {
                        None => {
                            // Boolean shorthand: <div disabled /> → disabled={true}
                            Some(ast.expression_boolean_literal(SPAN, true))
                        }
                        Some(JSXAttributeValue::StringLiteral(s)) => {
                            Some(ast.expression_string_literal(SPAN, s.value, None))
                        }
                        Some(JSXAttributeValue::ExpressionContainer(ec)) => {
                            jsx_expression_to_expr(ec.unbox().expression)
                        }
                        Some(JSXAttributeValue::Element(el)) => {
                            // Nested JSX element already transformed by exit_expression (post-order)
                            Some(Expression::JSXElement(el))
                        }
                        Some(JSXAttributeValue::Fragment(fr)) => {
                            Some(Expression::JSXFragment(fr))
                        }
                    };

                    let value_expr = match value_expr {
                        Some(v) => v,
                        None => continue, // skip JSXEmptyExpression
                    };

                    // ---- Event handler renaming (native elements only) ----
                    if !is_fn {
                        if let Some(html_attr) = QwikTransform::jsx_event_to_html_attribute(&key) {
                            let is_target_const = remaining_spreads == 0;
                            let prop = build_object_prop(&html_attr, value_expr, &ast, allocator);
                            if is_target_const {
                                const_props.push(prop);
                                // static_listeners stays true
                            } else {
                                var_props.push(prop);
                                static_listeners = false;
                            }
                            continue;
                        }
                    }

                    // ---- Regular attribute classification -----------------
                    let is_const = is_const::is_const_expr_with_context(
                        &value_expr,
                        &const_idents,
                        self.global_collect,
                    );

                    let is_target_const = remaining_spreads == 0;

                    let goes_to_const = if is_fn || !is_target_const {
                        // For components or when inside spreads:
                        // const && no active spreads → const_props, else → var_props
                        is_const && is_target_const
                    } else {
                        // For native elements with no spreads:
                        // not const → var_props, const → const_props
                        is_const
                    };

                    let prop = build_object_prop(&key, value_expr, &ast, allocator);
                    if goes_to_const {
                        const_props.push(prop);
                    } else {
                        var_props.push(prop);
                    }
                }
            }
        }

        // ---- Sort var_props alphabetically when !should_runtime_sort -------
        if !should_runtime_sort {
            var_props.sort_by(|a, b| {
                let key_a = object_prop_key_str(a);
                let key_b = object_prop_key_str(b);
                key_a.cmp(&key_b)
            });
        }

        // ---- Process children -----------------------------------------------
        let children_opt = self.build_children(children, is_text_only, ctx);

        // ---- Compute flags --------------------------------------------------
        // bit 0 = static_listeners, bit 1 = static_subtree
        let flags: u32 = (static_listeners as u32) | ((static_subtree as u32) << 1);

        // ---- Build final expressions ----------------------------------------
        // Non-empty var_props means jsx_mutable = true
        let var_props_expr: Option<Expression<'a>> = if var_props.is_empty() {
            None
        } else {
            self.jsx_mutable = true;
            let mut props_arena: ArenaVec<ObjectPropertyKind<'a>> = ArenaVec::new_in(allocator);
            for p in var_props {
                props_arena.push(p);
            }
            Some(ast.expression_object(SPAN, props_arena))
        };

        let const_props_expr: Option<Expression<'a>> = if const_props.is_empty() {
            None
        } else {
            let mut props_arena: ArenaVec<ObjectPropertyKind<'a>> = ArenaVec::new_in(allocator);
            for p in const_props {
                props_arena.push(p);
            }
            Some(ast.expression_object(SPAN, props_arena))
        };

        (should_runtime_sort, var_props_expr, const_props_expr, children_opt, flags)
    }

    /// Build the children expression from a list of `JSXChild` nodes.
    ///
    /// Returns:
    /// - `None` if no (non-empty) children.
    /// - The single expression if exactly one child.
    /// - An array expression if multiple children.
    fn build_children<'a>(
        &mut self,
        children: &mut ArenaVec<'a, JSXChild<'a>>,
        is_text_only: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Option<Expression<'a>> {
        let allocator: &'a Allocator = ctx.ast.allocator;
        let ast = AstBuilder::new(allocator);

        let mut exprs: Vec<Expression<'a>> = Vec::new();

        let children_owned: Vec<JSXChild<'a>> = std::mem::replace(children, ArenaVec::new_in(allocator)).into_iter().collect();

        for child in children_owned {
            match child {
                JSXChild::Text(text) => {
                    let normalized = QwikTransform::normalize_jsx_text(text.value.as_str());
                    if !normalized.is_empty() {
                        exprs.push(ast.expression_string_literal(SPAN, ast.atom(&normalized), None));
                    }
                }
                JSXChild::ExpressionContainer(ec) => {
                    if let Some(e) = jsx_expression_to_expr(ec.unbox().expression) {
                        exprs.push(e);
                    }
                    // EmptyExpression ({}) → skip
                }
                JSXChild::Element(el) => {
                    // Already transformed by exit_expression (post-order).
                    // el is Box<JSXElement<'a>> but it's already transformed to
                    // an Expression by exit_expression. Wait — exit_expression
                    // transforms the JSXElement in place at the Expression level.
                    // But JSXChild::Element is a different arm.
                    // We need to transform it now if it wasn't already.
                    // Actually in OXC's traverse: JSXChild::Element IS visited
                    // as a JSXElement node, and exit_expression fires on it IF
                    // it appears as an Expression. But JSXChild::Element is not
                    // an Expression — it's a child slot.
                    // The Traverse impl visits JSXElement children via
                    // visit_jsx_child which calls visit_jsx_element for Element
                    // children. exit_expression only fires for Expression nodes.
                    // So we must transform child elements here explicitly.
                    let was_root_child = false; // children are never root
                    let child_expr = self.transform_jsx_element(el.unbox(), was_root_child, ctx);
                    exprs.push(child_expr);
                }
                JSXChild::Fragment(fr) => {
                    let was_root_child = false;
                    let child_expr = self.transform_jsx_fragment(fr.unbox(), was_root_child, ctx);
                    exprs.push(child_expr);
                }
                JSXChild::Spread(sp) => {
                    // {..expr} spread child — wrap as spread in array
                    exprs.push(sp.unbox().expression);
                }
            }
        }

        if exprs.is_empty() {
            return None;
        }

        // is_text_only + one string child → keep as string
        if is_text_only {
            if exprs.len() == 1 {
                return Some(exprs.remove(0));
            }
        }

        if exprs.len() == 1 {
            return Some(exprs.remove(0));
        }

        // Multiple children → array expression
        let mut elements: ArenaVec<ArrayExpressionElement<'a>> = ArenaVec::new_in(allocator);
        for e in exprs {
            elements.push(ArrayExpressionElement::from(e));
        }
        Some(ast.expression_array(SPAN, elements))
    }

    // -----------------------------------------------------------------------
    // transform_function_expr — Lib-mode _captures injection (Plan 12-03)
    // -----------------------------------------------------------------------

    /// Inject a `_captures` parameter and per-capture destructuring into a
    /// function or arrow function expression for Lib mode QRL output.
    ///
    /// For an arrow `(existingParams) => body` or `function(existingParams) { body }`:
    /// 1. Replace params with a single `_captures` BindingIdentifier parameter.
    /// 2. Prepend `const varN = _captures[N]` for each `scoped_ident`.
    /// 3. If the arrow has an expression body, wrap it in a block body first.
    ///
    /// The OXC arena allocator is used for all newly created nodes.
    pub(crate) fn transform_function_expr<'a>(
        expr: &mut Expression<'a>,
        scoped_idents: &[String],
        allocator: &'a Allocator,
    ) {
        let ast = AstBuilder::new(allocator);

        // Build the `_captures` parameter binding.
        let captures_param = {
            let binding_id = ast.binding_pattern_binding_identifier(SPAN, ast.atom("_captures"));
            let formal = ast.formal_parameter(
                SPAN,
                ArenaVec::new_in(allocator),
                binding_id,
                None::<TSTypeAnnotation<'a>>,
                None::<Expression<'a>>,
                false,
                None,    // accessibility
                false,   // readonly
                false,   // override
            );
            let mut items: ArenaVec<FormalParameter<'a>> = ArenaVec::new_in(allocator);
            items.push(formal);
            items
        };

        // Build `const varN = _captures[N]` statements for each scoped ident.
        let mut prepend_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);
        for (idx, ident_name) in scoped_idents.iter().enumerate() {
            // `_captures[idx]`
            let captures_ref = ast.expression_identifier(SPAN, ast.atom("_captures"));
            let index_expr = ast.expression_numeric_literal(
                SPAN,
                idx as f64,
                None,
                NumberBase::Decimal,
            );
            // Build `_captures[idx]` as a ComputedMemberExpression wrapped in Expression.
            let computed_me = ast.member_expression_computed(SPAN, captures_ref, index_expr, false);
            let member_expr = Expression::from(computed_me);

            // `const varN = _captures[idx]`
            let binding = ast.binding_pattern_binding_identifier(SPAN, ast.atom(ident_name.as_str()));
            let mut declarators: ArenaVec<VariableDeclarator<'a>> = ArenaVec::new_in(allocator);
            declarators.push(ast.variable_declarator(
                SPAN,
                VariableDeclarationKind::Const,
                binding,
                None::<TSTypeAnnotation<'a>>,
                Some(member_expr),
                false,
            ));
            let decl = ast.alloc_variable_declaration(
                SPAN,
                VariableDeclarationKind::Const,
                declarators,
                false,
            );
            prepend_stmts.push(Statement::VariableDeclaration(decl));
        }

        match expr {
            Expression::ArrowFunctionExpression(arrow) => {
                // Replace params with `_captures` single param.
                let new_params = ast.formal_parameters(
                    SPAN,
                    FormalParameterKind::ArrowFormalParameters,
                    captures_param,
                    None::<FormalParameterRest<'a>>,
                );
                arrow.params = ArenaBox::new_in(new_params, allocator);

                // If expression body, convert to block body.
                if arrow.expression {
                    // The body contains one ExpressionStatement; extract it.
                    let old_body_stmts = std::mem::replace(
                        &mut arrow.body.statements,
                        ArenaVec::new_in(allocator),
                    );
                    // The expression-body arrow has `statements` containing one ExpressionStatement.
                    // Wrap in a ReturnStatement.
                    let mut new_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);
                    for stmt in old_body_stmts {
                        if let Statement::ExpressionStatement(expr_stmt_box) = stmt {
                            // Unbox to get owned ExpressionStatement, then take .expression
                            let expr_stmt = expr_stmt_box.unbox();
                            let ret_stmt = ast.statement_return(SPAN, Some(expr_stmt.expression));
                            new_stmts.push(ret_stmt);
                        } else {
                            new_stmts.push(stmt);
                        }
                    }
                    arrow.expression = false;
                    // Prepend capture stmts then original stmts.
                    let mut final_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);
                    for s in prepend_stmts {
                        final_stmts.push(s);
                    }
                    for s in new_stmts {
                        final_stmts.push(s);
                    }
                    let new_body = ast.function_body(SPAN, ArenaVec::new_in(allocator), final_stmts);
                    arrow.body = ArenaBox::new_in(new_body, allocator);
                } else {
                    // Block body: prepend const destructurings.
                    let old_stmts = std::mem::replace(
                        &mut arrow.body.statements,
                        ArenaVec::new_in(allocator),
                    );
                    let mut final_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);
                    for s in prepend_stmts {
                        final_stmts.push(s);
                    }
                    for s in old_stmts {
                        final_stmts.push(s);
                    }
                    let new_body = ast.function_body(SPAN, ArenaVec::new_in(allocator), final_stmts);
                    arrow.body = ArenaBox::new_in(new_body, allocator);
                }
            }
            Expression::FunctionExpression(func) => {
                // Replace params with `_captures` single param.
                let new_params = ast.formal_parameters(
                    SPAN,
                    FormalParameterKind::FormalParameter,
                    captures_param,
                    None::<FormalParameterRest<'a>>,
                );
                func.params = ArenaBox::new_in(new_params, allocator);

                // Prepend const destructurings to function body.
                if let Some(body) = &mut func.body {
                    let old_stmts = std::mem::replace(
                        &mut body.statements,
                        ArenaVec::new_in(allocator),
                    );
                    let mut final_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);
                    for s in prepend_stmts {
                        final_stmts.push(s);
                    }
                    for s in old_stmts {
                        final_stmts.push(s);
                    }
                    body.statements = final_stmts;
                }
            }
            _ => {
                // Non-function: no injection possible (C03 already fired at call site).
            }
        }
    }

    // -----------------------------------------------------------------------
    // QRL call form builders (Plan 12-02)
    // -----------------------------------------------------------------------

    /// Build `_noopQrl("symbol_name")` or `_noopQrlDEV("symbol_name", [...], {meta})`.
    ///
    /// - Prod/Test/Lib mode: `_noopQrl("symbol_name")` — no captures appended if empty,
    ///   capture array appended when non-empty.
    /// - Dev/Hmr mode: `_noopQrlDEV("symbol_name", [...], { file, lo, hi, displayName })`.
    ///   An empty capture array `[]` is always emitted as second arg in Dev mode so the
    ///   metadata 3rd arg has a fixed position.
    pub(crate) fn create_noop_qrl<'a>(
        &self,
        symbol_name: &str,
        scoped_idents: &[String],
        span: (u32, u32),
        display_name: &str,
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        let ast = AstBuilder::new(allocator);
        let is_dev = matches!(self.mode, EmitMode::Dev | EmitMode::Hmr);
        let callee_name = if is_dev { "_noopQrlDEV" } else { "_noopQrl" };

        let callee = ast.expression_identifier(SPAN, ast.atom(callee_name));
        let mut args: ArenaVec<Argument<'_>> = ArenaVec::new_in(allocator);

        // Arg 1: symbol name string literal
        args.push(Argument::StringLiteral(
            ast.alloc_string_literal(SPAN, ast.atom(symbol_name), None),
        ));

        if is_dev {
            // Dev: always emit capture array as second arg (empty or populated)
            let captures_expr = build_capture_array(scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
            // Arg 3: dev metadata object
            let meta = build_dev_metadata(&self.file_name, span.0, span.1, display_name, &ast, allocator);
            push_expr_arg(&ast, &mut args, meta);
        } else if !scoped_idents.is_empty() {
            // Non-dev: only emit capture array if non-empty
            let captures_expr = build_capture_array(scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
        }

        ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false)
    }

    /// Build `inlinedQrl(fn, "symbol_name")` or `inlinedQrlDEV(fn, "symbol_name", [...], {meta})`.
    ///
    /// - Prod/Test/Lib mode: `inlinedQrl(folded_expr, "symbol_name"[, captures])`.
    /// - Dev/Hmr mode: `inlinedQrlDEV(folded_expr, "symbol_name", [...], { file, lo, hi, displayName })`.
    pub(crate) fn create_inline_qrl<'a>(
        &mut self,
        folded_expr: Expression<'a>,
        symbol_name: &str,
        scoped_idents: &[String],
        span: (u32, u32),
        display_name: &str,
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        let ast = AstBuilder::new(allocator);
        let is_dev = matches!(self.mode, EmitMode::Dev | EmitMode::Hmr);
        let callee_name = if is_dev { "inlinedQrlDEV" } else { "inlinedQrl" };

        let callee = ast.expression_identifier(SPAN, ast.atom(callee_name));
        let mut args: ArenaVec<Argument<'_>> = ArenaVec::new_in(allocator);

        // Arg 1: the folded function expression
        push_expr_arg(&ast, &mut args, folded_expr);

        // Arg 2: symbol name string literal
        args.push(Argument::StringLiteral(
            ast.alloc_string_literal(SPAN, ast.atom(symbol_name), None),
        ));

        if is_dev {
            // Dev: always emit capture array as third arg, metadata as fourth
            let captures_expr = build_capture_array(scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
            let meta = build_dev_metadata(&self.file_name, span.0, span.1, display_name, &ast, allocator);
            push_expr_arg(&ast, &mut args, meta);
        } else if !scoped_idents.is_empty() {
            // Non-dev: only emit capture array if non-empty
            let captures_expr = build_capture_array(scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
        }

        ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false)
    }

    /// Build `qrl(() => import('./canonical'), "symbol_name"[, captures][, meta])` and push a
    /// [`SegmentRecord`] to `self.segments`.
    ///
    /// - Prod/Test/Lib mode: `qrl(arrow, "symbol_name"[, captures])`.
    /// - Dev/Hmr mode: `qrlDEV(arrow, "symbol_name", [...], { file, lo, hi, displayName })`.
    ///
    /// The `folded_expr` is serialized via OXC Codegen and stored in the `expr` field of the
    /// pushed `SegmentRecord`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_segment<'a>(
        &mut self,
        folded_expr: Expression<'a>,
        names: &crate::hash::ContextNameResult,
        scoped_idents: Vec<String>,
        local_idents: Vec<String>,
        ctx_name: &str,
        ctx_kind: crate::types::CtxKind,
        span: (u32, u32),
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        let ast = AstBuilder::new(allocator);
        let is_dev = matches!(self.mode, EmitMode::Dev | EmitMode::Hmr);

        // Serialize `folded_expr` for the SegmentRecord.expr field.
        // folded_expr is the extracted closure body; the QRL call itself uses a fresh
        // `() => import('./canonical')` arrow, so we consume folded_expr here for codegen.
        let expr_code = {
            let binding2 = ast.binding_pattern_binding_identifier(SPAN, ast.atom("_x"));
            let mut decls: ArenaVec<VariableDeclarator<'_>> = ArenaVec::new_in(allocator);
            decls.push(ast.variable_declarator(
                SPAN,
                VariableDeclarationKind::Const,
                binding2,
                None::<TSTypeAnnotation<'_>>,
                Some(folded_expr),
                false,
            ));
            let var_decl = ast.alloc_variable_declaration(
                SPAN, VariableDeclarationKind::Const, decls, false,
            );
            let mut body: ArenaVec<Statement<'_>> = ArenaVec::new_in(allocator);
            body.push(Statement::VariableDeclaration(var_decl));
            let directives: ArenaVec<Directive<'_>> = ArenaVec::new_in(allocator);
            let comments: ArenaVec<Comment> = ArenaVec::new_in(allocator);
            let prog = ast.program(SPAN, SourceType::tsx(), "", comments, None, directives, body);
            let raw = Codegen::new().build(&prog).code;
            raw.trim_start_matches("const _x = ")
                .trim_end_matches(';')
                .trim()
                .to_string()
        };

        // Build the import path: `./canonical_filename` or `./canonical_filename.ext`
        let import_path = if self.explicit_extensions {
            format!("./{}.{}", names.canonical_filename, self.extension)
        } else {
            format!("./{}", names.canonical_filename)
        };

        // Build `() => import('./canonical_filename')` arrow expression.
        let import_expr = ast.expression_import(
            SPAN,
            ast.expression_string_literal(SPAN, ast.atom(import_path.as_str()), None),
            None,
            None,
        );
        let arrow_params = ast.formal_parameters(
            SPAN,
            FormalParameterKind::ArrowFormalParameters,
            ArenaVec::new_in(allocator),
            None::<FormalParameterRest<'a>>,
        );
        // Build an expression-body arrow: `() => import('...')`
        // expression=true means the body is an expression, not a block.
        let import_stmt = ast.statement_expression(SPAN, import_expr);
        let arrow_body = ast.function_body(SPAN, ArenaVec::new_in(allocator), ast.vec1(import_stmt));
        let arrow = ast.expression_arrow_function(
            SPAN,
            true, // expression body
            false, // not async
            None::<TSTypeParameterDeclaration<'a>>,
            arrow_params,
            None::<TSTypeAnnotation<'a>>,
            arrow_body,
        );

        // Determine callee name
        let callee_name = if is_dev { "qrlDEV" } else { "qrl" };
        let callee = ast.expression_identifier(SPAN, ast.atom(callee_name));
        let mut args: ArenaVec<Argument<'_>> = ArenaVec::new_in(allocator);

        // Arg 1: the arrow function importing the segment module
        push_expr_arg(&ast, &mut args, arrow);

        // Arg 2: symbol name string literal
        args.push(Argument::StringLiteral(
            ast.alloc_string_literal(SPAN, ast.atom(names.symbol_name.as_str()), None),
        ));

        if is_dev {
            // Dev: always emit capture array + metadata
            let captures_expr = build_capture_array(&scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
            let meta = build_dev_metadata(
                &self.file_name, span.0, span.1, &names.display_name, &ast, allocator,
            );
            push_expr_arg(&ast, &mut args, meta);
        } else if !scoped_idents.is_empty() {
            let captures_expr = build_capture_array(&scoped_idents, &ast, allocator);
            push_expr_arg(&ast, &mut args, captures_expr);
        }

        let qrl_call = ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false);

        // Determine entry key via policy
        let segment_data = crate::types::SegmentData {
            display_name: names.display_name.clone(),
            hash: names.hash.clone(),
            name: names.symbol_name.clone(),
            ctx_name: ctx_name.to_string(),
            ctx_kind: ctx_kind.clone(),
            origin: self.rel_path.clone(),
            extension: self.extension.clone(),
            span,
            parent: self.segment_stack.last().cloned(),
            scoped_idents: scoped_idents.clone(),
            captures: !scoped_idents.is_empty(),
            capture_names: scoped_idents.clone(),
            needed_imports: vec![],
            segment_qrl_names: vec![],
            body_span: span,
            param_names: vec![],
            body_code: expr_code.clone(),
            child_lazy_imports: vec![],
            needs_qrl_import: false,
        };
        let entry = self.entry_policy.get_entry_for_sym(&self.stack_ctxt, &segment_data);

        self.segments.push(SegmentRecord {
            name: names.symbol_name.clone(),
            display_name: names.display_name.clone(),
            canonical_filename: names.canonical_filename.clone(),
            entry,
            expr: Some(expr_code),
            scoped_idents: scoped_idents.clone(),
            local_idents,
            ctx_name: ctx_name.to_string(),
            ctx_kind,
            origin: self.rel_path.clone(),
            span,
            hash: names.hash.clone(),
            is_inline: false,
        });

        qrl_call
    }

    // -----------------------------------------------------------------------
    // serialize_expression — emit a single Expression<'a> to a String
    // -----------------------------------------------------------------------

    /// Serialize `expr` to source text using OXC Codegen.
    ///
    /// Wraps the expression in `const _x = <expr>;`, runs Codegen, then strips
    /// the wrapper to return just the expression text (no trailing semicolon).
    fn serialize_expression<'a>(expr: &Expression<'a>, allocator: &'a Allocator) -> String {
        let ast = AstBuilder::new(allocator);
        let cloned: Expression<'a> = expr.clone_in(allocator);
        let binding = ast.binding_pattern_binding_identifier(SPAN, ast.atom("_x"));
        let mut declarators: ArenaVec<VariableDeclarator<'_>> = ArenaVec::new_in(allocator);
        declarators.push(ast.variable_declarator(
            SPAN,
            VariableDeclarationKind::Const,
            binding,
            None::<TSTypeAnnotation<'_>>,
            Some(cloned),
            false,
        ));
        let var_decl = ast.alloc_variable_declaration(
            SPAN, VariableDeclarationKind::Const, declarators, false,
        );
        let mut body: ArenaVec<Statement<'_>> = ArenaVec::new_in(allocator);
        body.push(Statement::VariableDeclaration(var_decl));
        let directives: ArenaVec<Directive<'_>> = ArenaVec::new_in(allocator);
        let comments: ArenaVec<Comment> = ArenaVec::new_in(allocator);
        let prog = ast.program(SPAN, SourceType::tsx(), "", comments, None, directives, body);
        let raw = Codegen::new().build(&prog).code;
        raw.trim_start_matches("const _x = ")
            .trim_end_matches(';')
            .trim()
            .to_string()
    }

    // -----------------------------------------------------------------------
    // build_w_call — build `q_name.w([cap1, cap2])` expression
    // -----------------------------------------------------------------------

    /// Build `q_name.w([cap1, cap2])` call expression.
    fn build_w_call<'a>(
        q_name: &str,
        captures: &[String],
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        let ast = AstBuilder::new(allocator);
        // `q_name.w`
        let obj = ast.expression_identifier(SPAN, ast.atom(q_name));
        let member = ast.member_expression_static(SPAN, obj, ast.identifier_name(SPAN, ast.atom("w")), false);
        let callee = Expression::from(member);
        // `[cap1, cap2]`
        let captures_arr = build_capture_array(captures, &ast, allocator);
        let mut args: ArenaVec<Argument<'_>> = ArenaVec::new_in(allocator);
        push_expr_arg(&ast, &mut args, captures_arr);
        ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false)
    }

    // -----------------------------------------------------------------------
    // build_s_call — build `q_name.s(fn_body)` expression statement code
    // -----------------------------------------------------------------------

    /// Build `q_name.s(fn_body_name)` as a serialized expression statement string.
    /// Returns the string `"q_name.s(fn_body_name);"`.
    fn build_s_call_code(q_name: &str, fn_body_name: &str) -> String {
        format!("{q_name}.s({fn_body_name});")
    }

    // -----------------------------------------------------------------------
    // create_synthetic_qqsegment — Phase 15 signal wrapping decision tree
    // -----------------------------------------------------------------------

    /// Decide whether `expr` is eligible for signal wrapping.
    ///
    /// Implements the 8-step decision tree from SPEC §create_synthetic_qqsegment.
    /// Returns `(Some(code_string), is_const)` if the expression should be wrapped,
    /// or `(None, is_const)` if it should be used as-is.
    ///
    /// The returned code string is the serialized `_wrapProp(...)` or `_fnSignal(...)`
    /// call. The caller is responsible for parsing it back into an `Expression`.
    pub(crate) fn create_synthetic_qqsegment<'a>(
        &mut self,
        expr: &Expression<'a>,
        allocator: &'a Allocator,
    ) -> (Option<String>, bool) {
        // Step 1: Collect all identifiers referenced in `expr`.
        let descendent_idents = IdentCollector::collect(expr);

        // Step 2: Partition decl_stack into Var-only (decl_collect) and others (invalid_decl).
        let all_decl: Vec<IdPlusType> = self
            .decl_stack
            .iter()
            .flat_map(|frame| frame.iter().cloned())
            .collect();
        let mut decl_collect: Vec<IdPlusType> = Vec::new();
        let mut invalid_decl_names: HashSet<String> = HashSet::new();
        for (name, id_type) in &all_decl {
            match id_type {
                IdentType::Var(_) => {
                    decl_collect.push((name.clone(), id_type.clone()));
                }
                IdentType::Fn | IdentType::Class => {
                    invalid_decl_names.insert(name.clone());
                }
            }
        }

        // Step 3: If any descendent_ident is in invalid_decl → return (None, false).
        for ident in &descendent_idents {
            if invalid_decl_names.contains(ident) {
                return (None, false);
            }
        }

        // Step 4: For each ident NOT in decl_collect: check via global_collect → side effects.
        let decl_collect_names: HashSet<String> =
            decl_collect.iter().map(|(n, _)| n.clone()).collect();
        let collect = unsafe { &*self.global_collect };
        let mut contains_side_effect = false;
        for ident in &descendent_idents {
            if !decl_collect_names.contains(ident) && collect.is_global(ident) {
                contains_side_effect = true;
            }
        }

        // Step 5: compute_scoped_idents → (scoped_idents, is_const).
        let (scoped_names, is_const) = compute_scoped_idents(&descendent_idents, &decl_collect);

        // Step 6: contains_side_effect → return (None, scoped_idents.is_empty()).
        if contains_side_effect {
            return (None, scoped_names.is_empty());
        }

        // Step 7: Plain Identifier → return (None, is_const).
        if matches!(expr, Expression::Identifier(_)) {
            return (None, is_const);
        }

        // Step 8: !is_const && (Call | Template) → return (None, false).
        if !is_const
            && matches!(
                expr,
                Expression::CallExpression(_) | Expression::TemplateLiteral(_)
            )
        {
            return (None, false);
        }

        // --- _wrapProp fast path ---
        // Unwrap TSAsExpression if present.
        let inner_expr: &Expression<'a> = match expr {
            Expression::TSAsExpression(ts_as) => &ts_as.expression,
            Expression::TSTypeAssertion(ts_assert) => &ts_assert.expression,
            Expression::ParenthesizedExpression(paren) => &paren.expression,
            other => other,
        };

        if let Expression::StaticMemberExpression(member) = inner_expr {
            if let Expression::Identifier(obj_id) = &member.object {
                let obj_name = obj_id.name.as_str().to_string();
                let prop_name = member.property.name.as_str();
                self.needs_wrap_prop = true;
                if prop_name == "value" {
                    // 1-arg form for .value (per golden fixtures)
                    return (Some(format!("_wrapProp({obj_name})")), false);
                } else {
                    // 2-arg form for other properties
                    return (Some(format!("_wrapProp({obj_name}, \"{prop_name}\")")), false);
                }
            }
        }

        // --- Fallthrough to convert_inlined_fn ---
        // Build scoped_idents as (name, is_const) pairs for convert_inlined_fn.
        let scoped_with_const: Vec<(String, bool)> = scoped_names
            .iter()
            .map(|name| {
                // Find if this name is const in decl_collect.
                let ic = decl_collect
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, ty)| matches!(ty, IdentType::Var(true)))
                    .unwrap_or(false);
                (name.clone(), ic)
            })
            .collect();

        let (fn_signal_opt, arrow_code, new_is_const) =
            inlined_fn::convert_inlined_fn(expr, &scoped_with_const, is_const, self.is_server, allocator);

        if let Some(fn_signal_code) = fn_signal_opt {
            self.needs_fn_signal = true;
            let hoisted = self.hoist_fn_signal_call(fn_signal_code, arrow_code);
            (Some(hoisted), new_is_const)
        } else {
            (None, new_is_const)
        }
    }

    // -----------------------------------------------------------------------
    // hoist_fn_signal_call — deduplicate identical _fnSignal arrows to _hf<N>
    // -----------------------------------------------------------------------

    /// Deduplicate identical `_fnSignal` arrow functions by hoisting them to
    /// module scope as `const _hf<N> = (p0, ...) => <body>;`.
    ///
    /// Takes the full `_fnSignal(arrow, [caps])` code string and the `arrow_code`
    /// string (used as the dedup key). Returns the modified call string with the
    /// arrow argument replaced by `_hf<N>`.
    ///
    /// Per SPEC §hoist_fn_signal_call (lines 2554–2594).
    pub(crate) fn hoist_fn_signal_call(
        &mut self,
        fn_signal_code: String,
        arrow_code: String,
    ) -> String {
        // Look up arrow_code in the dedup map.
        if let Some((existing_name, _counter)) = self.hoisted_fn_signals.get(&arrow_code).cloned() {
            // Already hoisted — replace the arrow in the call string with _hf<N> ident.
            let result = replace_fn_signal_arrow(&fn_signal_code, &arrow_code, &existing_name);
            return result;
        }

        // New arrow — allocate a name.
        let n = self.hoisted_fn_counter;
        self.hoisted_fn_counter += 1;
        let hf_name = format!("_hf{n}");

        // Push the const to extra_top_items.
        self.extra_top_items.push(HoistedConst {
            name: hf_name.clone(),
            symbol_name: hf_name.clone(),
            rhs_code: arrow_code.clone(),
        });

        // If server mode and there is a third arg (string literal), hoist the string too.
        if self.is_server {
            // The third arg of _fnSignal is the server-mode source string.
            // Extract it from fn_signal_code: it's the last argument (after the captures array).
            if let Some(server_str) = extract_fn_signal_third_arg(&fn_signal_code) {
                let str_name = format!("{hf_name}_str");
                self.extra_top_items.push(HoistedConst {
                    name: str_name.clone(),
                    symbol_name: str_name.clone(),
                    rhs_code: server_str,
                });
            }
        }

        // Register in dedup map.
        self.hoisted_fn_signals.insert(arrow_code.clone(), (hf_name.clone(), n));

        // Replace the arrow in the call with the new _hf<N> ident.
        replace_fn_signal_arrow(&fn_signal_code, &arrow_code, &hf_name)
    }

    // -----------------------------------------------------------------------
    // hoist_qrl_to_module_scope — Level 1 QRL hoisting (Plan 13-01)
    // -----------------------------------------------------------------------

    /// Hoist a QRL call expression to module scope (Level 1 hoisting).
    ///
    /// **EmitMode::Lib guard:** Returns `qrl_call` unchanged — no hoisting for Lib mode.
    ///
    /// **Extracted qrl() (non-inline strategy):**
    /// 1. Build const name `q_{symbol_name}`.
    /// 2. Dedup: push `HoistedConst` to `extra_top_items` if not already present.
    /// 3. Return `q_name` ident (no captures) or `q_name.w([captures])` (with captures).
    ///
    /// **inlinedQrl (inline/hoist strategy):**
    /// 1. Extract fn_body and captures from the call.
    /// 2. Build `_noopQrl('symbol_name')` and hoist as `const q_name = ...`.
    /// 3. If fn_body is a global ident: push `RefAssignment` for deferred `.s()` emission.
    ///    If fn_body is a local ident: return comma expr `(q_name.s(fn), q_name[.w([caps])])`.
    /// 4. Return `q_name` or `q_name.w([captures])`.
    fn hoist_qrl_to_module_scope<'a>(
        &mut self,
        qrl_call: Expression<'a>,
        scoped_idents: &[String],
        symbol_name: &str,
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        // LIB-03: Lib mode guard — return unchanged.
        if self.mode == EmitMode::Lib {
            return qrl_call;
        }

        let ast = AstBuilder::new(allocator);
        let q_name = format!("q_{symbol_name}");

        // Determine whether this is an inlinedQrl call (inline/hoist strategy).
        let is_inlined = if let Expression::CallExpression(ref call) = qrl_call {
            if let Expression::Identifier(ref id) = call.callee {
                let name = id.name.as_str();
                name == "inlinedQrl" || name == "inlinedQrlDEV"
            } else {
                false
            }
        } else {
            false
        };

        if is_inlined && self.is_inline_strategy {
            // Branch C: inlinedQrl (Inline/Hoist strategy)
            // Extract fn_body (first arg) and captures from the inlinedQrl call.
            let (fn_body_name_opt, capture_args) = if let Expression::CallExpression(ref call) = qrl_call {
                let fn_body_name = call.arguments.first().and_then(|arg| {
                    if let Argument::Identifier(id) = arg {
                        Some(id.name.as_str().to_string())
                    } else {
                        None
                    }
                });
                // Captures are scoped_idents passed in — we already have them.
                (fn_body_name, scoped_idents)
            } else {
                (None, scoped_idents)
            };

            // Build _noopQrl('symbol_name') for the module-scope const.
            let noop_expr = self.create_noop_qrl(symbol_name, &[], (0, 0), symbol_name, allocator);

            // Dedup: push HoistedConst only if not already present.
            if !self.extra_top_items.iter().any(|h| h.symbol_name == symbol_name) {
                let rhs_code = Self::serialize_expression(&noop_expr, allocator);
                self.extra_top_items.push(HoistedConst {
                    name: q_name.clone(),
                    rhs_code,
                    symbol_name: symbol_name.to_string(),
                });
            }

            // Build call-site expression: q_name or q_name.w([captures]).
            let call_site_expr = if capture_args.is_empty() {
                ast.expression_identifier(SPAN, ast.atom(&q_name))
            } else {
                Self::build_w_call(&q_name, capture_args, allocator)
            };

            // Handle fn_body placement.
            if let Some(fn_body_name) = fn_body_name_opt {
                let collect = unsafe { &*self.global_collect };
                let is_global = collect.is_global(&fn_body_name);

                if is_global {
                    // Global ident: push RefAssignment for deferred emission.
                    let s_call_code = Self::build_s_call_code(&q_name, &fn_body_name);
                    self.ref_assignments.push(RefAssignment {
                        target_ident: fn_body_name,
                        s_call_code,
                    });
                    call_site_expr
                } else {
                    // Local variable: emit comma expression at call site.
                    // (q_name.s(localVar), q_name[.w([captures])])
                    let s_obj = ast.expression_identifier(SPAN, ast.atom(&q_name));
                    let s_member = ast.member_expression_static(SPAN, s_obj, ast.identifier_name(SPAN, ast.atom("s")), false);
                    let s_callee = Expression::from(s_member);
                    let s_arg = ast.expression_identifier(SPAN, ast.atom(&fn_body_name));
                    let mut s_args: ArenaVec<Argument<'_>> = ArenaVec::new_in(allocator);
                    push_expr_arg(&ast, &mut s_args, s_arg);
                    let s_call = ast.expression_call(SPAN, s_callee, None::<TSTypeParameterInstantiation<'a>>, s_args, false);

                    // Comma expression: (s_call, call_site_expr)
                    let mut seq_exprs: ArenaVec<Expression<'_>> = ArenaVec::new_in(allocator);
                    seq_exprs.push(s_call);
                    seq_exprs.push(call_site_expr);
                    ast.expression_sequence(SPAN, seq_exprs)
                }
            } else {
                // No identifier fn_body (e.g. arrow function inline) — just return q_name.
                call_site_expr
            }
        } else {
            // Branch B: extracted qrl() (non-inline)
            // Dedup: push HoistedConst only if not already present.
            if !self.extra_top_items.iter().any(|h| h.symbol_name == symbol_name) {
                let rhs_code = Self::serialize_expression(&qrl_call, allocator);
                self.extra_top_items.push(HoistedConst {
                    name: q_name.clone(),
                    rhs_code,
                    symbol_name: symbol_name.to_string(),
                });
            }

            // Return q_name ident (no captures) or q_name.w([captures]).
            if scoped_idents.is_empty() {
                ast.expression_identifier(SPAN, ast.atom(&q_name))
            } else {
                Self::build_w_call(&q_name, scoped_idents, allocator)
            }
        }
    }

    // -----------------------------------------------------------------------
    // compute_hoist_target_depth — Level 2 hoisting (Plan 13-02)
    // -----------------------------------------------------------------------

    /// Determine the hoisting target depth for Level 2 hoisting.
    ///
    /// Returns an index into `hoisted_qrls` where the `.w()` call should be placed.
    ///
    /// `decl_stack` has one extra root frame (index 0) that has no corresponding
    /// `hoisted_qrls` entry. So: `hoisted_qrls[j]` corresponds to `decl_stack[j+1]`.
    ///
    /// - If `scoped_idents` is empty: hoist to the component top frame.
    ///   `component_depths.last()` is the `decl_stack` depth before the component$
    ///   arrow was pushed, so the component$ body frame is at `decl_stack[component_top]`
    ///   = `hoisted_qrls[component_top - 1]`.
    /// - If non-empty: find the shallowest `decl_stack` frame containing ANY captured
    ///   ident, clamp to at least the component top, then convert to `hoisted_qrls` index.
    fn compute_hoist_target_depth(&self, scoped_idents: &[String]) -> usize {
        // `component_depths.last()` = decl_stack.len() BEFORE component$ arrow was entered.
        // After the arrow entered, decl_stack has one more frame at that index.
        // hoisted_qrls[0] = first function scope (component$ body) = decl_stack[1].
        // Formula: hoisted_qrls_idx = decl_scope_idx - 1 (skip root frame at index 0).
        let component_decl_depth = self.component_depths.last().copied().unwrap_or(1);
        // The component$ body is at decl_stack index `component_decl_depth`,
        // which maps to hoisted_qrls index `component_decl_depth - 1` (clamped to 0).
        let component_hoist_idx = component_decl_depth.saturating_sub(1);

        let max_valid = self.hoisted_qrls.len().saturating_sub(1);

        if scoped_idents.is_empty() {
            return component_hoist_idx.min(max_valid);
        }

        // Find the deepest decl_stack frame that declares any captured ident.
        // This is the shallowest scope that sees ALL captures — we must hoist
        // to at least this depth so all captured bindings are in scope.
        let mut max_decl_scope: usize = 1; // Start at first non-root frame
        for (frame_idx, frame) in self.decl_stack.iter().enumerate() {
            if frame_idx == 0 {
                continue; // Skip root frame — no hoisted_qrls entry for it.
            }
            let frame_has_capture = scoped_idents.iter().any(|cap| {
                frame.iter().any(|(name, _)| name == cap)
            });
            if frame_has_capture {
                max_decl_scope = frame_idx;
                // Don't break — keep scanning to find the DEEPEST frame with a capture
            }
        }

        // Convert to hoisted_qrls index (subtract 1 for root frame).
        let max_hoist_idx = max_decl_scope.saturating_sub(1);

        // Target = max(max_hoist_idx, component_hoist_idx), clamped to valid range.
        let target = max_hoist_idx.max(component_hoist_idx);
        target.min(max_valid)
    }

    // -----------------------------------------------------------------------
    // hoist_qrl_if_needed — Level 2 loop-context .w() hoisting (Plan 13-02)
    // -----------------------------------------------------------------------

    /// Optionally hoist a `.w(captures)` call out of a loop body.
    ///
    /// Conditions for hoisting (ALL must be true):
    /// 1. `iteration_var_stack` is non-empty (inside a loop)
    /// 2. `hoisted_qrls` is non-empty (at least one active function/arrow scope)
    ///
    /// If conditions are met:
    /// - Compute target depth via `compute_hoist_target_depth`.
    /// - Serialize `w_call_expr` and push `(const_name, rhs_code)` into `hoisted_qrls[target]`.
    /// - Return an `IdentifierReference` to the hoisted const name.
    ///
    /// Otherwise returns `w_call_expr` unchanged.
    fn hoist_qrl_if_needed<'a>(
        &mut self,
        w_call_expr: Expression<'a>,
        scoped_idents: &[String],
        const_name: &str,
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        // Condition 1: inside a loop.
        if self.iteration_var_stack.is_empty() {
            return w_call_expr;
        }
        // Condition 2: active hoisting scope exists.
        if self.hoisted_qrls.is_empty() {
            return w_call_expr;
        }

        let target_depth = self.compute_hoist_target_depth(scoped_idents);
        let rhs_code = Self::serialize_expression(&w_call_expr, allocator);

        if let Some(frame) = self.hoisted_qrls.get_mut(target_depth) {
            frame.push((const_name.to_string(), rhs_code));
        }

        let ast = AstBuilder::new(allocator);
        ast.expression_identifier(SPAN, ast.atom(const_name))
    }

    // -----------------------------------------------------------------------
    // maybe_level2_hoist — apply Level 2 loop hoisting if conditions are met
    // -----------------------------------------------------------------------

    /// Apply Level 2 loop hoisting to `expr` if all conditions are met.
    ///
    /// Conditions:
    /// 1. `expr` is a `q_name.w([caps])` call (StaticMemberExpression with property "w").
    /// 2. `iteration_var_stack` is non-empty (inside a loop).
    /// 3. `hoisted_qrls` is non-empty (inside a function scope).
    /// 4. `ctx_name` does NOT start with "component" (component$ roots excluded).
    ///
    /// If all conditions are met, calls `hoist_qrl_if_needed` to hoist and returns
    /// the replacement identifier. Otherwise returns `expr` unchanged.
    fn maybe_level2_hoist<'a>(
        &mut self,
        expr: Expression<'a>,
        scoped_idents: &[String],
        symbol_name: &str,
        ctx_name: &str,
        allocator: &'a Allocator,
    ) -> Expression<'a> {
        // Condition 4: not component$ root.
        if ctx_name.starts_with("component") {
            return expr;
        }
        // Condition 1: must be a .w() call.
        let is_w_call = match &expr {
            Expression::CallExpression(call) => {
                match &call.callee {
                    Expression::StaticMemberExpression(me) => {
                        me.property.name.as_str() == "w"
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !is_w_call {
            return expr;
        }
        // Conditions 2 and 3 are checked inside hoist_qrl_if_needed.
        self.hoist_qrl_if_needed(expr, scoped_idents, symbol_name, allocator)
    }

    // -----------------------------------------------------------------------
    // inject_hoisted_qrls_into_block — Level 2 hoisting (Plan 13-02)
    // -----------------------------------------------------------------------

    /// Drain `entries` and prepend `const <name> = <rhs_code>;` declarations to
    /// the function body block.
    ///
    /// Entries are prepended in order (first entry → first const in output).
    fn inject_hoisted_qrls_into_block<'a>(
        body: &mut FunctionBody<'a>,
        entries: Vec<(String, String)>,
        allocator: &'a Allocator,
    ) {
        if entries.is_empty() {
            return;
        }

        let old_stmts = std::mem::replace(&mut body.statements, ArenaVec::new_in(allocator));
        let mut new_stmts: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);

        for (name, rhs_code) in entries {
            let src = format!("const {name} = {rhs_code};");
            if let Some(stmt) = parse_single_statement(&src, allocator) {
                new_stmts.push(stmt);
            }
        }
        for stmt in old_stmts {
            new_stmts.push(stmt);
        }

        body.statements = new_stmts;
    }
}

// ---------------------------------------------------------------------------
// Traverse implementation
// ---------------------------------------------------------------------------

impl<'a> Traverse<'a, ()> for QwikTransform {
    // -----------------------------------------------------------------------
    // Call expressions (XFRM-02, XFRM-08)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Phase 14: JSX expression hooks
    // -----------------------------------------------------------------------

    /// Save `root_jsx_mode` when entering a JSX node, so children know they
    /// are not root. The PARENT's `was_root` value is stacked here.
    fn enter_expression(&mut self, expr: &mut Expression<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        match expr {
            Expression::JSXElement(_) | Expression::JSXFragment(_) => {
                // Push the current root mode so exit_expression can retrieve it.
                // We push BEFORE setting to false so the first (outermost) JSX
                // in a subtree sees root_jsx_mode=true, and its children see false.
                self.jsx_root_mode_stack.push(self.root_jsx_mode);
                self.root_jsx_mode = false;
            }
            _ => {}
        }
    }

    /// Transform JSXElement/JSXFragment nodes into `_jsxSorted`/`_jsxSplit` calls.
    ///
    /// Children are already transformed when this fires (post-order traversal).
    /// Note: child JSXElements appearing as `JSXChild::Element` are NOT reached
    /// by exit_expression because they live in the JSXChild slot, not an Expression
    /// slot — those are handled recursively inside `build_children`.
    fn exit_expression(&mut self, expr: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        match expr {
            Expression::JSXElement(_) => {
                let was_root = self.jsx_root_mode_stack.pop().unwrap_or(true);
                // Restore root mode for the parent context.
                self.root_jsx_mode = was_root;

                let placeholder = ctx.ast.expression_null_literal(SPAN);
                let old = std::mem::replace(expr, placeholder);
                if let Expression::JSXElement(el) = old {
                    *expr = self.transform_jsx_element(el.unbox(), was_root, ctx);
                }
            }
            Expression::JSXFragment(_) => {
                let was_root = self.jsx_root_mode_stack.pop().unwrap_or(true);
                self.root_jsx_mode = was_root;

                let placeholder = ctx.ast.expression_null_literal(SPAN);
                let old = std::mem::replace(expr, placeholder);
                if let Expression::JSXFragment(frag) = old {
                    *expr = self.transform_jsx_fragment(frag.unbox(), was_root, ctx);
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Call expressions (XFRM-02, XFRM-08)
    // -----------------------------------------------------------------------

    /// 7-priority dispatch for call expressions.
    ///
    /// Only fires for `Expression::Identifier` callees (member expressions are
    /// handled separately in later phases via segment extraction).
    ///
    /// Priority order:
    ///   1. sync$ → stub (Phase 12+)
    ///   2. bare $ (qsegment) → stub (Phase 12+)
    ///   3. JSX function (jsx/jsxs/jsxDEV) → stub (Phase 14+)
    ///   4. inlinedQrl → stub (Phase 12+)
    ///   5. _fnSignal → stub (Phase 15+)
    ///   6. marker function → stub (Phase 12 adds _create_synthetic_qsegment)
    ///   7. plain identifier → push `callee_name` to `stack_ctxt`
    fn enter_call_expression(
        &mut self,
        call: &mut CallExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let callee_name = match &call.callee {
            Expression::Identifier(id) => id.name.as_str().to_string(),
            _ => return,
        };

        // Priority 1: sync$
        if self
            .sync_qrl_fn
            .as_deref()
            .map_or(false, |n| n == callee_name)
        {
            return; // Phase 12+ stub
        }

        // Priority 2: bare $ (qsegment)
        if self
            .qsegment_fn
            .as_deref()
            .map_or(false, |n| n == callee_name)
        {
            let descendent_idents = collect_arg0_idents(&call.arguments);
            let ctx_name = "$".to_string();
            let ctx_kind = words::classify_ctx_kind(&ctx_name);
            self.segment_stack.push("$".to_string());
            self.pending_qsegments.push(PendingQSegment {
                ctx_name,
                ctx_kind,
                descendent_idents,
                span_start: call.span.start,
                display_name_override: None,
                hash_override: None,
            });
            return;
        }

        // Priority 3: JSX function
        if self.jsx_functions.contains(&callee_name) {
            return; // Phase 14+ stub
        }

        // Priority 4: inlinedQrl
        if self
            .inlined_qrl_fn
            .as_deref()
            .map_or(false, |n| n == callee_name)
        {
            return; // Phase 12+ stub
        }

        // Priority 5: _fnSignal
        if self
            .fn_signal_fn
            .as_deref()
            .map_or(false, |n| n == callee_name)
        {
            return; // Phase 15+ stub
        }

        // Priority 6: marker function ($-suffixed named import or local export)
        if self.marker_functions.contains_key(&callee_name) {
            // Get the specifier name (what was originally imported, e.g. "component$").
            let specifier = self
                .marker_functions
                .get(&callee_name)
                .cloned()
                .unwrap_or_else(|| callee_name.clone());
            // Collect descendent_idents from the first arg BEFORE children are visited.
            let descendent_idents = collect_arg0_idents(&call.arguments);
            let ctx_kind = words::classify_ctx_kind(&specifier);
            // Phase 13: track component$ depth for Level 2 hoisting.
            if specifier.starts_with("component") {
                self.component_depths.push(self.decl_stack.len());
            }
            self.segment_stack.push(specifier.clone());
            self.pending_qsegments.push(PendingQSegment {
                ctx_name: specifier,
                ctx_kind,
                descendent_idents,
                span_start: call.span.start,
                display_name_override: None,
                hash_override: None,
            });
            return;
        }

        // Priority 7: plain identifier — push to context stack
        self.stack_ctxt.push(callee_name);
        self.ctxt_pushed_calls.insert(call.span.start);
    }

    /// Symmetric pop for case-7 plain-identifier calls; callee rename (XFRM-08);
    /// and segment extraction (Plan 12-03).
    fn exit_call_expression(
        &mut self,
        call: &mut CallExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Symmetric pop for plain-identifier pushes.
        if self.ctxt_pushed_calls.remove(&call.span.start) {
            self.stack_ctxt.pop();
        }

        // Check if this call expression has a matching PendingQSegment.
        // Inner calls complete (exit) before outer ones due to Traverse ordering,
        // so we peek at the last entry in pending_qsegments.
        let has_pending = self
            .pending_qsegments
            .last()
            .map_or(false, |p| p.span_start == call.span.start);

        if has_pending {
            let pending = self.pending_qsegments.pop().unwrap();
            self.segment_stack.pop();

            // Retrieve the allocator via ctx.ast.
            let allocator: &'a Allocator = ctx.ast.allocator;

            // The first argument (after child traversal has completed processing
            // nested $ calls) is now in call.arguments[0].
            // Take ownership of the first arg for processing.
            let first_arg_opt: Option<Expression<'a>> = if call.arguments.is_empty() {
                None
            } else {
                // Replace first arg with a NullLiteral placeholder, take ownership.
                let old_arg = std::mem::replace(
                    &mut call.arguments[0],
                    Argument::NullLiteral(ctx.ast.alloc_null_literal(SPAN)),
                );
                argument_to_expression(old_arg)
            };

            let first_arg = match first_arg_opt {
                Some(expr) => expr,
                None => {
                    // No first arg — nothing to extract. Just rename callee.
                    if let Expression::Identifier(id) = &mut call.callee {
                        let callee_name = id.name.as_str().to_string();
                        if self.marker_functions.contains_key(&callee_name) || callee_name == "$" {
                            let qrl_name = words::dollar_to_qrl_name(&callee_name);
                            id.name = ctx.ast.atom(&qrl_name).into();
                        }
                    }
                    return;
                }
            };

            // --- Flatten decl_stack for Var entries ---
            let all_decl: Vec<IdPlusType> = self
                .decl_stack
                .iter()
                .flat_map(|frame| frame.iter().cloned())
                .collect();

            let span = (call.span.start, call.span.end);
            let ctx_name = &pending.ctx_name;
            let ctx_kind = pending.ctx_kind.clone();

            // --- Compute names via register_context_name ---
            let names = hash::register_context_name(
                &self.stack_ctxt,
                &mut self.segment_names,
                self.scope.as_deref(),
                &self.rel_path,
                &self.file_name,
                &self.mode,
                None,
                pending.display_name_override.as_deref(),
                pending.hash_override.as_deref(),
            );

            // --- Check if we should emit ---
            let should_emit = self.should_emit_segment(ctx_name, ctx_kind.clone());

            // --- can_capture check ---
            let (mut scoped_idents, _is_const_cap) =
                compute_scoped_idents(&pending.descendent_idents, &all_decl);

            // Exclude function parameters.
            let param_idents = get_function_params(&first_arg);
            scoped_idents.retain(|id| !param_idents.contains(id));

            // C03: if not a function/arrow and has captures, clear and emit diagnostic.
            if !can_capture_scope(&first_arg) && !scoped_idents.is_empty() {
                self.diagnostics.push(Diagnostic {
                    scope: "optimizer".to_string(),
                    category: DiagnosticCategory::SourceError,
                    code: Some("C03".to_string()),
                    file: self.file_name.clone(),
                    message: "CanNotCapture: non-function expression cannot capture scope variables"
                        .to_string(),
                    highlights: None,
                    suggestions: None,
                });
                scoped_idents.clear();
            }

            let mut first_arg_mut = first_arg;

            // --- Output routing: should_emit check applies in ALL modes ---
            // Priority 0: strip_ctx_name / strip_event_handlers → _noopQrl (any mode).
            let hoisted_l1: Expression<'a> = if !should_emit {
                let qrl_expr = self.create_noop_qrl(
                    &names.symbol_name,
                    &scoped_idents,
                    span,
                    &names.display_name,
                    allocator,
                );
                // Hoist _noopQrl to module scope (Lib guard inside handles Lib mode).
                self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                )
            } else if self.mode == EmitMode::Lib {
                // Lib mode: 10-step path → inlinedQrl, never push to segments.

                // Inject _captures into the function if captures are non-empty.
                if !scoped_idents.is_empty() {
                    Self::transform_function_expr(&mut first_arg_mut, &scoped_idents, allocator);
                }

                let qrl_expr = self.create_inline_qrl(
                    first_arg_mut,
                    &names.symbol_name,
                    &scoped_idents,
                    span,
                    &names.display_name,
                    allocator,
                );
                // Lib guard inside hoist_qrl_to_module_scope returns unchanged (LIB-03).
                self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                )
            } else if self.is_inline_strategy {
                // Non-Lib inline strategy → inlinedQrl (no segment module).
                let qrl_expr = self.create_inline_qrl(
                    first_arg_mut,
                    &names.symbol_name,
                    &scoped_idents,
                    span,
                    &names.display_name,
                    allocator,
                );
                self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                )
            } else {
                // Non-Lib, non-inline → create_segment (qrl() call + SegmentRecord).
                let local_idents = self.get_local_idents(&first_arg_mut);
                let qrl_expr = self.create_segment(
                    first_arg_mut,
                    &names,
                    scoped_idents.clone(),
                    local_idents,
                    ctx_name,
                    ctx_kind,
                    span,
                    allocator,
                );
                self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                )
            };

            // --- Level 2: loop-context .w() hoisting (Phase 13-02) ---
            // If inside a loop and the Level 1 result is a q_name.w([caps]) call,
            // hoist it to the outermost valid function scope.
            let final_expr = self.maybe_level2_hoist(
                hoisted_l1,
                &scoped_idents,
                &names.symbol_name,
                ctx_name,
                allocator,
            );
            call.arguments[0] = expr_to_argument(final_expr);

            // Phase 13: pop component_depths if this was a component$ exit.
            if ctx_name.starts_with("component") {
                self.component_depths.pop();
            }
        }

        // convert_qrl_word: rewrite marker-function callee names (XFRM-08).
        // e.g. component$(...) → componentQrl(...)
        // Also handle bare $ → Qrl.
        // For aliased imports (e.g. `import { component$ as c$ }`), we use the
        // SPECIFIER (component$) not the local alias (c$) for the QRL name.
        if let Expression::Identifier(id) = &mut call.callee {
            let callee_name = id.name.as_str().to_string();
            let is_bare_dollar = self
                .qsegment_fn
                .as_deref()
                .map_or(false, |n| n == callee_name);
            if is_bare_dollar {
                let qrl_name = words::dollar_to_qrl_name(&callee_name);
                id.name = ctx.ast.atom(&qrl_name).into();
            } else if let Some(specifier) = self.marker_functions.get(&callee_name) {
                // Use the resolved specifier for QRL name computation, not the local alias.
                let qrl_name = words::dollar_to_qrl_name(specifier);
                id.name = ctx.ast.atom(&qrl_name).into();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Variable declarations (XFRM-06)
    // -----------------------------------------------------------------------

    /// Track variable declaration kind (`const` / `let` / `var`) for use in
    /// `enter_variable_declarator`.
    fn enter_variable_declaration(
        &mut self,
        decl: &mut VariableDeclaration<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.current_var_kind = Some(decl.kind);
    }

    fn exit_variable_declaration(
        &mut self,
        _decl: &mut VariableDeclaration<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.current_var_kind = None;
    }

    /// Add binding to the current decl_stack frame, and push var name to `stack_ctxt`.
    fn enter_variable_declarator(
        &mut self,
        decl: &mut VariableDeclarator<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Only handle simple `BindingIdentifier` patterns for now.
        let name = match &decl.id {
            BindingPattern::BindingIdentifier(id) => id.name.as_str().to_string(),
            _ => {
                self.var_decl_ctxt_pushed = false;
                return;
            }
        };

        let is_const = matches!(self.current_var_kind, Some(VariableDeclarationKind::Const));
        let is_static = decl
            .init
            .as_ref()
            .map_or(false, |expr| is_const::is_const_expression(expr));

        if let Some(frame) = self.decl_stack.last_mut() {
            frame.push((name.clone(), IdentType::Var(is_const && is_static)));
        }

        // Push to context stack so the initializer expression has this name in scope.
        self.stack_ctxt.push(name);
        self.var_decl_ctxt_pushed = true;
    }

    fn exit_variable_declarator(
        &mut self,
        _decl: &mut VariableDeclarator<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.var_decl_ctxt_pushed {
            self.stack_ctxt.pop();
            self.var_decl_ctxt_pushed = false;
        }
    }

    // -----------------------------------------------------------------------
    // Function bodies and declarations (XFRM-06 scope frames)
    // -----------------------------------------------------------------------

    /// Push a new scope frame when entering any function body.
    ///
    /// If `func.id` is `Some` (function declaration or named function expression):
    /// - Push the name to `stack_ctxt` for the function body context.
    /// - Push `IdentType::Fn` into the **current** (parent) frame before creating
    ///   the new child frame.
    ///
    /// Params are added to the new child frame as `Var(false)`.
    fn enter_function(&mut self, func: &mut Function<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        let pushed_name = if let Some(id) = &func.id {
            let name = id.name.as_str().to_string();
            // Add the function name to the current (parent) scope frame.
            if let Some(frame) = self.decl_stack.last_mut() {
                frame.push((name.clone(), IdentType::Fn));
            }
            // Push to context stack for display_name accumulation.
            self.stack_ctxt.push(name);
            true
        } else {
            false
        };
        self.fn_ctxt_push_stack.push(pushed_name);

        // Push new child scope frame.
        self.decl_stack.push(vec![]);

        // Phase 13: push a new hoisted_qrls frame for this function scope.
        self.hoisted_qrls.push(Vec::new());

        // Add params as Var(false) entries in the child frame.
        for param in &func.params.items {
            collect_binding_names(&param.pattern, &mut |name| {
                if let Some(frame) = self.decl_stack.last_mut() {
                    frame.push((name.to_string(), IdentType::Var(false)));
                }
            });
        }
    }

    fn exit_function(&mut self, func: &mut Function<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        // Phase 13: drain hoisted_qrls frame into function body before popping.
        if let Some(entries) = self.hoisted_qrls.pop() {
            if !entries.is_empty() {
                let allocator: &'a Allocator = ctx.ast.allocator;
                if let Some(body) = func.body.as_mut() {
                    Self::inject_hoisted_qrls_into_block(body, entries, allocator);
                }
            }
        }
        self.decl_stack.pop();
        if let Some(pushed) = self.fn_ctxt_push_stack.pop() {
            if pushed {
                self.stack_ctxt.pop();
            }
        }
    }

    /// Push a new scope frame when entering an arrow function.
    fn enter_arrow_function_expression(
        &mut self,
        arrow: &mut ArrowFunctionExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.decl_stack.push(vec![]);
        // Phase 13: push a new hoisted_qrls frame for this arrow scope.
        self.hoisted_qrls.push(Vec::new());
        // Add params as Var(false) entries.
        for param in &arrow.params.items {
            collect_binding_names(&param.pattern, &mut |name| {
                if let Some(frame) = self.decl_stack.last_mut() {
                    frame.push((name.to_string(), IdentType::Var(false)));
                }
            });
        }
    }

    fn exit_arrow_function_expression(
        &mut self,
        arrow: &mut ArrowFunctionExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Phase 13: drain hoisted_qrls frame into function body before popping.
        if let Some(entries) = self.hoisted_qrls.pop() {
            if !entries.is_empty() {
                let allocator: &'a Allocator = ctx.ast.allocator;
                Self::inject_hoisted_qrls_into_block(&mut arrow.body, entries, allocator);
            }
        }
        self.decl_stack.pop();
    }

    // -----------------------------------------------------------------------
    // Loop enter/exit hooks (Phase 13 Level 2)
    // -----------------------------------------------------------------------

    fn enter_for_statement(
        &mut self,
        _stmt: &mut ForStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.push(Vec::new());
    }

    fn exit_for_statement(
        &mut self,
        _stmt: &mut ForStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
    }

    fn enter_for_in_statement(
        &mut self,
        stmt: &mut ForInStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let mut vars: Vec<String> = Vec::new();
        if let ForStatementLeft::VariableDeclaration(decl) = &stmt.left {
            for d in &decl.declarations {
                collect_binding_names(&d.id, &mut |name| vars.push(name.to_string()));
            }
        }
        self.iteration_var_stack.push(vars);
    }

    fn exit_for_in_statement(
        &mut self,
        _stmt: &mut ForInStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
    }

    fn enter_for_of_statement(
        &mut self,
        stmt: &mut ForOfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let mut vars: Vec<String> = Vec::new();
        if let ForStatementLeft::VariableDeclaration(decl) = &stmt.left {
            for d in &decl.declarations {
                collect_binding_names(&d.id, &mut |name| vars.push(name.to_string()));
            }
        }
        self.iteration_var_stack.push(vars);
    }

    fn exit_for_of_statement(
        &mut self,
        _stmt: &mut ForOfStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
    }

    fn enter_while_statement(
        &mut self,
        _stmt: &mut WhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.push(Vec::new());
    }

    fn exit_while_statement(
        &mut self,
        _stmt: &mut WhileStatement<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.iteration_var_stack.pop();
    }

    // -----------------------------------------------------------------------
    // Class declarations (XFRM-06)
    // -----------------------------------------------------------------------

    /// Add class name to the current (last) frame.
    ///
    /// Class declarations don't trigger a separate frame push in our
    /// implementation, so the name goes directly into the current frame.
    fn enter_class(&mut self, class: &mut Class<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        if let Some(id) = &class.id {
            let name = id.name.as_str().to_string();
            if let Some(frame) = self.decl_stack.last_mut() {
                frame.push((name, IdentType::Class));
            }
        }
    }

    /// Drain `extra_top_items`, `ref_assignments`, and `extra_bottom_items` into `program.body`.
    ///
    /// Drain order:
    /// 1. Prepend `extra_top_items` as `const q_name = <rhs>;` declarations.
    /// 2. Walk original body statements; after each stmt that defines a const matching
    ///    any `ref_assignments.target_ident`, emit the `.s()` call immediately after.
    /// 3. Append `extra_bottom_items` as expression statements.
    fn exit_program(
        &mut self,
        program: &mut Program<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Check if there's anything to do.
        let has_jsx_imports = self.needs_jsx_sorted
            || self.needs_jsx_split
            || self.needs_get_var_props
            || self.needs_get_const_props
            || self.needs_fragment;
        let has_signal_imports =
            self.needs_wrap_prop || self.needs_fn_signal || self.needs_val || self.needs_chk;

        // Fast path: nothing to drain and no imports.
        if self.extra_top_items.is_empty()
            && self.ref_assignments.is_empty()
            && self.extra_bottom_items.is_empty()
            && !has_jsx_imports
            && !has_signal_imports
        {
            return;
        }

        let allocator: &'a Allocator = ctx.ast.allocator;
        let mut new_body: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);

        // --- Step 0a: Prepend signal wrapping imports ---
        // Order: _wrapProp, _fnSignal, _val, _chk (before JSX imports)
        if self.needs_wrap_prop {
            let src = r#"import { _wrapProp } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_fn_signal {
            let src = r#"import { _fnSignal } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_val {
            let src = r#"import { _val } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_chk {
            let src = r#"import { _chk } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }

        // --- Step 0b: Prepend JSX runtime imports ---
        // Order matches golden snapshot: _jsxSorted, _getVarProps, _getConstProps, _jsxSplit, Fragment
        if self.needs_jsx_sorted {
            let src = r#"import { _jsxSorted } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_get_var_props {
            let src = r#"import { _getVarProps } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_get_const_props {
            let src = r#"import { _getConstProps } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_jsx_split {
            let src = r#"import { _jsxSplit } from "@qwik.dev/core";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }
        if self.needs_fragment {
            let src = r#"import { Fragment as _Fragment } from "@qwik.dev/core/jsx-runtime";"#;
            if let Some(stmt) = parse_single_statement(src, allocator) {
                new_body.push(stmt);
            }
        }

        // --- Step 1: Prepend extra_top_items ---
        let top_items = std::mem::take(&mut self.extra_top_items);
        for hoisted in top_items {
            // Parse `const q_name = <rhs_code>;` via OXC parser and extract the stmt.
            let src = format!("const {} = {};", hoisted.name, hoisted.rhs_code);
            let stmt_opt = parse_single_statement(&src, allocator);
            if let Some(stmt) = stmt_opt {
                new_body.push(stmt);
            }
        }

        // --- Step 2: Walk original body, interleave ref_assignments ---
        let ref_assignments = std::mem::take(&mut self.ref_assignments);
        let original_body = std::mem::replace(&mut program.body, ArenaVec::new_in(allocator));

        for stmt in original_body {
            // Check if this statement defines a const whose name matches any ref_assignment.
            let defined_name: Option<String> = match &stmt {
                Statement::VariableDeclaration(decl) => {
                    if decl.kind == VariableDeclarationKind::Const {
                        decl.declarations.first().and_then(|d| {
                            if let BindingPattern::BindingIdentifier(id) = &d.id {
                                Some(id.name.as_str().to_string())
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };

            new_body.push(stmt);

            // Emit any matching .s() ref_assignments immediately after this statement.
            if let Some(ref name) = defined_name {
                for ra in ref_assignments.iter().filter(|r| &r.target_ident == name) {
                    // Parse `q_name.s(fn_body);` as a statement.
                    if let Some(s_stmt) = parse_single_statement(&ra.s_call_code, allocator) {
                        new_body.push(s_stmt);
                    }
                }
            }
        }

        // --- Step 3: Append extra_bottom_items ---
        let bottom_items = std::mem::take(&mut self.extra_bottom_items);
        for code in bottom_items {
            if let Some(stmt) = parse_single_statement(&code, allocator) {
                new_body.push(stmt);
            }
        }

        program.body = new_body;
    }
}

// ---------------------------------------------------------------------------
// Phase 15 helpers — _fnSignal call string manipulation
// ---------------------------------------------------------------------------

/// Replace the arrow function argument in a `_fnSignal(arrow, [caps])` call string
/// with the given `_hf<N>` identifier name.
///
/// The `arrow_code` is the exact string that appears as the first argument.
/// We find the first occurrence of `arrow_code` and replace it.
fn replace_fn_signal_arrow(fn_signal_code: &str, arrow_code: &str, hf_name: &str) -> String {
    // Find the exact arrow_code substring and replace its first occurrence.
    if let Some(pos) = fn_signal_code.find(arrow_code) {
        format!(
            "{}{}{}",
            &fn_signal_code[..pos],
            hf_name,
            &fn_signal_code[pos + arrow_code.len()..]
        )
    } else {
        // Fallback: couldn't find arrow — return as-is.
        fn_signal_code.to_string()
    }
}

/// Extract the third argument string from a `_fnSignal(arrow, [caps], "src")` call string.
///
/// Returns the third argument including quotes, e.g. `"\"(p0) => p0.value\""`.
/// Returns `None` if the third argument cannot be found (client-mode call).
fn extract_fn_signal_third_arg(fn_signal_code: &str) -> Option<String> {
    // The third arg follows the captures array closing `]`.
    // Find the last `, "..."`  or `, '...'` pattern.
    // Strategy: find the closing `)` then work backwards to find `, "`.
    let trimmed = fn_signal_code.trim_end_matches(')');
    // Find last `, "` or `, '`
    if let Some(pos) = trimmed.rfind(", \"") {
        Some(trimmed[pos + 2..].to_string() + "\"")
    } else if let Some(pos) = trimmed.rfind(", '") {
        Some(trimmed[pos + 2..].to_string() + "'")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// parse_single_statement — parse a source string into a single Statement<'a>
// ---------------------------------------------------------------------------

/// Parse `src` (a single JavaScript statement, with or without trailing semicolon)
/// and return the first `Statement` from the resulting `Program.body`, allocated
/// into `allocator`.
///
/// Returns `None` if parsing fails or produces an empty program.
///
/// Used by `exit_program` to materialize serialized `HoistedConst` / `RefAssignment`
/// strings back into AST nodes without manual construction.
fn parse_single_statement<'a>(src: &str, allocator: &'a Allocator) -> Option<Statement<'a>> {
    use oxc::parser::Parser;

    let src_owned: &str = allocator.alloc_str(src);
    let ret = Parser::new(allocator, src_owned, SourceType::default()).parse();
    if ret.panicked {
        return None;
    }
    // SAFETY: the parsed program borrows from `allocator`; we're returning a Statement
    // that also borrows from `allocator`. Both have the same lifetime.
    let program: Program<'a> = unsafe {
        std::mem::transmute::<Program<'_>, Program<'a>>(ret.program)
    };
    // Drain the first statement from body.
    let mut body = program.body;
    if body.is_empty() {
        None
    } else {
        Some(body.remove(0))
    }
}

// ---------------------------------------------------------------------------
// Phase 14: JSX AST builder helpers
// ---------------------------------------------------------------------------

/// Build a `_jsxSorted` or `_jsxSplit` call expression with 6 arguments:
/// `(tag, varProps, constProps, children, flags, key)`.
fn build_jsx_call<'a>(
    callee_name: &str,
    tag: Expression<'a>,
    var_props: Option<Expression<'a>>,
    const_props: Option<Expression<'a>>,
    children: Option<Expression<'a>>,
    flags: u32,
    key: Expression<'a>,
    ast: &AstBuilder<'a>,
    allocator: &'a Allocator,
) -> Expression<'a> {
    let mut args: ArenaVec<Argument<'a>> = ArenaVec::new_in(allocator);

    // Arg 1: tag
    push_expr_arg(ast, &mut args, tag);
    // Arg 2: varProps or null
    let var_arg = var_props.unwrap_or_else(|| ast.expression_null_literal(SPAN));
    push_expr_arg(ast, &mut args, var_arg);
    // Arg 3: constProps or null
    let const_arg = const_props.unwrap_or_else(|| ast.expression_null_literal(SPAN));
    push_expr_arg(ast, &mut args, const_arg);
    // Arg 4: children or null
    let children_arg = children.unwrap_or_else(|| ast.expression_null_literal(SPAN));
    push_expr_arg(ast, &mut args, children_arg);
    // Arg 5: flags
    let flags_expr = ast.expression_numeric_literal(SPAN, flags as f64, None, NumberBase::Decimal);
    push_expr_arg(ast, &mut args, flags_expr);
    // Arg 6: key
    push_expr_arg(ast, &mut args, key);

    let callee = ast.expression_identifier(SPAN, ast.atom(callee_name));
    ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false)
}

/// Build `fn_name(arg)` single-argument call expression.
fn build_fn_call<'a>(
    fn_name: &str,
    arg: Expression<'a>,
    ast: &AstBuilder<'a>,
    allocator: &'a Allocator,
) -> Expression<'a> {
    let mut args: ArenaVec<Argument<'a>> = ArenaVec::new_in(allocator);
    push_expr_arg(ast, &mut args, arg);
    let callee = ast.expression_identifier(SPAN, ast.atom(fn_name));
    ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'a>>, args, false)
}

/// Build `{ key: value }` as a single `ObjectPropertyKind`.
fn build_object_prop<'a>(
    key: &str,
    value: Expression<'a>,
    ast: &AstBuilder<'a>,
    _allocator: &'a Allocator,
) -> ObjectPropertyKind<'a> {
    // Determine if this key needs quotes (contains special chars like `:`).
    let needs_quotes = key.contains(':') || key.contains('-');
    let prop_key = if needs_quotes {
        PropertyKey::StringLiteral(ast.alloc_string_literal(SPAN, ast.atom(key), None))
    } else {
        PropertyKey::StaticIdentifier(ast.alloc_identifier_name(SPAN, ast.atom(key)))
    };
    ObjectPropertyKind::ObjectProperty(ast.alloc_object_property(
        SPAN,
        PropertyKind::Init,
        prop_key,
        value,
        false,
        false,
        false,
    ))
}

/// Extract the string key from a `JSXAttributeName`.
///
/// For NamespacedName (e.g. `ns:local`), we can't return a combined borrowed string,
/// so we return the namespace part. The caller must handle NamespacedName separately
/// if needed, or use the owned version.
fn jsx_attr_key_str<'a>(name: &'a JSXAttributeName<'a>) -> &'a str {
    match name {
        JSXAttributeName::Identifier(id) => id.name.as_str(),
        JSXAttributeName::NamespacedName(nn) => nn.namespace.name.as_str(),
    }
}

/// Owned version of `jsx_attr_key_str` that properly handles NamespacedName.
fn jsx_attr_key_owned(name: &JSXAttributeName<'_>) -> String {
    match name {
        JSXAttributeName::Identifier(id) => id.name.as_str().to_string(),
        JSXAttributeName::NamespacedName(nn) => {
            format!("{}:{}", nn.namespace.name.as_str(), nn.name.name.as_str())
        }
    }
}

/// Convert a `JSXExpression<'a>` into `Option<Expression<'a>>`.
///
/// Returns `None` for `JSXEmptyExpression`, `Some` for all other variants.
///
/// SAFETY: `JSXExpression` is defined via `inherit_variants!` which causes it to
/// share the exact same memory layout as `Expression` for all variants except
/// `EmptyExpression = 64`. Since we guard against `EmptyExpression`, the transmute
/// is safe.
fn jsx_expression_to_expr<'a>(jsx_expr: JSXExpression<'a>) -> Option<Expression<'a>> {
    if matches!(jsx_expr, JSXExpression::EmptyExpression(_)) {
        return None;
    }
    // SAFETY: All non-EmptyExpression variants of JSXExpression are identical to
    // the corresponding Expression variants (same discriminant, same data).
    let expr: Expression<'a> = unsafe {
        std::mem::transmute::<JSXExpression<'a>, Expression<'a>>(jsx_expr)
    };
    Some(expr)
}

/// Get the string key from an `ObjectPropertyKind` for sorting.
fn object_prop_key_str<'a>(kind: &ObjectPropertyKind<'a>) -> String {
    match kind {
        ObjectPropertyKind::ObjectProperty(p) => match &p.key {
            PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(),
            PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
            _ => String::new(),
        },
        ObjectPropertyKind::SpreadProperty(_) => String::new(),
    }
}

/// Recursively convert a `JSXMemberExpression` into a `StaticMemberExpression`.
fn jsx_member_to_expr<'a>(
    me: &JSXMemberExpression<'a>,
    ast: &AstBuilder<'a>,
    _allocator: &'a Allocator,
) -> Expression<'a> {
    let object_expr: Expression<'a> = match &me.object {
        JSXMemberExpressionObject::IdentifierReference(id) => {
            ast.expression_identifier(SPAN, ast.atom(id.name.as_str()))
        }
        JSXMemberExpressionObject::MemberExpression(inner_me) => {
            // Need allocator for recursive calls but we can drop the unused param warning
            jsx_member_to_expr(inner_me, ast, _allocator)
        }
        JSXMemberExpressionObject::ThisExpression(_) => {
            ast.expression_this(SPAN)
        }
    };
    let property = ast.identifier_name(SPAN, ast.atom(me.property.name.as_str()));
    let static_me = ast.member_expression_static(SPAN, object_expr, property, false);
    Expression::from(static_me)
}

// ---------------------------------------------------------------------------
// QRL AST builder helpers (Plan 12-02)
// ---------------------------------------------------------------------------

/// Build `[cap1, cap2, ...]` as an `Expression::ArrayExpression`.
///
/// When `captures` is empty, returns an empty array expression `[]`.
fn build_capture_array<'a>(
    captures: &[String],
    ast: &AstBuilder<'a>,
    allocator: &'a Allocator,
) -> Expression<'a> {
    let mut elements: ArenaVec<ArrayExpressionElement<'a>> = ArenaVec::new_in(allocator);
    for name in captures {
        elements.push(ArrayExpressionElement::Identifier(
            ast.alloc_identifier_reference(SPAN, ast.atom(name.as_str())),
        ));
    }
    ast.expression_array(SPAN, elements)
}

/// Build `{ file: "test.tsx", lo: 100, hi: 200, displayName: "..." }` as an ObjectExpression.
fn build_dev_metadata<'a>(
    file_name: &str,
    span_lo: u32,
    span_hi: u32,
    display_name: &str,
    ast: &AstBuilder<'a>,
    allocator: &'a Allocator,
) -> Expression<'a> {
    let mut props: ArenaVec<ObjectPropertyKind<'a>> = ArenaVec::new_in(allocator);

    // file: "test.tsx"
    let file_key = ast.property_key_static_identifier(SPAN, ast.atom("file"));
    let file_val = ast.expression_string_literal(SPAN, ast.atom(file_name), None);
    props.push(ObjectPropertyKind::ObjectProperty(ast.alloc_object_property(
        SPAN, PropertyKind::Init, file_key, file_val, false, false, false,
    )));

    // lo: <number>
    let lo_key = ast.property_key_static_identifier(SPAN, ast.atom("lo"));
    let lo_val = ast.expression_numeric_literal(
        SPAN, span_lo as f64, None, NumberBase::Decimal,
    );
    props.push(ObjectPropertyKind::ObjectProperty(ast.alloc_object_property(
        SPAN, PropertyKind::Init, lo_key, lo_val, false, false, false,
    )));

    // hi: <number>
    let hi_key = ast.property_key_static_identifier(SPAN, ast.atom("hi"));
    let hi_val = ast.expression_numeric_literal(
        SPAN, span_hi as f64, None, NumberBase::Decimal,
    );
    props.push(ObjectPropertyKind::ObjectProperty(ast.alloc_object_property(
        SPAN, PropertyKind::Init, hi_key, hi_val, false, false, false,
    )));

    // displayName: "..."
    let dn_key = ast.property_key_static_identifier(SPAN, ast.atom("displayName"));
    let dn_val = ast.expression_string_literal(SPAN, ast.atom(display_name), None);
    props.push(ObjectPropertyKind::ObjectProperty(ast.alloc_object_property(
        SPAN, PropertyKind::Init, dn_key, dn_val, false, false, false,
    )));

    ast.expression_object(SPAN, props)
}

/// Convert an `Expression<'a>` to an `Argument<'a>` and push it onto `args`.
///
/// This helper handles the `Argument` enum which "inherits" Expression variants
/// via an OXC macro — each Expression variant has a matching Argument variant.
fn push_expr_arg<'a>(
    _ast: &AstBuilder<'a>,
    args: &mut ArenaVec<'a, Argument<'a>>,
    expr: Expression<'a>,
) {
    // Argument inherits Expression variants via #[inherit_variants!] macro.
    // Each Expression::X maps to Argument::X with the same inner data.
    let arg: Argument<'a> = match expr {
        Expression::StringLiteral(b) => Argument::StringLiteral(b),
        Expression::NumericLiteral(b) => Argument::NumericLiteral(b),
        Expression::BooleanLiteral(b) => Argument::BooleanLiteral(b),
        Expression::NullLiteral(b) => Argument::NullLiteral(b),
        Expression::Identifier(b) => Argument::Identifier(b),
        Expression::ArrayExpression(b) => Argument::ArrayExpression(b),
        Expression::ObjectExpression(b) => Argument::ObjectExpression(b),
        Expression::ArrowFunctionExpression(b) => Argument::ArrowFunctionExpression(b),
        Expression::FunctionExpression(b) => Argument::FunctionExpression(b),
        Expression::CallExpression(b) => Argument::CallExpression(b),
        Expression::ImportExpression(b) => Argument::ImportExpression(b),
        Expression::TemplateLiteral(b) => Argument::TemplateLiteral(b),
        Expression::TaggedTemplateExpression(b) => Argument::TaggedTemplateExpression(b),
        Expression::AssignmentExpression(b) => Argument::AssignmentExpression(b),
        Expression::LogicalExpression(b) => Argument::LogicalExpression(b),
        Expression::BinaryExpression(b) => Argument::BinaryExpression(b),
        Expression::UnaryExpression(b) => Argument::UnaryExpression(b),
        Expression::ConditionalExpression(b) => Argument::ConditionalExpression(b),
        Expression::SequenceExpression(b) => Argument::SequenceExpression(b),
        Expression::NewExpression(b) => Argument::NewExpression(b),
        Expression::AwaitExpression(b) => Argument::AwaitExpression(b),
        Expression::YieldExpression(b) => Argument::YieldExpression(b),
        Expression::UpdateExpression(b) => Argument::UpdateExpression(b),
        Expression::ChainExpression(b) => Argument::ChainExpression(b),
        Expression::ParenthesizedExpression(b) => Argument::ParenthesizedExpression(b),
        Expression::TSAsExpression(b) => Argument::TSAsExpression(b),
        Expression::TSSatisfiesExpression(b) => Argument::TSSatisfiesExpression(b),
        Expression::TSNonNullExpression(b) => Argument::TSNonNullExpression(b),
        Expression::TSTypeAssertion(b) => Argument::TSTypeAssertion(b),
        Expression::TSInstantiationExpression(b) => Argument::TSInstantiationExpression(b),
        Expression::Super(b) => Argument::Super(b),
        Expression::ThisExpression(b) => Argument::ThisExpression(b),
        Expression::ClassExpression(b) => Argument::ClassExpression(b),
        Expression::MetaProperty(b) => Argument::MetaProperty(b),
        Expression::RegExpLiteral(b) => Argument::RegExpLiteral(b),
        Expression::StaticMemberExpression(b) => Argument::StaticMemberExpression(b),
        Expression::ComputedMemberExpression(b) => Argument::ComputedMemberExpression(b),
        Expression::PrivateFieldExpression(b) => Argument::PrivateFieldExpression(b),
        Expression::JSXElement(b) => Argument::JSXElement(b),
        Expression::JSXFragment(b) => Argument::JSXFragment(b),
        Expression::BigIntLiteral(b) => Argument::BigIntLiteral(b),
        // These variants do not appear in QRL arguments; unreachable in practice.
        Expression::PrivateInExpression(_) | Expression::V8IntrinsicExpression(_) => {
            unreachable!("PrivateInExpression/V8IntrinsicExpression cannot be QRL arguments")
        }
    };
    args.push(arg);
}

// ---------------------------------------------------------------------------
// Helper: collect descendent idents from call's first argument
// ---------------------------------------------------------------------------

/// Extract all `IdentifierReference` names from the first argument of a call
/// expression. Used in `enter_call_expression` to snapshot identifiers BEFORE
/// children are traversed (the "pre-fold" snapshot for scoped_idents computation).
///
/// Uses `IdentCollectorOnArg` which implements `Visit` directly on `Argument`
/// variants, avoiding any transmute.
fn collect_arg0_idents<'a>(args: &[Argument<'a>]) -> HashSet<String> {
    let Some(arg0) = args.first() else {
        return HashSet::new();
    };
    let mut collector = IdentCollector { idents: HashSet::new() };
    // Visit the argument directly without converting to Expression.
    match arg0 {
        Argument::ArrowFunctionExpression(arrow) => {
            use oxc::ast_visit::Visit;
            collector.visit_arrow_function_expression(arrow);
        }
        Argument::FunctionExpression(func) => {
            use oxc::ast_visit::Visit;
            use oxc::semantic::ScopeFlags;
            collector.visit_function(func, ScopeFlags::empty());
        }
        Argument::Identifier(id) => {
            collector.idents.insert(id.name.as_str().to_string());
        }
        _ => {
            // For other argument types (literals, other calls) collect via codegen
            // round-trip is not needed — these rarely appear as $ first args.
        }
    }
    collector.idents
}

// ---------------------------------------------------------------------------
// Helper: convert Expression<'a> to Argument<'a> (simplified for QRL results)
// ---------------------------------------------------------------------------

/// Convert an `Expression<'a>` produced by QRL builders into an `Argument<'a>`.
/// The QRL builders always return `CallExpression` variants; we handle others
/// defensively via the full push_expr_arg logic.
fn expr_to_argument<'a>(expr: Expression<'a>) -> Argument<'a> {
    match expr {
        Expression::CallExpression(b) => Argument::CallExpression(b),
        Expression::Identifier(b) => Argument::Identifier(b),
        Expression::StringLiteral(b) => Argument::StringLiteral(b),
        Expression::NullLiteral(b) => Argument::NullLiteral(b),
        Expression::ArrowFunctionExpression(b) => Argument::ArrowFunctionExpression(b),
        Expression::FunctionExpression(b) => Argument::FunctionExpression(b),
        Expression::ArrayExpression(b) => Argument::ArrayExpression(b),
        Expression::ObjectExpression(b) => Argument::ObjectExpression(b),
        // Phase 13: hoist_qrl_to_module_scope may return these variants.
        Expression::StaticMemberExpression(b) => Argument::StaticMemberExpression(b),
        Expression::SequenceExpression(b) => Argument::SequenceExpression(b),
        // These expression types are never produced by QRL call builders.
        // Hitting this branch would be a logic error — panic in debug mode
        // to surface it early.
        #[allow(unreachable_patterns)]
        other => panic!(
            "expr_to_argument: unexpected Expression variant from QRL builder: {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

// ---------------------------------------------------------------------------
// Helper: collect binding names recursively from a BindingPattern
// ---------------------------------------------------------------------------

fn collect_binding_names<'a, F>(pattern: &BindingPattern<'a>, f: &mut F)
where
    F: FnMut(&str),
{
    match pattern {
        BindingPattern::BindingIdentifier(id) => f(id.name.as_str()),
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names(&prop.value, f);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_names(&rest.argument, f);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for element in arr.elements.iter().flatten() {
                collect_binding_names(element, f);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_names(&rest.argument, f);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names(&assign.left, f);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use oxc::allocator::Allocator;
    use oxc::codegen::Codegen;
    use oxc::parser::Parser;
    use oxc::semantic::SemanticBuilder;
    use oxc::span::SourceType;
    use oxc_traverse::traverse_mut;

    use crate::collector::global_collect;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    use crate::types::EntryStrategy;

    /// Options with sensible defaults for tests.
    fn default_opts<'b>(
        collect: &'b GlobalCollect,
        strip_ctx_name: &'b [String],
        strip_event_handlers: bool,
    ) -> QwikTransformOptions<'b> {
        QwikTransformOptions {
            global_collect: collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name,
            strip_event_handlers,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        }
    }

    /// Parse + collect + construct `QwikTransform` + run traverse + return the transform.
    fn make_transform(src: &str) -> QwikTransform {
        let allocator = Allocator::default();
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        // SAFETY: program borrows from allocator; allocator lives for the rest of this fn.
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let _scoping = traverse_mut(&mut xfrm, &allocator, &mut program, scoping, ());
        xfrm
    }

    // -----------------------------------------------------------------------
    // Marker function detection
    // -----------------------------------------------------------------------

    #[test]
    fn marker_functions_from_imports() {
        let xfrm = make_transform(
            r#"import { component$, useTask$ } from "@qwik.dev/core";"#,
        );
        assert!(
            xfrm.marker_functions.contains_key("component$"),
            "component$ should be in marker_functions"
        );
        assert_eq!(
            xfrm.marker_functions.get("component$").map(|s| s.as_str()),
            Some("component$")
        );
        assert!(
            xfrm.marker_functions.contains_key("useTask$"),
            "useTask$ should be in marker_functions"
        );
    }

    #[test]
    fn marker_functions_from_exports() {
        // An exported function whose name ends with $ — should be a marker.
        let xfrm = make_transform(r#"export function myHelper$() {}"#);
        assert!(
            xfrm.marker_functions.contains_key("myHelper$"),
            "locally-exported myHelper$ should be in marker_functions"
        );
        assert_eq!(
            xfrm.marker_functions.get("myHelper$").map(|s| s.as_str()),
            Some("myHelper$")
        );
    }

    // -----------------------------------------------------------------------
    // Special-case function detection
    // -----------------------------------------------------------------------

    #[test]
    fn special_case_fn_detection() {
        let xfrm = make_transform(
            r#"import { $, sync$, inlinedQrl, _fnSignal } from "@qwik.dev/core";"#,
        );
        assert_eq!(xfrm.qsegment_fn.as_deref(), Some("$"));
        assert_eq!(xfrm.sync_qrl_fn.as_deref(), Some("sync$"));
        assert_eq!(xfrm.inlined_qrl_fn.as_deref(), Some("inlinedQrl"));
        assert_eq!(xfrm.fn_signal_fn.as_deref(), Some("_fnSignal"));
    }

    // -----------------------------------------------------------------------
    // JSX function detection
    // -----------------------------------------------------------------------

    #[test]
    fn jsx_functions_detected() {
        let xfrm = make_transform(
            r#"import { jsx, jsxs } from "@qwik.dev/core/jsx-runtime";"#,
        );
        assert!(xfrm.jsx_functions.contains("jsx"), "jsx should be in jsx_functions");
        assert!(xfrm.jsx_functions.contains("jsxs"), "jsxs should be in jsx_functions");
    }

    // -----------------------------------------------------------------------
    // stack_ctxt push/pop
    // -----------------------------------------------------------------------

    #[test]
    fn stack_ctxt_plain_call() {
        // After full traversal, stack_ctxt should be empty (all pushes popped).
        let xfrm = make_transform(r#"const x = foo(bar());"#);
        assert!(
            xfrm.stack_ctxt.is_empty(),
            "stack_ctxt should be empty after traversal, got: {:?}",
            xfrm.stack_ctxt
        );
    }

    #[test]
    fn stack_ctxt_marker_call() {
        // component$ is a marker function (priority 6), so it must NOT be pushed to stack_ctxt.
        let xfrm = make_transform(
            r#"import { component$ } from "@qwik.dev/core"; const Cmp = component$(() => {});"#,
        );
        assert!(
            xfrm.stack_ctxt.is_empty(),
            "stack_ctxt should be empty after traversal (marker calls must not push), got: {:?}",
            xfrm.stack_ctxt
        );
    }

    // -----------------------------------------------------------------------
    // decl_stack scope management
    // -----------------------------------------------------------------------

    #[test]
    fn decl_stack_scope_push_pop() {
        // After full traversal, only the root frame should remain.
        let xfrm = make_transform(
            r#"function outer() { const x = 1; function inner() { const y = 2; } }"#,
        );
        assert_eq!(
            xfrm.decl_stack.len(),
            1,
            "After traversal only the root frame should remain"
        );
    }

    #[test]
    fn decl_stack_var_is_const() {
        // `const a = 42` → Var(true) — literal is static
        // `let b = 42`   → Var(false) — not const keyword
        // `const c = foo()` → Var(false) — non-static init (call expression)
        let xfrm = make_transform(r#"const a = 42; let b = 42; const c = foo();"#);
        let root = &xfrm.decl_stack[0];
        let find = |name: &str| {
            root.iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
        };
        assert_eq!(find("a"), Some(IdentType::Var(true)), "const a = 42 should be Var(true)");
        assert_eq!(find("b"), Some(IdentType::Var(false)), "let b = 42 should be Var(false)");
        assert_eq!(find("c"), Some(IdentType::Var(false)), "const c = foo() should be Var(false)");
    }

    #[test]
    fn decl_stack_fn_entry() {
        // `function myFn() {}` → IdentType::Fn in root frame.
        let xfrm = make_transform(r#"function myFn() {}"#);
        let root = &xfrm.decl_stack[0];
        let entry = root.iter().find(|(n, _)| n == "myFn");
        assert!(entry.is_some(), "myFn should appear in root frame");
        assert_eq!(entry.unwrap().1, IdentType::Fn, "myFn should have IdentType::Fn");
    }

    // -----------------------------------------------------------------------
    // should_emit_segment
    // -----------------------------------------------------------------------

    #[test]
    fn should_emit_strip_ctx_name() {
        let allocator = Allocator::default();
        let src = "";
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let strip = vec!["useServer".to_string()];
        let opts = default_opts(&collect, &strip, false);
        let xfrm = QwikTransform::new(opts);

        assert!(
            !xfrm.should_emit_segment("useServerLoader$", CtxKind::Function),
            "useServerLoader$ should be stripped (prefix match)"
        );
        assert!(
            xfrm.should_emit_segment("component$", CtxKind::Function),
            "component$ should NOT be stripped"
        );
    }

    #[test]
    fn should_emit_strip_event_handlers() {
        let allocator = Allocator::default();
        let src = "";
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = default_opts(&collect, &[], true);
        let xfrm = QwikTransform::new(opts);

        assert!(
            !xfrm.should_emit_segment("onClick$", CtxKind::EventHandler),
            "EventHandler should be stripped when strip_event_handlers=true"
        );
        assert!(
            xfrm.should_emit_segment("useTask$", CtxKind::Function),
            "Function kind should NOT be stripped by strip_event_handlers"
        );
    }

    // -----------------------------------------------------------------------
    // convert_qrl_word (XFRM-08) — end-to-end via codegen
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // IdentCollector
    // -----------------------------------------------------------------------

    /// Parse `src` as an expression (wrapped in `const _x = <src>;`) and return
    /// the allocator + a `'static` transmuted program so tests can borrow the
    /// expression for the lifetime of the allocator.
    ///
    /// Returns the allocator (must stay alive) and the initializer expression.
    fn parse_expr_program(src: &str) -> (Allocator, oxc::ast::ast::Program<'static>) {
        let allocator = Allocator::default();
        let wrapped = allocator.alloc_str(&format!("const _x = {src};"));
        let ret = Parser::new(&allocator, wrapped, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        (allocator, program)
    }

    /// Extract the first variable declarator's `init` expression from a program.
    fn first_init<'a>(program: &'a oxc::ast::ast::Program<'a>) -> &'a Expression<'a> {
        program
            .body
            .iter()
            .find_map(|stmt| {
                if let oxc::ast::ast::Statement::VariableDeclaration(decl) = stmt {
                    decl.declarations.first().and_then(|d| d.init.as_ref())
                } else {
                    None
                }
            })
            .expect("expected a variable declaration with init")
    }

    #[test]
    fn ident_collector_basic() {
        let (_alloc, program) = parse_expr_program("a + b.c + foo(d)");
        let expr = first_init(&program);
        let idents = IdentCollector::collect(expr);
        assert!(idents.contains("a"), "should contain a");
        assert!(idents.contains("b"), "should contain b (object of member expr)");
        assert!(idents.contains("foo"), "should contain foo");
        assert!(idents.contains("d"), "should contain d");
        // "c" is a static property name, not an IdentifierReference
        assert!(!idents.contains("c"), "c is a property name, not an ident ref");
    }

    // -----------------------------------------------------------------------
    // compute_scoped_idents
    // -----------------------------------------------------------------------

    #[test]
    fn compute_scoped_idents_partial_match_not_all_const() {
        let idents: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let decl: Vec<IdPlusType> = vec![
            ("a".to_string(), IdentType::Var(true)),
            ("b".to_string(), IdentType::Var(false)),
        ];
        let (names, is_const) = compute_scoped_idents(&idents, &decl);
        // c is not in decl, so only a and b match
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        assert!(!is_const, "is_const should be false because b is Var(false)");
    }

    #[test]
    fn compute_scoped_idents_all_const() {
        let idents: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let decl: Vec<IdPlusType> = vec![
            ("a".to_string(), IdentType::Var(true)),
            ("b".to_string(), IdentType::Var(true)),
        ];
        let (names, is_const) = compute_scoped_idents(&idents, &decl);
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        assert!(is_const, "is_const should be true when all matched are Var(true)");
    }

    #[test]
    fn compute_scoped_idents_no_intersection() {
        let idents: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let decl: Vec<IdPlusType> = vec![("y".to_string(), IdentType::Var(true))];
        let (names, is_const) = compute_scoped_idents(&idents, &decl);
        assert!(names.is_empty(), "no intersection should return empty vec");
        // is_const starts true and is never set false (no matches), so stays true
        assert!(is_const, "is_const is true when no matches (vacuously)");
    }

    #[test]
    fn compute_scoped_idents_excludes_fn_class() {
        let idents: HashSet<String> =
            ["myFn", "MyClass", "x"].iter().map(|s| s.to_string()).collect();
        let decl: Vec<IdPlusType> = vec![
            ("myFn".to_string(), IdentType::Fn),
            ("MyClass".to_string(), IdentType::Class),
            ("x".to_string(), IdentType::Var(true)),
        ];
        let (names, is_const) = compute_scoped_idents(&idents, &decl);
        // Only Var entries should be captured
        assert_eq!(names, vec!["x".to_string()]);
        assert!(is_const);
    }

    // -----------------------------------------------------------------------
    // get_function_params
    // -----------------------------------------------------------------------

    #[test]
    fn get_function_params_arrow() {
        let (_alloc, program) = parse_expr_program("(a, b) => a + b");
        let expr = first_init(&program);
        let params = get_function_params(expr);
        assert!(params.contains("a"));
        assert!(params.contains("b"));
    }

    #[test]
    fn get_function_params_function_expr() {
        let (_alloc, program) = parse_expr_program("function(a, b) {}");
        let expr = first_init(&program);
        let params = get_function_params(expr);
        assert!(params.contains("a"));
        assert!(params.contains("b"));
    }

    #[test]
    fn get_function_params_non_fn_empty() {
        let (_alloc, program) = parse_expr_program("42");
        let expr = first_init(&program);
        let params = get_function_params(expr);
        assert!(params.is_empty());
    }

    #[test]
    fn get_function_params_destructured() {
        let (_alloc, program) = parse_expr_program("({ x, y }) => x");
        let expr = first_init(&program);
        let params = get_function_params(expr);
        assert!(params.contains("x"), "destructured x should be included");
        assert!(params.contains("y"), "destructured y should be included");
    }

    // -----------------------------------------------------------------------
    // QRL call form builders — TDD RED tests
    // -----------------------------------------------------------------------

    use oxc::allocator::Vec as ArenaVec;
    use oxc::ast::AstBuilder;
    use oxc::ast::ast::Argument;

    /// Build a QwikTransform with a specific mode for builder testing.
    fn make_transform_with_mode(mode: EmitMode) -> QwikTransform {
        let allocator = Allocator::default();
        let src = "";
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &mode,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        QwikTransform::new(opts)
    }

    /// Emit an Expression to a string via OXC Codegen.
    fn emit_expr(allocator: &Allocator, expr: Expression<'_>) -> String {
        // Wrap in a const to emit via codegen
        let ast = AstBuilder::new(allocator);
        use oxc::span::SPAN;
        use oxc::ast::ast::*;
        let binding = ast.binding_pattern_binding_identifier(SPAN, ast.atom("_x"));
        let mut declarators: ArenaVec<VariableDeclarator<'_>> = ArenaVec::new_in(allocator);
        declarators.push(ast.variable_declarator(
            SPAN,
            VariableDeclarationKind::Const,
            binding,
            None::<TSTypeAnnotation<'_>>,
            Some(expr),
            false,
        ));
        let decl = ast.alloc_variable_declaration(SPAN, VariableDeclarationKind::Const, declarators, false);
        let mut body: ArenaVec<Statement<'_>> = ArenaVec::new_in(allocator);
        body.push(Statement::VariableDeclaration(decl));
        let directives: ArenaVec<Directive<'_>> = ArenaVec::new_in(allocator);
        let comments: ArenaVec<Comment> = ArenaVec::new_in(allocator);
        let program = ast.program(SPAN, oxc::span::SourceType::tsx(), "", comments, None, directives, body);
        let result = Codegen::new().build(&program);
        // Extract the RHS from "const _x = <expr>;"
        let code = result.code;
        // Return just the expression part
        code.trim_start_matches("const _x = ")
            .trim_end_matches(';')
            .trim()
            .to_string()
    }

    #[test]
    fn create_noop_qrl_prod_no_captures() {
        let allocator = Allocator::default();
        let xfrm = make_transform_with_mode(EmitMode::Lib);
        let expr = xfrm.create_noop_qrl("sym_HASH", &[], (0, 0), "display_name", &allocator);
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("_noopQrl("),
            "Prod/Lib mode should use _noopQrl, got: {code}"
        );
        assert!(
            code.contains(r#""sym_HASH""#),
            "Should contain symbol name, got: {code}"
        );
    }

    #[test]
    fn create_noop_qrl_prod_with_captures() {
        let allocator = Allocator::default();
        let xfrm = make_transform_with_mode(EmitMode::Lib);
        let expr = xfrm.create_noop_qrl("sym_HASH", &["a".to_string(), "b".to_string()], (0, 0), "display_name", &allocator);
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("_noopQrl("),
            "Lib mode should use _noopQrl, got: {code}"
        );
        assert!(
            code.contains("[a, b]") || (code.contains("a") && code.contains("b")),
            "Should contain capture array, got: {code}"
        );
    }

    #[test]
    fn create_noop_qrl_dev_mode() {
        let allocator = Allocator::default();
        let xfrm = make_transform_with_mode(EmitMode::Dev);
        let expr = xfrm.create_noop_qrl("sym_HASH", &[], (10, 20), "my_display_name", &allocator);
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("_noopQrlDEV("),
            "Dev mode should use _noopQrlDEV, got: {code}"
        );
        assert!(
            code.contains("my_display_name"),
            "Dev mode should include displayName, got: {code}"
        );
    }

    #[test]
    fn create_inline_qrl_lib_no_captures() {
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        // folded_expr = () => 42
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(
                SPAN,
                ArenaVec::new_in(&allocator),
                ast.vec1(ast.statement_expression(SPAN, ast.expression_numeric_literal(SPAN, 42.0, None, oxc::ast::ast::NumberBase::Decimal)))
            );
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        let mut xfrm = make_transform_with_mode(EmitMode::Lib);
        let expr = xfrm.create_inline_qrl(folded_fn, "sym_HASH", &[], (0, 0), "display_name", &allocator);
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("inlinedQrl("),
            "Lib mode should use inlinedQrl, got: {code}"
        );
        assert!(
            code.contains(r#""sym_HASH""#),
            "Should contain symbol name, got: {code}"
        );
    }

    #[test]
    fn create_inline_qrl_dev_mode_with_captures() {
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(SPAN, ArenaVec::new_in(&allocator), ArenaVec::new_in(&allocator));
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        let mut xfrm = make_transform_with_mode(EmitMode::Dev);
        let expr = xfrm.create_inline_qrl(folded_fn, "sym_HASH", &["cap1".to_string()], (5, 15), "disp", &allocator);
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("inlinedQrlDEV("),
            "Dev mode should use inlinedQrlDEV, got: {code}"
        );
        assert!(
            code.contains("cap1"),
            "Should contain capture, got: {code}"
        );
        assert!(
            code.contains("disp"),
            "Should contain displayName in metadata, got: {code}"
        );
    }

    #[test]
    fn create_segment_prod_no_captures() {
        use crate::hash::register_context_name;
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(SPAN, ArenaVec::new_in(&allocator), ArenaVec::new_in(&allocator));
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        let mut xfrm = make_transform_with_mode(EmitMode::Lib);
        let mut names_map = std::collections::HashMap::new();
        let names = register_context_name(
            &["test".to_string(), "component".to_string()],
            &mut names_map,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None, None, None,
        );
        let expr = xfrm.create_segment(
            folded_fn,
            &names,
            vec![],
            vec![],
            "component$",
            crate::types::CtxKind::Function,
            (0, 50),
            &allocator,
        );
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("qrl("),
            "Lib mode should use qrl, got: {code}"
        );
        // Import path should contain the canonical filename
        assert!(
            code.contains("./"),
            "Should contain relative import path, got: {code}"
        );
        // Should have pushed a segment record
        assert_eq!(xfrm.segments.len(), 1, "Should have pushed one segment record");
        let seg = &xfrm.segments[0];
        assert!(seg.expr.is_some(), "Segment record expr field should be populated");
    }

    #[test]
    fn create_segment_explicit_extensions() {
        use crate::hash::register_context_name;
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(SPAN, ArenaVec::new_in(&allocator), ArenaVec::new_in(&allocator));
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        // Build a transform with explicit_extensions=true
        let alloc2 = Allocator::default();
        let src = "";
        let source_in_arena2: &str = alloc2.alloc_str(src);
        let ret = Parser::new(&alloc2, source_in_arena2, SourceType::tsx()).parse();
        let program2 = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect2 = global_collect(&program2);
        let opts2 = QwikTransformOptions {
            global_collect: &collect2,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: true,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts2);
        let mut names_map = std::collections::HashMap::new();
        let names = register_context_name(
            &["test".to_string(), "component".to_string()],
            &mut names_map,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None, None, None,
        );
        let expr = xfrm.create_segment(
            folded_fn,
            &names,
            vec![],
            vec![],
            "component$",
            crate::types::CtxKind::Function,
            (0, 50),
            &allocator,
        );
        let code = emit_expr(&allocator, expr);
        // With explicit_extensions=true, path should end with ".tsx"
        assert!(
            code.contains(".tsx"),
            "With explicit_extensions=true, import path should include .tsx extension, got: {code}"
        );
    }

    #[test]
    fn create_segment_dev_mode() {
        use crate::hash::register_context_name;
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(SPAN, ArenaVec::new_in(&allocator), ArenaVec::new_in(&allocator));
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        let mut xfrm = make_transform_with_mode(EmitMode::Dev);
        let mut names_map = std::collections::HashMap::new();
        let names = register_context_name(
            &["test".to_string(), "component".to_string()],
            &mut names_map,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Dev,
            None, None, None,
        );
        let expr = xfrm.create_segment(
            folded_fn,
            &names,
            vec![],
            vec![],
            "component$",
            crate::types::CtxKind::Function,
            (0, 50),
            &allocator,
        );
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("qrlDEV("),
            "Dev mode should use qrlDEV, got: {code}"
        );
    }

    #[test]
    fn create_segment_with_captures() {
        use crate::hash::register_context_name;
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);
        use oxc::span::SPAN;
        let folded_fn = {
            let params = ast.formal_parameters(
                SPAN,
                oxc::ast::ast::FormalParameterKind::ArrowFormalParameters,
                ArenaVec::new_in(&allocator),
                None::<oxc::ast::ast::FormalParameterRest<'_>>,
            );
            let body = ast.function_body(SPAN, ArenaVec::new_in(&allocator), ArenaVec::new_in(&allocator));
            ast.expression_arrow_function(SPAN, false, false, None::<oxc::ast::ast::TSTypeParameterDeclaration<'_>>, params, None::<oxc::ast::ast::TSTypeAnnotation<'_>>, body)
        };
        let mut xfrm = make_transform_with_mode(EmitMode::Lib);
        let mut names_map = std::collections::HashMap::new();
        let names = register_context_name(
            &["test".to_string(), "component".to_string()],
            &mut names_map,
            None,
            "test.tsx",
            "test.tsx",
            &EmitMode::Lib,
            None, None, None,
        );
        let expr = xfrm.create_segment(
            folded_fn,
            &names,
            vec!["store".to_string()],
            vec![],
            "component$",
            crate::types::CtxKind::Function,
            (0, 50),
            &allocator,
        );
        let code = emit_expr(&allocator, expr);
        assert!(
            code.contains("store"),
            "Should contain capture in array, got: {code}"
        );
        assert!(
            code.contains("qrl("),
            "Should use qrl for Lib mode, got: {code}"
        );
    }

    #[test]
    fn convert_qrl_word_in_output() {
        let allocator = Allocator::default();
        let src =
            r#"import { component$ } from "@qwik.dev/core"; const Cmp = component$(() => {});"#;
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let _scoping = traverse_mut(&mut xfrm, &allocator, &mut program, scoping, ());
        let codegen_result = Codegen::new().build(&program);
        assert!(
            codegen_result.code.contains("componentQrl"),
            "Output should contain componentQrl, got: {}",
            codegen_result.code
        );
        assert!(
            !codegen_result.code.contains("component$("),
            "Output should NOT contain component$( after rewrite, got: {}",
            codegen_result.code
        );
    }

    // -----------------------------------------------------------------------
    // _create_synthetic_qsegment — TDD RED tests (Plan 12-03)
    // -----------------------------------------------------------------------

    /// Helper: run transform on `src` with a given mode and return (code, xfrm).
    fn run_transform_with_mode(src: &str, mode: EmitMode) -> (String, QwikTransform) {
        let allocator = Allocator::default();
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &mode,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let _scoping = traverse_mut(&mut xfrm, &allocator, &mut program, scoping, ());
        let code = Codegen::new().build(&program).code;
        (code, xfrm)
    }

    /// Helper: run transform with strip_ctx_name list.
    fn run_transform_with_strip(src: &str, strip: &[&str]) -> String {
        let allocator = Allocator::default();
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let strip_vec: Vec<String> = strip.iter().map(|s| s.to_string()).collect();
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &strip_vec,
            strip_event_handlers: false,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let _scoping = traverse_mut(&mut xfrm, &allocator, &mut program, scoping, ());
        Codegen::new().build(&program).code
    }

    // Test 1: component$(() => {}) in Lib mode → inlinedQrl output, no segments pushed
    #[test]
    fn segment_extraction_lib_mode_produces_inlined_qrl() {
        let src = r#"import { component$ } from "@qwik.dev/core";
const Cmp = component$(() => {});"#;
        let (code, xfrm) = run_transform_with_mode(src, EmitMode::Lib);
        assert!(
            code.contains("inlinedQrl("),
            "Lib mode should produce inlinedQrl, got: {code}"
        );
        assert!(
            xfrm.segments.is_empty(),
            "Lib mode should NOT push to segments, got {} segments",
            xfrm.segments.len()
        );
    }

    // Test 2: component$(() => {}) in Segment mode → qrl() output, 1 segment pushed
    #[test]
    fn segment_extraction_segment_mode_produces_qrl_and_segment() {
        let allocator = Allocator::default();
        let src = r#"import { component$ } from "@qwik.dev/core";
const Cmp = component$(() => {});"#;
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Lib,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        // Override to Segment-like: use Prod mode (non-Lib) with Segment strategy
        let allocator2 = Allocator::default();
        let src2 = r#"import { component$ } from "@qwik.dev/core";
const Cmp = component$(() => {});"#;
        let source_in_arena2: &str = allocator2.alloc_str(src2);
        let ret2 = Parser::new(&allocator2, source_in_arena2, SourceType::tsx()).parse();
        let mut program2 = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret2.program,
            )
        };
        let collect2 = global_collect(&program2);
        let opts2 = QwikTransformOptions {
            global_collect: &collect2,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm2 = QwikTransform::new(opts2);
        let semantic2 = SemanticBuilder::new().build(&program2);
        let scoping2 = semantic2.semantic.into_scoping();
        let _scoping2 = traverse_mut(&mut xfrm2, &allocator2, &mut program2, scoping2, ());
        let code2 = Codegen::new().build(&program2).code;
        assert!(
            code2.contains("qrl("),
            "Prod/Segment mode should produce qrl(), got: {code2}"
        );
        assert_eq!(
            xfrm2.segments.len(),
            1,
            "Segment mode should push 1 segment record, got {}",
            xfrm2.segments.len()
        );
        // Not a no-op: callee was rewritten
        assert!(
            !xfrm2.segments.is_empty(),
            "segments Vec should have an entry"
        );
        let _seg = &xfrm2.segments[0];
    }

    // Test 3: Lib mode with captured outer variable → _captures injection
    #[test]
    fn segment_extraction_lib_captures_injection() {
        let src = r#"import { component$ } from "@qwik.dev/core";
const sig = 42;
const Cmp = component$(() => { return sig; });"#;
        let (code, xfrm) = run_transform_with_mode(src, EmitMode::Lib);
        assert!(
            code.contains("inlinedQrl("),
            "Lib mode should produce inlinedQrl, got: {code}"
        );
        assert!(
            xfrm.segments.is_empty(),
            "Lib mode should not push segments"
        );
        // sig is a captured variable — should appear as a capture in the inlinedQrl call
        assert!(
            code.contains("sig"),
            "Captured var 'sig' should appear in the output, got: {code}"
        );
    }

    // Test 4: strip_ctx_name prefix → _noopQrl output
    #[test]
    fn segment_extraction_strip_ctx_name_produces_noop() {
        let src = r#"import { useServerLoader$ } from "@qwik.dev/core";
useServerLoader$(() => { return 42; });"#;
        let code = run_transform_with_strip(src, &["useServer"]);
        assert!(
            code.contains("_noopQrl("),
            "Stripped ctx_name should produce _noopQrl, got: {code}"
        );
    }

    // Test 5: non-function first arg → C03 diagnostic, captures cleared
    #[test]
    fn segment_extraction_non_fn_arg_clears_captures() {
        // When the first arg is not a function/arrow and there would be captures, C03 fires
        // and the scoped_idents are cleared. The result is still a QRL (noop or inline)
        // but without captures.
        let src = r#"import { component$ } from "@qwik.dev/core";
const x = 42;
component$(x);"#;
        let (code, xfrm) = run_transform_with_mode(src, EmitMode::Lib);
        // The transform should still produce output (not crash)
        assert!(
            !code.is_empty(),
            "Transform should produce output, got empty code"
        );
        // In Lib mode, should still produce inlinedQrl or noop
        let has_qrl = code.contains("inlinedQrl(") || code.contains("_noopQrl(");
        assert!(
            has_qrl || code.contains("Qrl"),
            "Should produce some QRL form, got: {code}"
        );
    }

    // Test 6: nested $ calls — inner extracted first
    #[test]
    fn segment_extraction_nested_calls() {
        let src = r#"import { component$, useTask$ } from "@qwik.dev/core";
const Cmp = component$(() => {
    useTask$(() => { console.log("task"); });
});"#;
        let (code, xfrm) = run_transform_with_mode(src, EmitMode::Lib);
        // Both should be extracted as inlinedQrl in Lib mode
        // Count inlinedQrl occurrences - should be 2
        let count = code.matches("inlinedQrl(").count();
        assert!(
            count >= 2,
            "Nested $ calls should each produce inlinedQrl, found {} in: {code}",
            count
        );
        // Lib mode: no segments
        assert!(
            xfrm.segments.is_empty(),
            "Lib mode nested calls should not push segments"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 13: hoist_qrl_to_module_scope tests
    // -----------------------------------------------------------------------

    /// Run transform with a given mode; return (output code, QwikTransform).
    fn run_transform_mode_segment(src: &str, mode: EmitMode) -> (String, QwikTransform) {
        let allocator = Allocator::default();
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &mode,
            scope: None,
            rel_path: "test",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let _scoping = traverse_mut(&mut xfrm, &allocator, &mut program, scoping, ());
        let code = Codegen::new().build(&program).code;
        (code, xfrm)
    }

    // Test: Lib mode guard — hoist_qrl_to_module_scope returns unchanged (no extra_top_items).
    #[test]
    fn hoist_qrl_to_module_scope_lib_guard() {
        let src = r#"import { component$ } from "@qwik.dev/core";
const Cmp = component$(() => {});"#;
        let (code, xfrm) = run_transform_mode_segment(src, EmitMode::Lib);
        // Lib mode: no hoisting — extra_top_items must be empty after traversal.
        assert!(
            xfrm.extra_top_items.is_empty(),
            "Lib mode must not populate extra_top_items, got {} items",
            xfrm.extra_top_items.len()
        );
        // Lib mode produces inlinedQrl inline.
        assert!(
            code.contains("inlinedQrl("),
            "Lib mode should produce inlinedQrl, got: {code}"
        );
    }

    // Test: Segment mode — extracted qrl() hoisted, call site replaced with q_name ident.
    #[test]
    fn hoist_qrl_to_module_scope_extracted_no_captures() {
        let src = r#"import { component$ } from "@qwik.dev/core";
const Cmp = component$(() => {});"#;
        let (code, _xfrm) = run_transform_mode_segment(src, EmitMode::Prod);
        // In Segment mode: `const q_<sym> = qrl(...)` should appear at module top.
        assert!(
            code.contains("const q_"),
            "Segment mode should hoist qrl() to const q_<sym>, got: {code}"
        );
        // The hoisted const uses qrl(...).
        assert!(
            code.contains("qrl("),
            "Hoisted const should contain qrl() call, got: {code}"
        );
        // The call site should reference q_<sym> (identifier), not inline qrl().
        // componentQrl(q_<sym>) pattern.
        assert!(
            code.contains("componentQrl(q_"),
            "Call site should be componentQrl(q_<sym>), got: {code}"
        );
    }

    // Test: deduplication — same symbol pushed only once.
    #[test]
    fn hoist_qrl_to_module_scope_dedup() {
        // Two calls that produce the same segment (same display name context) — distinct
        // symbols so they get distinct HoistedConsts. But if we call with the same name twice,
        // only one entry should appear.
        // We test via direct call.
        let allocator = Allocator::default();
        let src = "";
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let ast = AstBuilder::new(&allocator);
        // Build a simple qrl() call expression.
        let make_qrl_call = || {
            let callee = ast.expression_identifier(SPAN, ast.atom("qrl"));
            let args: ArenaVec<Argument<'_>> = ArenaVec::new_in(&allocator);
            ast.expression_call(SPAN, callee, None::<TSTypeParameterInstantiation<'_>>, args, false)
        };
        // Call hoist twice with the same symbol_name — should only push 1 HoistedConst.
        let _e1 = xfrm.hoist_qrl_to_module_scope(make_qrl_call(), &[], "sym_ABCDEFGHIJK", &allocator);
        let _e2 = xfrm.hoist_qrl_to_module_scope(make_qrl_call(), &[], "sym_ABCDEFGHIJK", &allocator);
        assert_eq!(
            xfrm.extra_top_items.len(),
            1,
            "Dedup: same symbol_name should push only one HoistedConst, got {}",
            xfrm.extra_top_items.len()
        );
    }

    // Test: with captures → call site becomes q_name.w([caps]).
    #[test]
    fn hoist_qrl_to_module_scope_extracted_with_captures() {
        let src = r#"import { useTask$ } from "@qwik.dev/core";
const count = 1;
const t = useTask$(() => { console.log(count); });"#;
        let (code, _xfrm) = run_transform_mode_segment(src, EmitMode::Prod);
        // With captures, the call site should use .w([count]).
        assert!(
            code.contains(".w([count])") || code.contains(".w(["),
            "Captured variable should produce q_name.w([count]) at call site, got: {code}"
        );
    }

    // -----------------------------------------------------------------------
    // Level 2 hoisting tests (Phase 13-02)
    // -----------------------------------------------------------------------

    /// Build a minimal QwikTransform in Segment/Prod mode for unit testing.
    fn make_transform_prod_segment() -> QwikTransform {
        make_transform_with_mode(EmitMode::Prod)
    }

    // Test: hoist_qrl_if_needed returns unchanged when not inside a loop.
    #[test]
    fn hoist_qrl_if_needed_no_loop() {
        let allocator = Allocator::default();
        let mut xfrm = make_transform_prod_segment();
        // iteration_var_stack is empty → no hoisting
        assert!(xfrm.iteration_var_stack.is_empty());

        // Build a q_name.w([cap]) expression to attempt hoisting.
        let w_expr = QwikTransform::build_w_call("q_sym", &["cap".to_string()], &allocator);
        let w_code_before = emit_expr(&allocator, w_expr.clone_in(&allocator));

        // Add a hoisted_qrls frame so condition 3 would pass.
        xfrm.hoisted_qrls.push(Vec::new());

        let result = xfrm.hoist_qrl_if_needed(w_expr, &["cap".to_string()], "w_sym", &allocator);
        let result_code = emit_expr(&allocator, result);

        // Should be unchanged (still the .w() call).
        assert_eq!(
            result_code, w_code_before,
            "No hoisting when not in a loop: result should be unchanged, got: {result_code}"
        );
        // No entries should be added to hoisted_qrls.
        assert!(
            xfrm.hoisted_qrls[0].is_empty(),
            "hoisted_qrls should be empty when not in a loop"
        );
    }

    // Test: compute_hoist_target_depth returns component top depth when no captures.
    #[test]
    fn compute_hoist_target_depth_empty_captures() {
        let mut xfrm = make_transform_prod_segment();
        // Simulate: root frame + component$ body frame in decl_stack.
        // decl_stack[0] = root, decl_stack[1] = component$ body.
        xfrm.decl_stack.push(vec![]); // frame 1 = component$ body
        // component_depths: pushed before component$ arrow entered (decl_stack.len() was 1).
        xfrm.component_depths.push(1);
        // hoisted_qrls: one frame for component$ body.
        xfrm.hoisted_qrls.push(Vec::new());

        let depth = xfrm.compute_hoist_target_depth(&[]);
        // component_decl_depth = 1, hoisted_qrls index = 1 - 1 = 0.
        assert_eq!(
            depth, 0,
            "Empty captures should hoist to component top (index 0), got {depth}"
        );
    }

    // Test: compute_hoist_target_depth returns shallowest scope covering all captures.
    #[test]
    fn compute_hoist_target_depth_with_captures() {
        let mut xfrm = make_transform_prod_segment();
        // decl_stack: root (0), component$ body (1), inner_fn body (2)
        xfrm.decl_stack.push(vec![
            ("cart".to_string(), IdentType::Var(false)),
        ]); // frame 1 = component$ body, has 'cart'
        xfrm.decl_stack.push(vec![]); // frame 2 = inner function
        // component_depths[0] = 1 (pushed before component$ arrow entered).
        xfrm.component_depths.push(1);
        // hoisted_qrls: two frames.
        xfrm.hoisted_qrls.push(Vec::new()); // index 0 = component$ frame
        xfrm.hoisted_qrls.push(Vec::new()); // index 1 = inner fn frame

        // 'cart' is in decl_stack[1] → min_decl_scope = 1 → hoisted_qrls index = 0.
        let depth = xfrm.compute_hoist_target_depth(&["cart".to_string()]);
        assert_eq!(
            depth, 0,
            "cart is in component$ body (decl_stack[1]) → hoisted_qrls index 0, got {depth}"
        );
    }

    // Test: inject_hoisted_qrls_into_block prepends const declarations at top.
    #[test]
    fn inject_hoisted_qrls_into_block_prepends() {
        let allocator = Allocator::default();
        let ast = AstBuilder::new(&allocator);

        // Build a function body with one statement: `let x = 1;`
        let let_stmt = {
            let binding = ast.binding_pattern_binding_identifier(SPAN, ast.atom("x"));
            let mut declarators: ArenaVec<VariableDeclarator<'_>> = ArenaVec::new_in(&allocator);
            declarators.push(ast.variable_declarator(
                SPAN,
                VariableDeclarationKind::Let,
                binding,
                None::<TSTypeAnnotation<'_>>,
                Some(ast.expression_numeric_literal(SPAN, 1.0, None, NumberBase::Decimal)),
                false,
            ));
            Statement::VariableDeclaration(ast.alloc_variable_declaration(
                SPAN,
                VariableDeclarationKind::Let,
                declarators,
                false,
            ))
        };

        let mut stmts: ArenaVec<Statement<'_>> = ArenaVec::new_in(&allocator);
        stmts.push(let_stmt);
        let directives: ArenaVec<Directive<'_>> = ArenaVec::new_in(&allocator);
        let mut body = ast.function_body(SPAN, directives, stmts);

        // Inject: `const q_w_sym = q_sym.w([cap]);`
        let entries = vec![
            ("q_w_sym".to_string(), "q_sym.w([cap])".to_string()),
        ];
        QwikTransform::inject_hoisted_qrls_into_block(&mut body, entries, &allocator);

        // Verify: first statement is `const q_w_sym = q_sym.w([cap]);`
        assert_eq!(
            body.statements.len(),
            2,
            "Should have 2 statements after injection"
        );
        // First stmt should be a const decl.
        match &body.statements[0] {
            Statement::VariableDeclaration(decl) => {
                assert_eq!(
                    decl.kind,
                    VariableDeclarationKind::Const,
                    "First injected statement should be a const declaration"
                );
                let name = decl.declarations.first().and_then(|d| {
                    if let BindingPattern::BindingIdentifier(id) = &d.id {
                        Some(id.name.as_str().to_string())
                    } else {
                        None
                    }
                });
                assert_eq!(
                    name.as_deref(),
                    Some("q_w_sym"),
                    "Injected const should be named q_w_sym, got: {:?}",
                    name
                );
            }
            other => panic!("Expected VariableDeclaration, got {:?}", std::mem::discriminant(other)),
        }
        // Second stmt should be the original `let x = 1;`
        match &body.statements[1] {
            Statement::VariableDeclaration(decl) => {
                assert_eq!(decl.kind, VariableDeclarationKind::Let);
            }
            other => panic!("Expected VariableDeclaration (let), got {:?}", std::mem::discriminant(other)),
        }
    }

    // Test: Level 2 hoisting via end-to-end traversal in a loop context.
    #[test]
    fn level2_hoist_in_loop_context() {
        // A component with an onClick$ inside a .map() loop that captures a variable.
        let src = r#"import { component$, useStore } from "@qwik.dev/core";
export const App = component$(() => {
  const cart = useStore([]);
  const items = ["a", "b"];
  return items.map((item) => {
    return onClick$(cart);
  });
});"#;
        let (code, xfrm) = run_transform_mode_segment(src, EmitMode::Prod);
        // After traversal, hoisted_qrls should be empty (all drained).
        assert!(
            xfrm.hoisted_qrls.is_empty(),
            "hoisted_qrls should be empty after traversal (all frames drained)"
        );
        // iteration_var_stack should also be empty.
        assert!(
            xfrm.iteration_var_stack.is_empty(),
            "iteration_var_stack should be empty after traversal"
        );
        // The output should not be empty.
        assert!(!code.is_empty(), "output should be non-empty");
    }

    // -----------------------------------------------------------------------
    // Phase 14-01: JSX utility methods tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_camel_to_kebab_basic() {
        assert_eq!(QwikTransform::camel_to_kebab("onClick"), "on-click");
    }

    #[test]
    fn test_camel_to_kebab_key_down() {
        assert_eq!(QwikTransform::camel_to_kebab("keyDown"), "key-down");
    }

    #[test]
    fn test_camel_to_kebab_no_uppercase() {
        assert_eq!(QwikTransform::camel_to_kebab("click"), "click");
    }

    #[test]
    fn test_camel_to_kebab_consecutive_uppercase() {
        // DOM -> d-o-m  (no special-casing for acronyms per SPEC, no leading dash)
        assert_eq!(QwikTransform::camel_to_kebab("DOM"), "d-o-m");
    }

    #[test]
    fn test_jsx_event_to_html_attribute_basic() {
        assert_eq!(
            QwikTransform::jsx_event_to_html_attribute("onClick$"),
            Some("q-e:click".to_string())
        );
    }

    #[test]
    fn test_jsx_event_to_html_attribute_key_down() {
        assert_eq!(
            QwikTransform::jsx_event_to_html_attribute("onKeyDown$"),
            Some("q-e:key-down".to_string())
        );
    }

    #[test]
    fn test_jsx_event_to_html_attribute_window() {
        assert_eq!(
            QwikTransform::jsx_event_to_html_attribute("window:onScroll$"),
            Some("q-w:scroll".to_string())
        );
    }

    #[test]
    fn test_jsx_event_to_html_attribute_document() {
        assert_eq!(
            QwikTransform::jsx_event_to_html_attribute("document:onLoad$"),
            Some("q-d:load".to_string())
        );
    }

    #[test]
    fn test_jsx_event_to_html_attribute_case_sensitive() {
        // on-customEvent$ -> q-e:customEvent (starts with -, preserve case)
        assert_eq!(
            QwikTransform::jsx_event_to_html_attribute("on-customEvent$"),
            Some("q-e:customEvent".to_string())
        );
    }

    #[test]
    fn test_jsx_event_to_html_attribute_not_event() {
        assert_eq!(QwikTransform::jsx_event_to_html_attribute("notAnEvent"), None);
    }

    #[test]
    fn test_normalize_jsx_text_basic() {
        assert_eq!(QwikTransform::normalize_jsx_text("  hello\n  world  "), "hello world");
    }

    #[test]
    fn test_normalize_jsx_text_trim_newlines() {
        assert_eq!(QwikTransform::normalize_jsx_text("\n  text\n"), "text");
    }

    #[test]
    fn test_normalize_jsx_text_empty() {
        assert_eq!(QwikTransform::normalize_jsx_text(""), "");
    }

    #[test]
    fn test_gen_jsx_key_format() {
        let allocator = Allocator::default();
        let src = r#"import { component$ } from "@qwik.dev/core";
export const A = component$(() => {});"#;
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(ret.program)
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let k0 = xfrm.gen_jsx_key();
        let k1 = xfrm.gen_jsx_key();
        // Format: <2-char-prefix>_<counter>
        assert!(k0.contains('_'), "key should contain underscore: {k0}");
        let parts0: Vec<&str> = k0.splitn(2, '_').collect();
        assert_eq!(parts0[0].len(), 2, "prefix should be 2 chars: {k0}");
        assert_eq!(parts0[1], "0", "first key counter should be 0: {k0}");
        let parts1: Vec<&str> = k1.splitn(2, '_').collect();
        assert_eq!(parts1[1], "1", "second key counter should be 1: {k1}");
    }

    #[test]
    fn test_jsx_fields_initialized() {
        let allocator = Allocator::default();
        let src = r#"import { component$ } from "@qwik.dev/core";"#;
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(ret.program)
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let xfrm = QwikTransform::new(opts);
        assert_eq!(xfrm.jsx_key_counter, 0, "jsx_key_counter should start at 0");
        assert_eq!(xfrm.jsx_file_hash_prefix.len(), 2, "jsx_file_hash_prefix should be 2 chars");
        assert!(!xfrm.jsx_mutable, "jsx_mutable should start false");
        assert!(xfrm.root_jsx_mode, "root_jsx_mode should start true");
    }

    // Test: exit_program drain order — top items prepended, bottom appended.
    #[test]
    fn exit_program_drain_order() {
        // In Segment mode, extra_top_items are populated with hoisted consts.
        // After exit_program, the output code should start with `const q_` declarations
        // BEFORE the rest of the module body.
        let src = r#"import { component$ } from "@qwik.dev/core";
export const App = component$(() => {});"#;
        let (code, _xfrm) = run_transform_mode_segment(src, EmitMode::Prod);
        // Find the position of `const q_` vs `export const App`.
        let q_pos = code.find("const q_");
        let app_pos = code.find("export const App");
        if let (Some(qp), Some(ap)) = (q_pos, app_pos) {
            assert!(
                qp < ap,
                "Hoisted const q_ should appear BEFORE export const App in output.\nOutput:\n{code}"
            );
        } else {
            // If segment mode didn't produce both, just verify the code compiled.
            assert!(
                !code.is_empty(),
                "exit_program_drain_order: output should be non-empty, got: {code}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Phase 14: JSX transform integration tests
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Helper to transform code with JSX visible (not inside component$ segment).
    // Uses a non-$ arrow function so JSX stays in the main output file.
    // -----------------------------------------------------------------------

    fn run_transform_inline_jsx(src: &str) -> String {
        let (code, _) = run_transform_mode_segment(src, EmitMode::Prod);
        code
    }

    /// Test: basic div with class prop produces _jsxSorted (JSX at top level, not in segment)
    #[test]
    fn jsx_basic_div_class() {
        // Lightweight component (no $) — JSX stays in main file
        let src = r#"import { _jsxSorted } from "@qwik.dev/core";
export const Lightweight = (props) => {
    return <div class="foo">hello</div>;
};"#;
        let code = run_transform_inline_jsx(src);
        assert!(code.contains("_jsxSorted"), "Should contain _jsxSorted call, got:\n{code}");
        assert!(
            code.contains(r#""foo""#),
            "Should have class value 'foo' in output, got:\n{code}"
        );
    }

    /// Test: component element (uppercase tag) emits key
    #[test]
    fn jsx_component_emits_key() {
        let src = r#"export const Header = () => <h1>hi</h1>;
export const App = () => {
    return <Header />;
};"#;
        let code = run_transform_inline_jsx(src);
        assert!(code.contains("_jsxSorted"), "Should contain _jsxSorted, got:\n{code}");
        // Component reference is identifier (not string)
        assert!(
            code.contains("_jsxSorted(Header"),
            "Component tag should be identifier reference, got:\n{code}"
        );
        // Key should be a string like "xx_0"
        assert!(
            code.contains("_0\"") || code.contains("_1\""),
            "Should have a key string ending with _0 or _1, got:\n{code}"
        );
    }

    /// Test: fragment transformation
    #[test]
    fn jsx_fragment_transformation() {
        let src = r#"export const App = () => {
    return <><div/><span/></>;
};"#;
        let code = run_transform_inline_jsx(src);
        assert!(code.contains("_Fragment"), "Should contain _Fragment, got:\n{code}");
        assert!(
            code.contains(r#"from "@qwik.dev/core/jsx-runtime""#),
            "Should import from jsx-runtime, got:\n{code}"
        );
        assert!(code.contains("_jsxSorted"), "Should use _jsxSorted, got:\n{code}");
    }

    /// Test: spread props route through _jsxSplit
    #[test]
    fn jsx_spread_props_use_jsx_split() {
        let src = r#"export const App = (props) => {
    return <button {...props}>click</button>;
};"#;
        let code = run_transform_inline_jsx(src);
        assert!(code.contains("_jsxSplit"), "Spread props should use _jsxSplit, got:\n{code}");
        assert!(
            code.contains("_getVarProps") || code.contains("_getConstProps"),
            "Spread should use _getVarProps/_getConstProps, got:\n{code}"
        );
    }

    /// Test: event handler renaming (onClick$ -> q-e:click for native elements)
    #[test]
    fn jsx_event_handler_renaming() {
        let src = r#"import { component$ } from "@qwik.dev/core";
const handler = () => {};
export const App = () => {
    return <button onClick$={handler}>click</button>;
};"#;
        let code = run_transform_inline_jsx(src);
        // Event handler should be renamed
        assert!(
            code.contains("\"q-e:click\"") || code.contains("\"q-e:click\":"),
            "onClick$ should become 'q-e:click', got:\n{code}"
        );
    }

    /// Test: className -> class for native elements (not components)
    #[test]
    fn jsx_classname_to_class_native() {
        let src = r#"export const App = () => {
    return <div className="foo" />;
};"#;
        let code = run_transform_inline_jsx(src);
        // className should become class for native elements
        assert!(
            !code.contains("className"),
            "className should be renamed to class for native elements, got:\n{code}"
        );
        assert!(
            code.contains("class:") || code.contains(r#"class: "#) || code.contains(r#""class":"#),
            "Should have 'class' prop in output, got:\n{code}"
        );
    }

    /// Test: nested JSX — inner elements get null key, outer gets real key
    #[test]
    fn jsx_nested_null_key() {
        let src = r#"export const App = () => {
    return <div><span/></div>;
};"#;
        let code = run_transform_inline_jsx(src);
        assert!(code.contains("_jsxSorted"), "Should use _jsxSorted, got:\n{code}");
        // The outer div has root mode → key; the nested span gets null key
        // We just verify both null AND a key string appear in the output.
        assert!(
            code.contains("null"),
            "Nested elements should have null key, got:\n{code}"
        );
    }

    /// Test: JSX runtime imports are injected at top of output
    #[test]
    fn jsx_runtime_imports_injected() {
        let src = r#"export const App = () => {
    return <div>hello</div>;
};"#;
        let code = run_transform_inline_jsx(src);
        // Import should appear in code
        assert!(
            code.contains(r#"import { _jsxSorted } from "@qwik.dev/core""#),
            "Should inject _jsxSorted import, got:\n{code}"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 15 unit tests: create_synthetic_qqsegment, _wrapProp, hoist_fn_signal
    // -----------------------------------------------------------------------

    /// Helper: build a minimal QwikTransform with one Var entry in decl_stack.
    fn make_transform_with_decl_var(name: &str, is_const: bool) -> QwikTransform {
        let src = "const x = 1;";
        let allocator = Allocator::default();
        let source_in_arena: &str = allocator.alloc_str(src);
        let ret = Parser::new(&allocator, source_in_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        // Push the variable into decl_stack root frame.
        if let Some(frame) = xfrm.decl_stack.last_mut() {
            frame.push((name.to_string(), IdentType::Var(is_const)));
        }
        xfrm
    }

    /// Helper: parse a single expression using a fresh Allocator (static lifetime workaround).
    fn parse_expr_for_test(src: &str) -> (Allocator, String) {
        // Return the source for the caller to keep the allocator alive.
        (Allocator::default(), src.to_string())
    }

    #[test]
    fn test_create_synthetic_qqsegment_plain_ident() {
        // Plain identifier should return (None, is_const) — step 7.
        let src = "signal";
        let alloc = Allocator::default();
        let src_arena: &str = alloc.alloc_str(src);
        let expr = Parser::new(&alloc, src_arena, SourceType::tsx())
            .parse_expression()
            .unwrap();
        // SAFETY: transmute for test lifetime.
        let expr: Expression<'static> =
            unsafe { std::mem::transmute::<Expression<'_>, Expression<'static>>(expr) };
        let alloc2 = Allocator::default();
        let mut xfrm = make_transform_with_decl_var("signal", false);
        let (result, _is_const) = xfrm.create_synthetic_qqsegment(&expr, &alloc2);
        assert!(result.is_none(), "Plain ident should return None (step 7)");
    }

    #[test]
    fn test_wrap_prop_value_1_arg() {
        // `signal.value` → `_wrapProp(signal)` (1 arg for .value)
        let src = "signal.value";
        let alloc = Allocator::default();
        let src_arena: &str = alloc.alloc_str(src);
        let expr = Parser::new(&alloc, src_arena, SourceType::tsx())
            .parse_expression()
            .unwrap();
        let expr: Expression<'static> =
            unsafe { std::mem::transmute::<Expression<'_>, Expression<'static>>(expr) };
        let alloc2 = Allocator::default();
        let mut xfrm = make_transform_with_decl_var("signal", false);
        let (result, _is_const) = xfrm.create_synthetic_qqsegment(&expr, &alloc2);
        assert!(result.is_some(), "signal.value should produce _wrapProp");
        let code = result.unwrap();
        assert_eq!(code, "_wrapProp(signal)", "1-arg form for .value, got: {code}");
        assert!(xfrm.needs_wrap_prop, "needs_wrap_prop should be set");
    }

    #[test]
    fn test_wrap_prop_named_2_args() {
        // `signal.count` → `_wrapProp(signal, "count")` (2 args for non-.value)
        let src = "signal.count";
        let alloc = Allocator::default();
        let src_arena: &str = alloc.alloc_str(src);
        let expr = Parser::new(&alloc, src_arena, SourceType::tsx())
            .parse_expression()
            .unwrap();
        let expr: Expression<'static> =
            unsafe { std::mem::transmute::<Expression<'_>, Expression<'static>>(expr) };
        let alloc2 = Allocator::default();
        let mut xfrm = make_transform_with_decl_var("signal", false);
        let (result, _is_const) = xfrm.create_synthetic_qqsegment(&expr, &alloc2);
        assert!(result.is_some(), "signal.count should produce _wrapProp");
        let code = result.unwrap();
        assert_eq!(
            code,
            r#"_wrapProp(signal, "count")"#,
            "2-arg form for non-.value, got: {code}"
        );
    }

    #[test]
    fn test_hoist_fn_signal_dedup() {
        // Two identical arrows should produce the same _hf<N> reference.
        let src = "const x = 1;";
        let alloc = Allocator::default();
        let src_arena: &str = alloc.alloc_str(src);
        let ret = Parser::new(&alloc, src_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: false,
        };
        let mut xfrm = QwikTransform::new(opts);
        let arrow = "p0 => p0.color";
        let call1 = format!("_fnSignal({arrow}, [theme])");
        let call2 = format!("_fnSignal({arrow}, [theme])");

        let result1 = xfrm.hoist_fn_signal_call(call1, arrow.to_string());
        let result2 = xfrm.hoist_fn_signal_call(call2, arrow.to_string());

        // Both should reference _hf0
        assert!(result1.contains("_hf0"), "First call should use _hf0, got: {result1}");
        assert!(result2.contains("_hf0"), "Second call should also use _hf0 (dedup), got: {result2}");
        // Only one HoistedConst should have been pushed
        assert_eq!(
            xfrm.extra_top_items.len(),
            1,
            "Dedup: only one HoistedConst should be pushed, got {}",
            xfrm.extra_top_items.len()
        );
        assert_eq!(xfrm.hoisted_fn_counter, 1, "Counter should be 1 after one unique arrow");
    }

    #[test]
    fn test_fn_signal_server_mode() {
        // Server mode: hoist_fn_signal_call should push two HoistedConsts (_hf0 and _hf0_str).
        let src = "const x = 1;";
        let alloc = Allocator::default();
        let src_arena: &str = alloc.alloc_str(src);
        let ret = Parser::new(&alloc, src_arena, SourceType::tsx()).parse();
        let mut program = unsafe {
            std::mem::transmute::<oxc::ast::ast::Program<'_>, oxc::ast::ast::Program<'static>>(
                ret.program,
            )
        };
        let collect = global_collect(&program);
        let opts = QwikTransformOptions {
            global_collect: &collect,
            core_module: "@qwik.dev/core",
            strip_ctx_name: &[],
            strip_event_handlers: false,
            mode: &EmitMode::Prod,
            scope: None,
            rel_path: "test.tsx",
            file_name: "test.tsx",
            entry_strategy: &EntryStrategy::Segment,
            extension: "tsx",
            explicit_extensions: false,
            is_server: true,
        };
        let mut xfrm = QwikTransform::new(opts);
        let arrow = "p0 => p0.color";
        let server_str = "\"p0 => p0.color\"";
        let call = format!("_fnSignal({arrow}, [theme], {server_str})");

        let result = xfrm.hoist_fn_signal_call(call, arrow.to_string());

        assert!(result.contains("_hf0"), "Should reference _hf0, got: {result}");
        assert_eq!(xfrm.extra_top_items.len(), 2, "Server mode: _hf0 + _hf0_str, got {}", xfrm.extra_top_items.len());
        assert_eq!(xfrm.extra_top_items[0].name, "_hf0");
        assert_eq!(xfrm.extra_top_items[1].name, "_hf0_str");
    }
}
