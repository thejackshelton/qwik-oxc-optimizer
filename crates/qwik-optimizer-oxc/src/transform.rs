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

use oxc::ast::ast::*;
use oxc::ast_visit::Visit;
use oxc_traverse::{Traverse, TraverseCtx};

use crate::collector::GlobalCollect;
use crate::entry_strategy::{self, EntryPolicy};
use crate::is_const;
use crate::types::{CtxKind, EmitMode, EntryStrategy};
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

    /// Placeholder for top-level statements prepended to the output module.
    /// Final AST type resolved in Plan 12-03 (exit_program).
    pub(crate) extra_top_items: Vec<String>,

    /// Placeholder for top-level statements appended to the output module.
    pub(crate) extra_bottom_items: Vec<String>,

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
            extra_top_items: Vec::new(),
            extra_bottom_items: Vec::new(),
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
            return; // Phase 12+ stub
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
            return; // Phase 12 adds _create_synthetic_qsegment
        }

        // Priority 7: plain identifier — push to context stack
        self.stack_ctxt.push(callee_name);
        self.ctxt_pushed_calls.insert(call.span.start);
    }

    /// Symmetric pop for case-7 plain-identifier calls; callee rename (XFRM-08).
    fn exit_call_expression(
        &mut self,
        call: &mut CallExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Symmetric pop for plain-identifier pushes.
        if self.ctxt_pushed_calls.remove(&call.span.start) {
            self.stack_ctxt.pop();
        }

        // convert_qrl_word: rewrite marker-function callee names (XFRM-08).
        // e.g. component$(...) → componentQrl(...)
        if let Expression::Identifier(id) = &mut call.callee {
            let callee_name = id.name.as_str().to_string();
            if self.marker_functions.contains_key(&callee_name) {
                let qrl_name = words::dollar_to_qrl_name(&callee_name);
                // Allocate the new name in the arena and convert Atom -> Ident.
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
}
