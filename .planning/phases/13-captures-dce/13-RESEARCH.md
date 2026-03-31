# Phase 13: Captures & DCE - Research

**Researched:** 2026-02-23
**Domain:** Rust OXC-based Qwik optimizer -- capture analysis edge cases and dead code elimination
**Confidence:** HIGH (all findings from direct codebase analysis and snapshot diff comparison)

## Summary

Phase 13 targets two categories of remaining semantic snapshot diffs: capture edge cases (17 tests) and dead code elimination gaps (9 tests). Post-Phase 12, the codebase has 63 exact matches out of 162 tests, with 99 diffs remaining. Of these, approximately 26 are actionable for Phase 13, with the rest belonging to other categories (import ordering, wrapping, entry field, dev mode, spread props, etc.) deferred to Phase 14.

The capture issues break down into several root causes: (1) OXC doesn't capture individual destructured props from component params -- it captures `_rawProps` instead of the individual property names, (2) outer loop iteration variables aren't captured for inner loop event handlers, (3) lightweight functional components (non-component$) don't get props-to-_rawProps rewrite, (4) C03 diagnostic emission where SWC captures but OXC doesn't (invalid_segment_expr), and (5) missing captures in nested $() scopes where OXC fails to thread captures through parent scope.

The DCE issues break down into: (1) unused variable declarations not stripped from segment bodies (const, let, function, class), (2) `if(false)` branches not eliminated inside segment bodies, (3) isBrowser/isServer dead branch elimination not applied inside Inline/Hoist strategy entry code, and (4) function/class declarations from invalid_decl_stack not removed from entry module output.

**Primary recommendation:** Fix capture root causes first (props destructuring capture propagation, outer loop var capture, lightweight functional rewrite), then implement segment-body-level DCE (unused var stripping, if(false) elimination).

## Architecture Patterns

### Relevant Source Files
```
crates/qwik-optimizer-oxc/src/
  collector.rs         # compute_captures() -- capture analysis
  transform.rs         # capture_stack, invalid_decl_stack, exit_call_expression
  const_replace.rs     # isBrowser/isServer replacement + dead branch elimination
  code_move.rs         # segment body serialization (where DCE should apply)
  is_const.rs          # const expression classification
  jsx_transform.rs     # JSX event handler segment creation
```

### Pattern: Capture Analysis Flow
1. On `enter_call_expression` for $()-calls: push `(body_ident_refs, body_local_decls)` frame onto `capture_stack`
2. During traversal: `enter_identifier_reference` adds names to `body_ident_refs`, `enter_variable_declaration` adds to `body_local_decls`
3. On `exit_call_expression`: pop frame, run `compute_captures()` to classify refs as LocalCapture vs ImportReemit
4. `reclassify_module_level_decl_captures()` converts module-level declarations to self-imports
5. JSX event handlers use `analyze_lambda_captures()` (separate code path from $() calls)

### Pattern: DCE Flow (Current)
1. `const_replace.rs`: Pre-pass replaces `isServer`/`isBrowser`/`isDev` with boolean literals + dead branch elimination
2. `simplify_unused_pure_var_decls()`: In exit_program, strips unused `/* @__PURE__ */` variable declarations
3. `collect_referenced_idents()`: Determines which identifiers are referenced for import filtering

### Anti-Patterns to Avoid
- **Modifying capture_stack outside enter/exit hooks**: The stack must be balanced; push in enter, pop in exit
- **Applying DCE before segment extraction**: Segment body serialization happens during traversal; DCE on segment bodies must happen post-serialization or during body code generation
- **Breaking props destructuring for non-component$ functions**: Lightweight functional components have different rewrite rules than component$

## Diff Categorization (99 Total Diffs)

### Phase 13 Scope: Captures (17 tests)

| Test | Root Cause | Category |
|------|-----------|----------|
| example_multi_capture | OXC captures `_rawProps` instead of individual prop names (foo, arg0, aaa) | PROPS-CAPTURE |
| example_lightweight_functional | Non-component$ arrow functions don't get _rawProps rewrite for individual prop destructuring | PROPS-CAPTURE |
| example_functional_component_capture_props | Missing captures: total, v1, v2, v3 not captured | PROPS-CAPTURE |
| example_functional_component_2 | Extra capture STEP_2 (const 2 should be inlined, not captured) + missing paramNames position | CONST-INLINE |
| example_jsx | Missing `children` capture in nested segment | CHILDREN-CAPTURE |
| example_exports | Missing captures (obj, v1, v2, v3) + C03 diagnostics not emitted | SCOPE-CAPTURE |
| example_invalid_segment_expr1 | OXC captures style/render; SWC emits C03 diagnostics instead | C03-DIAGNOSTIC |
| example_component_with_event_listeners_inside_loop | Missing _fnSignal wrapping for `results[i]` in loop context + missing captureNames for `cart` | LOOP-CAPTURE |
| should_extract_single_qrl_with_nested_components | Missing `item` capture in nested component segment | NESTED-CAPTURE |
| should_transform_nested_loops | Outer loop var `row` not captured for inner loop handler | OUTER-LOOP-VAR |
| should_not_generate_conflicting_props_identifiers | Hoist strategy: segments should be extracted to named consts, not inlined | HOIST-CAPTURE |
| should_transform_component_with_normal_function | Missing capture in segment | MISSING-CAPTURE |
| should_extract_single_qrl | captureNames ordering diff | CAPTURE-ORDER |
| should_extract_single_qrl_2 | Segment naming dedup suffix order swapped | NAMING-DEDUP |
| should_extract_single_qrl_with_index | captureNames ordering + `if` brace formatting | CAPTURE-ORDER |
| should_transform_qrls_in_ternary_expression | captureNames ordering diff | CAPTURE-ORDER |
| example_props_optimization | Multiple issues: DCE of defaults (??3 vs ??1+2), missing useTaskQrl captures, import ordering | PROPS-CAPTURE |

### Phase 13 Scope: DCE (9 tests)

| Test | Root Cause | Category |
|------|-----------|----------|
| example_10 | Unused const declarations not stripped from segment body | UNUSED-VAR |
| example_8 | Unused destructured const + unused const not stripped | UNUSED-VAR |
| example_9 | Unused let, function, class, try/catch not stripped from segment body | UNUSED-DECL |
| example_capturing_fn_class | Function/class declarations in invalid_decl_stack not removed from component body | INVALID-DECL |
| example_dead_code | `if(false)` not eliminated inside segment body + `deps` import not stripped | IF-FALSE |
| example_use_optimization | Destructured assignments not simplified (SWC inlines intermediate vars) | CONST-FOLD |
| example_optimization_issue_4386 | Intermediate const declarations not simplified | CONST-FOLD |
| example_qwik_router_inline | isBrowser/isServer DCE not applied inside Inline strategy entry code | BUILD-DCE |
| example_qwik_react_inline | isBrowser/isServer DCE not applied inside pre-transformed code | BUILD-DCE |

### Phase 13 Scope: Combined (some tests overlap)

| Test | Has Capture + DCE Issues |
|------|--------------------------|
| example_immutable_analysis | DCE (unused const) + aesthetic (line wrapping in _hf string) |
| should_wrap_store_expression | DCE (unused const) + aesthetic |
| should_mark_props_as_var_props_for_inner_cmp | DCE (unused const) + aesthetic |

### NOT Phase 13 (deferred to Phase 14 or accepted)

| Category | Count | Tests |
|----------|-------|-------|
| AESTHETIC (whitespace-only) | 4 | example_2, example_7, should_handle_dangerously_set_inner_html, should_transform_event_names_without_jsx_transpile |
| IMPORT-ORDER | ~12 | Various import ordering diffs |
| WRAPPING (residual _fnSignal/_wrapProp) | ~14 | Various signal wrapping diffs |
| ENTRY-FIELD | 4 | example_11, example_default_export, example_manual_chunks, example_use_server_mount |
| DEV-MODE (file path format) | 2 | example_dev_mode, example_dev_mode_inlined |
| NOOP-QRL / _regSymbol / server$ | 6 | example_reg_ctx_name_segments*, example_strip_client_code, example_noop_dev_mode, example_drop_side_effects |
| SPREAD props | 7 | Various _jsxSplit/_createElement diffs |
| PRE-TRANSFORM (already-transformed input) | 1 | example_parsed_inlined_qrls |
| NAMING-CONFLICT | 1 | example_qwik_conflict |
| RENAME (@builder.io) | 2 | rename_builder_io, example_jsx_import_source |
| JSX-FLAG | 7 | Various flag value diffs |
| OTHER | ~8 | Various minor diffs |

## Common Pitfalls

### Pitfall 1: Props Destructuring vs Capture Interaction
**What goes wrong:** When a component$ parameter is `({foo, bar})`, SWC rewrites to `(_rawProps)` at the component level but captures the individual destructured names (foo, bar) in nested $() segments. OXC captures `_rawProps` instead.
**Why it happens:** The props destructuring rewrite converts the param to `_rawProps` before capture analysis runs on nested $() bodies. The nested body sees `_rawProps.foo` references, not `foo`.
**How to avoid:** Capture analysis for nested $() bodies must look at the ORIGINAL ident references (before props rewrite), or the rewrite must propagate capture info so nested segments know about the original names.
**Warning signs:** Segments that capture `_rawProps` when SWC captures individual prop names.

### Pitfall 2: Outer Loop Variable Capture
**What goes wrong:** In nested loops, inner event handlers need to capture outer loop iteration variables. `current_iteration_vars()` only returns the innermost loop's vars via `iteration_var_stack.last()`.
**Why it happens:** Design decision from Phase 10: "outer loop vars become captures" -- but the capture mechanism doesn't actually implement this.
**How to avoid:** For event handlers in nested loops, exclude only the innermost loop's iteration vars from captures. Outer loop vars must go through the capture mechanism (_captures[N]).
**Warning signs:** `should_transform_nested_loops` test -- `row` from outer loop not captured.

### Pitfall 3: Segment-Level DCE vs Module-Level DCE
**What goes wrong:** Module-level DCE (`simplify_unused_pure_var_decls`) works on the entry module but segment bodies don't get DCE treatment.
**Why it happens:** Segment body code is serialized as a string during traversal, before module-level DCE runs.
**How to avoid:** Apply DCE transforms either during body serialization or as a post-processing step on the serialized body code.
**Warning signs:** Segments contain unused `const hola = ...`, `let decl8`, `function decl10()`, etc.

### Pitfall 4: `if(false)` Inside Segment Bodies
**What goes wrong:** SWC eliminates `if(false){...}` branches from segment bodies. OXC doesn't.
**Why it happens:** `const_replace.rs` only handles `isServer`/`isBrowser`/`isDev` replacements. Literal `if(false)` is not handled.
**How to avoid:** The `DeadBranchEliminator` in `const_replace.rs` handles `if(false)` patterns, but it runs as a pre-pass on the whole program. Segment body extraction captures the raw source, bypassing this transform.
**Warning signs:** `example_dead_code` -- segment body contains `if(false){deps();}`.

### Pitfall 5: Invalid Declaration Removal
**What goes wrong:** Function and class declarations inside component$ bodies that are referenced in nested $() scopes get C02 diagnostics but their declarations are NOT removed from the component body output.
**Why it happens:** `invalid_decl_stack` tracks them for diagnostic purposes and excludes them from captures, but nothing removes them from the output.
**How to avoid:** After C02 diagnostic emission, also remove the function/class declarations from the component body.
**Warning signs:** `example_capturing_fn_class` -- `function hola()`, `class Thing`, `class Other` remain in output.

## Code Examples

### Current compute_captures() (collector.rs:171-239)
```rust
pub(crate) fn compute_captures(
    body_ident_refs: &[String],
    body_local_decls: &HashSet<String>,
    collect_result: &CollectResult,
) -> CaptureAnalysisResult {
    let mut capture_names = Vec::new();
    // ... classification logic ...
    capture_names.sort(); // Alphabetical order matching SWC
    CaptureAnalysisResult { capture_names, reemitted_imports, diagnostics }
}
```

### Current simplify_unused_pure_var_decls() (transform.rs:3858-3925)
```rust
fn simplify_unused_pure_var_decls<'a>(stmts: &mut Vec<Statement<'a>>, ctx: &mut TraverseCtx<'a, ()>) {
    let all_refs = collect_referenced_idents(stmts);
    // Only handles: single-declarator, BindingIdentifier, PURE-annotated CallExpression
    // Does NOT handle: unused non-call const, unused let, unused function/class, if(false)
}
```

### Current const_replace dead branch elimination (const_replace.rs:204-257)
```rust
fn eliminate_dead_if_statements<'a>(stmts: &mut Vec<Statement<'a>>, ast: &AstBuilder<'a>) {
    // Only eliminates if-statements whose test evaluates to a boolean literal
    // Works for: if(true), if(false), if(!true), if(!false)
    // Does NOT work for: literal if(false) in segment bodies (runs pre-transform)
}
```

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Full scope analysis for captures | Custom scope tracker | OXC Scoping API (already available via traverse) | Scoping handles all edge cases; manual tracking misses nested scopes |
| Complete DCE | Full tree-shaking pass | Targeted transforms for known patterns | SWC's DCE is limited to specific patterns, not a full optimizer |

## Detailed Root Cause Analysis

### CAPTURE Category: Props Destructuring (5 tests)

**Tests:** example_multi_capture, example_lightweight_functional, example_functional_component_capture_props, example_multi_capture, example_props_optimization

**Root cause:** When component$ has destructured props `({foo, bar})`, OXC rewrites to `(_rawProps)` and changes body references to `_rawProps.foo`, `_rawProps.bar`. Nested $() bodies then see `_rawProps` references, so capture analysis captures `_rawProps` as a single capture. SWC captures the individual property names (foo, bar) instead.

**SWC behavior:** SWC's capture analysis runs on the ORIGINAL source before props destructuring rewrite. The rewrite happens at a different phase. So nested scopes still see `foo`, `bar` as captures.

**Fix direction:** Either (a) run capture analysis on nested $() bodies BEFORE props destructuring rewrite, or (b) maintain a mapping from `_rawProps.X` back to the original destructured name `X` and use it during capture computation.

### CAPTURE Category: Nested Scope (4 tests)

**Tests:** example_jsx (children), should_extract_single_qrl_with_nested_components (item), should_transform_component_with_normal_function, example_exports

**Root cause:** Variables from enclosing scope (parent component body) that are used inside nested $() calls are not being captured. The `is_top_level_dollar_call` check zeros out captures for top-level segments, but for intermediate segments (component body -> nested $() call), the captures are not threaded correctly.

**Fix direction:** Review the capture_stack interaction for nested $() calls within component$ bodies. The issue is likely in how `reclassify_module_level_decl_captures` handles intermediate-scope variables.

### CAPTURE Category: Outer Loop Variables (1 test)

**Test:** should_transform_nested_loops

**Root cause:** `current_iteration_vars()` returns `iteration_var_stack.last()` (innermost loop only). For inner loop event handlers, the outer loop's iteration variable `row` is excluded from captures by the general capture filter, but it should be captured since it's not a parameter of the inner handler.

**Fix direction:** When filtering iteration vars from captures, only filter vars from ALL active iteration stacks that are in the handler's param list. Vars from outer loops that aren't in the param list must remain as captures.

### DCE Category: Unused Declarations (3 tests)

**Tests:** example_10, example_8, example_9

**Root cause:** SWC's DCE in `MinifyMode::Simplify` strips unused variable/let/function/class declarations from segment bodies. OXC's `simplify_unused_pure_var_decls` only handles PURE-annotated call expressions, not general unused declarations.

**Fix direction:** Extend the unused-declaration stripping to handle:
- `const x = expr;` where `x` is not referenced (strip to `expr;` if side-effectful, remove entirely if not)
- `let x = expr, y;` where none are referenced
- `function f() {}` where `f` is not referenced
- `class C {}` where `C` is not referenced

### DCE Category: if(false) in Segment Bodies (1 test)

**Test:** example_dead_code

**Root cause:** The `DeadBranchEliminator` runs as a pre-pass in `const_replace.rs` before segment extraction. The `if(false)` is inside a $() body, which gets serialized as raw source during traversal. The pre-pass doesn't see it because it's inside a lambda argument.

**Fix direction:** Apply dead-branch elimination to segment body code either:
- During body serialization (parse + transform + reserialize)
- As a post-processing step on the body string
- By extending the pre-pass to recurse into $() callback arguments

### DCE Category: isBrowser/isServer in Inline Strategy (2 tests)

**Tests:** example_qwik_router_inline, example_qwik_react_inline

**Root cause:** For Inline strategy, segment code stays in the entry module. The `const_replace` pre-pass replaces `isServer`/`isBrowser` with boolean literals and eliminates dead branches. But these tests have pre-transformed `inlinedQrl()` calls where the body was already serialized before the const_replace pass could see it. The issue is that `const_replace` runs on the program but segment body extraction happens during traversal, capturing the post-replace source.

**Fix direction:** These may resolve automatically once general `if(false)` elimination is applied to segment bodies. The `isBrowser`/`isServer` identifiers should already be replaced by the pre-pass; the issue is likely that the replacement happens but the dead branch elimination doesn't descend into inline lambda bodies deeply enough.

### DCE Category: Invalid Declarations Not Removed (1 test)

**Test:** example_capturing_fn_class

**Root cause:** Function/class declarations in the `invalid_decl_stack` get C02 diagnostics but are NOT removed from the component body output. SWC removes them.

**Fix direction:** After emitting C02 diagnostics, also strip the corresponding function/class declaration statements from the component body.

## Open Questions

1. **Props capture propagation timing**
   - What we know: SWC captures original prop names before destructuring rewrite; OXC captures `_rawProps` after rewrite
   - What's unclear: Whether to fix this by delaying rewrite or by maintaining a reverse mapping
   - Recommendation: Maintain a mapping of `_rawProps -> original destructured names` and use it during capture analysis for nested $() bodies

2. **Segment body DCE approach**
   - What we know: Segment body is serialized as a string during traversal
   - What's unclear: Whether to apply DCE by re-parsing body code or by transforming AST before serialization
   - Recommendation: Apply DCE at the AST level before body code serialization (requires moving DCE into the traverse hooks rather than post-processing)

3. **Cascade effects from Phase 12 on captures**
   - What we know: Phase 12 fixed _fnSignal/_wrapProp wrapping which could affect capture sets
   - What's unclear: Exact number of captures tests that were resolved by Phase 12
   - Recommendation: The 17 capture tests listed above are the remaining ones post-Phase 12

## Priority Ordering for Phase 13

Based on impact (number of tests fixed) and complexity:

### High Priority (fix first)
1. **Props destructuring capture propagation** (5 tests) -- Core capture mechanism issue
2. **Segment-body DCE: unused declarations** (3-5 tests) -- Broad impact
3. **Nested scope captures** (4 tests) -- Core capture mechanism issue

### Medium Priority
4. **if(false) elimination in segment bodies** (1 test) -- Isolated fix
5. **Invalid declaration removal** (1 test) -- Isolated fix
6. **Outer loop variable capture** (1 test) -- Isolated fix

### Low Priority (may resolve from above)
7. **Capture ordering tweaks** (3 tests) -- May be side effects of other fixes
8. **Build constant DCE in inline strategy** (2 tests) -- Complex, may overlap with Phase 14

## Sources

### Primary (HIGH confidence)
- Direct analysis of codebase: `collector.rs`, `transform.rs`, `const_replace.rs`, `code_move.rs`
- Snapshot diff analysis: all 99 .snap.new files compared against golden .snap references
- Test fixture source code analysis: `tests/input/*.tsx`

## Metadata

**Confidence breakdown:**
- Capture edge case categorization: HIGH -- direct evidence from snapshot diffs
- DCE categorization: HIGH -- direct evidence from snapshot diffs
- Root cause analysis: HIGH for most, MEDIUM for props capture timing (need to verify SWC order)
- Fix direction: MEDIUM -- approaches are clear but implementation details need validation

**Research date:** 2026-02-23
**Valid until:** Indefinite (codebase analysis, not external dependencies)
