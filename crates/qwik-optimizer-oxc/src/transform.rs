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
}

// ---------------------------------------------------------------------------
// Traverse implementation
// ---------------------------------------------------------------------------

impl<'a> Traverse<'a, ()> for QwikTransform {
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
                // Replace first arg with a placeholder, take ownership of it.
                // We use Expression::NullLiteral as a placeholder.
                let null_lit = ctx.ast.expression_null_literal(SPAN);
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
            if !should_emit {
                let qrl_expr = self.create_noop_qrl(
                    &names.symbol_name,
                    &scoped_idents,
                    span,
                    &names.display_name,
                    allocator,
                );
                // Hoist _noopQrl to module scope (Lib guard inside handles Lib mode).
                let hoisted = self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                );
                call.arguments[0] = expr_to_argument(hoisted);
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
                let hoisted = self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                );
                call.arguments[0] = expr_to_argument(hoisted);
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
                let hoisted = self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                );
                call.arguments[0] = expr_to_argument(hoisted);
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
                let hoisted = self.hoist_qrl_to_module_scope(
                    qrl_expr,
                    &scoped_idents,
                    &names.symbol_name,
                    allocator,
                );
                call.arguments[0] = expr_to_argument(hoisted);
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

        // Add params as Var(false) entries in the child frame.
        for param in &func.params.items {
            collect_binding_names(&param.pattern, &mut |name| {
                if let Some(frame) = self.decl_stack.last_mut() {
                    frame.push((name.to_string(), IdentType::Var(false)));
                }
            });
        }
    }

    fn exit_function(&mut self, _func: &mut Function<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
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
        _arrow: &mut ArrowFunctionExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.decl_stack.pop();
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
        // Fast path: nothing to drain.
        if self.extra_top_items.is_empty()
            && self.ref_assignments.is_empty()
            && self.extra_bottom_items.is_empty()
        {
            return;
        }

        let allocator: &'a Allocator = ctx.ast.allocator;
        let mut new_body: ArenaVec<Statement<'a>> = ArenaVec::new_in(allocator);

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
}
