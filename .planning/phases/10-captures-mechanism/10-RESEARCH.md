# Phase 10: Captures Mechanism - Research

**Researched:** 2026-02-23
**Domain:** Qwik segment extraction -- _captures array access pattern for extracted segments
**Confidence:** HIGH

## Summary

Phase 10 addresses the mechanism by which extracted segments access variables from their enclosing scope. The current OXC implementation has two distinct problems compared to SWC:

1. **Function parameters stripped from segment modules**: When a segment is extracted (e.g., event handler in a loop), SWC preserves the original function parameters (e.g., `(_, _1, item) =>`) in the segment module. OXC currently serializes the body from raw source, losing any parameter injection that should have happened, and the `inject_captures_into_body` function doesn't handle params at all.

2. **Iteration variables incorrectly captured**: SWC explicitly filters out function parameters (including injected iteration variables) from `scoped_idents` (captures) via `scoped_idents.retain(|id| !param_idents.contains(id))`. OXC's `compute_captures` does not perform this filtering, so iteration variables like `item`, `i`, `key` end up in the captures array instead of being passed as function parameters via `q:p`.

Additionally, there are secondary capture-related issues:
- Some tests show extra captures (`aaa`, `arg0`) that SWC correctly resolves through props destructuring rewrite
- Capture count mismatches where OXC captures variables that SWC accesses via `_rawProps.property`

**Primary recommendation:** Implement two-part fix: (A) filter function params from captures in `compute_captures` or the calling code, and (B) inject iteration variable params into the serialized segment body.

## Architecture Patterns

### SWC's Captures Flow (Reference)

SWC's captures mechanism follows this flow:

```
1. _create_synthetic_qsegment():
   a. Collect descendent_idents (all idents used in body)
   b. Partition declarations into (decl_collect, invalid_decl)
   c. Fold the expression (recursively transform nested segments)
   d. get_function_params() -> param_idents  [CRITICAL STEP]
   e. compute_scoped_idents(descendent_idents, decl_collect) -> scoped_idents
   f. scoped_idents.retain(|id| !param_idents.contains(id))  [FILTER PARAMS]
   g. Store scoped_idents on SegmentData

2. code_move::new_module():
   a. If scoped_idents non-empty: emit `import { _captures }` from core
   b. Emit sorted local_idents imports
   c. transform_function_expr(expr, _captures_id, scoped_idents)
      -> Preserves original params via `..arrow` spread
      -> Injects `const varName = _captures[N]` at body start
   d. Emit export const

3. Entry module (QRL call site):
   a. qrl(i_hash, "name", [captured_vars])
      -> Only scoped_idents in array, NOT function params
```

### OXC's Current Flow (What Differs)

```
1. exit_expression / enter_jsx_attribute:
   a. analyze_lambda_captures(source_code, span) -> (body_ident_refs, body_local_decls)
   b. compute_captures(body_ident_refs, body_local_decls, collected) -> capture_result
   c. Filter against capture_stack frames (parent $-body scopes)
   d. Reclassify module-level decl captures to needed_imports
   e. Store capture_names on SegmentData
   [MISSING: param filtering step]

2. Segment body serialization:
   a. For $() calls: codegen_expression_with_comments extracts AST expr
   b. For JSX events: serialize_jsx_lambda_from_source extracts from raw source
   [MISSING: param injection for iteration vars]

3. code_move::build_segment_code_with_hoisted():
   a. Check has_captures = segment.captures && !capture_names.is_empty()
   b. Emit _captures import if needed
   c. Emit sorted imports
   d. inject_captures_into_body: inserts `const var = _captures[N]` statements
   e. Emit export
   [CORRECT: _captures injection logic is sound]
```

### Key Differences Summary

| Aspect | SWC | OXC (Current) |
|--------|-----|---------------|
| Param filtering | `scoped_idents.retain(!param_idents)` | Not done -- params end up in captures |
| Iter var params | Injected into arrow via `transform_event_handler_with_iter_var` | Stored in param_names metadata only |
| Segment module params | Preserved via `..arrow` (original params kept) | Serialized from source (no param injection) |
| Props destructuring | `_rawProps` captures props collectively | Individual props captured separately (some tests) |

### Pattern 1: Function Parameter Exclusion from Captures

**What:** When computing captures for a segment, exclude all function parameter identifiers from the capture list.
**When to use:** Every segment extraction (both $() calls and JSX event handlers).
**Source:** SWC `transform.rs` lines 696-704

SWC approach:
```rust
// Get function parameters to exclude from captured scope
let param_idents = get_function_params(&folded);

let (mut scoped_idents, is_const) = compute_scoped_idents(&descendent_idents, &decl_collect);

// Filter out function parameters from scoped_idents
// Parameters don't need to be captured via _captures
scoped_idents.retain(|id| !param_idents.contains(id));
```

OXC equivalent would filter after `compute_captures()` or add params to `body_local_decls`.

### Pattern 2: Iteration Variable Parameter Injection

**What:** For event handlers inside loops, inject iteration variable params into the segment module's exported function signature.
**When to use:** When `param_names` includes iteration variables (positions 2+).
**Source:** SWC `transform.rs::transform_event_handler_with_iter_var` lines 3486-3580

SWC modifies the actual arrow expression params before it gets serialized. OXC needs to do the equivalent either:
- During body serialization (modifying the source string), or
- During code_move (reconstructing the function signature)

### Anti-Patterns to Avoid

- **Don't modify `compute_captures` itself**: The function is general-purpose. Parameter filtering should happen at the call sites, as SWC does.
- **Don't add params to body_local_decls**: This would prevent them from being detected as captures AND prevent them from being recognized as function params. They need to be filtered from captures while remaining as function params.
- **Don't inject iteration variable params via string manipulation of the serialized body**: This is fragile. Better to reconstruct the params in `code_move::build_segment_code_with_hoisted` using the segment's `param_names`.

## Common Pitfalls

### Pitfall 1: Iteration Variables in Captures vs Params

**What goes wrong:** Iteration variables (like `item` in `.map(item => ...)` or `i` in `for (let i = 0; ...)`) end up in the captures array AND are missing from the function params.
**Why it happens:** `analyze_lambda_captures` collects all ident refs including params; no subsequent filtering removes param idents from captures.
**How to avoid:** After computing captures, filter out all identifiers that appear in the segment's param_names.
**Warning signs:** QRL call sites show `[cart, item]` instead of `[cart]`; segment modules show `() =>` instead of `(_, _1, item) =>`.

### Pitfall 2: Param Injection for Expression Bodies

**What goes wrong:** Arrow functions with expression bodies `() => expr` need special handling when injecting captures AND params.
**Why it happens:** `inject_captures_into_body` already handles this case (converts to block body with return), but param injection is separate.
**How to avoid:** Handle the param injection at the same level as capture injection, or earlier (during body serialization).

### Pitfall 3: Props Destructuring Interaction

**What goes wrong:** Some capture diffs are actually caused by incomplete props destructuring rewrite, not the captures mechanism itself. For example, `example_multi_capture` shows `foo`, `aaa`, `arg0` as captures when SWC only captures `_rawProps`.
**Why it happens:** When props are destructured (`({ foo }) => ...`), SWC rewrites to `(_rawProps) => ... _rawProps.foo`, which means only `_rawProps` is captured. If the OXC destructuring rewrite is incomplete, individual prop names leak into captures.
**How to avoid:** These are NOT Phase 10 issues -- they're Phase 12 (props destructuring gaps). Phase 10 should only address the parameter filtering and injection mechanism.

### Pitfall 4: Multiple Capture Scopes

**What goes wrong:** Nested $() calls have layered capture scopes where inner segments should only capture from the nearest enclosing scope.
**Why it happens:** OXC's capture_stack mechanism already handles this, but parameter filtering needs to work correctly at each nesting level.
**How to avoid:** Apply param filtering per-segment, not globally. Each segment's own function params should be excluded from its captures.

## Code Examples

### Example: Parameter Filtering in OXC (Proposed)

Currently in `transform.rs` for JSX event handlers (around line 1192):

```rust
let capture_result = collector::compute_captures(
    &body_ident_refs,
    &body_local_decls,
    &self.collected,
);
```

Should add param filtering:

```rust
let capture_result = collector::compute_captures(
    &body_ident_refs,
    &body_local_decls,
    &self.collected,
);
// Filter function parameters from captures (SWC: scoped_idents.retain(!param_idents))
let param_names_set: HashSet<String> = param_names.iter().cloned().collect();
let capture_result = collector::CaptureAnalysisResult {
    capture_names: capture_result.capture_names
        .into_iter()
        .filter(|name| !param_names_set.contains(name))
        .collect(),
    reemitted_imports: capture_result.reemitted_imports,
    diagnostics: capture_result.diagnostics,
};
```

### Example: Parameter Injection in code_move.rs (Proposed)

In `build_segment_code_with_hoisted`, after constructing the body with captures, inject params into the function signature:

```rust
// If segment has param_names (iteration variable params), inject them
if !segment.param_names.is_empty() && has_captures {
    // The serialized body starts with `() => { ... }` or similar
    // Need to replace the `()` with `(_, _1, item)` etc.
    // Based on param_names stored on segment
}
```

However, the cleaner approach is to handle param injection during body serialization time (in `serialize_jsx_lambda_from_source` or equivalent).

### Example: SWC Segment Module Output (Golden Reference)

For an event handler inside a loop capturing `selectedItem` with iteration var `row`:

**Entry module (QRL call):**
```javascript
qrl(i_xxx, "handler_name", [selectedItem])
// Note: [selectedItem] only -- NOT [selectedItem, row]
```

**Segment module:**
```javascript
import { _captures } from "@qwik.dev/core";
export const handler_name = (_, _1, row) => {
  const selectedItem = _captures[0];
  // ... body uses both selectedItem and row
};
```

Key observations:
- `row` is a function parameter (position 2), NOT in _captures
- `selectedItem` IS in _captures[0]
- The function signature has `(_, _1, row)` -- placeholders for event/element, then iteration var

## Affected Tests (24 total)

### Category A: Parameter stripping (segment module has `() =>` but should have `(_, _1, var) =>`)
These are event handlers inside loops where iteration variables need to be function params:
1. `example_component_with_event_listeners_inside_loop` (6 segments affected)
2. `should_extract_single_qrl` (2 segments)
3. `should_extract_single_qrl_2` (1 segment)
4. `should_extract_single_qrl_with_index` (2 segments)
5. `should_extract_single_qrl_with_nested_components` (1 segment)
6. `should_transform_component_with_normal_function` (1 segment)
7. `should_transform_multiple_event_handlers` (3 segments)
8. `should_transform_multiple_event_handlers_case2` (3 segments)
9. `should_transform_nested_loops` (2 segments)
10. `example_functional_component_2` (1 segment)
11. `example_lightweight_functional` (1 segment, _rawProps param issue)
12. `should_not_generate_conflicting_props_identifiers` (1 segment)

### Category B: Extra captures (iteration vars in captures array instead of params)
Same tests as Category A, but from the entry module perspective -- the QRL call includes extra vars.

### Category C: Capture count mismatches (props destructuring interaction)
These tests have capture diffs but may be primarily caused by props destructuring gaps:
1. `destructure_args_inline_cmp_block_stmt` (capture name `_rawProps` vs `data`)
2. `destructure_args_inline_cmp_block_stmt2`
3. `destructure_args_inline_cmp_expr_stmt`
4. `example_multi_capture` (extra captures `aaa`, `arg0`, `foo`)
5. `example_functional_component_capture_props` (extra captures `total`, `v1`, `v2`, `v3`)
6. `example_jsx` (extra capture `children`)
7. `example_exports` (extra captures `obj`, `v1`, `v2`, `v3`)

### Category D: Minor capture-related diffs
Tests with small _captures adjustments (add/remove) that are side effects of other issues:
1. `example_inlined_entry_strategy` (1 capture removed)
2. `example_issue_33443` (1 capture removed)
3. `example_optimization_issue_3542` (1 capture removed)
4. `example_props_optimization` (4 captures removed)
5. `example_strip_client_code` (1 capture removed)

## Implementation Strategy

### Step 1: Filter Function Params from Captures (Core Fix)

**Where:** Both capture computation sites in `transform.rs`:
- JSX event handler path (around line 1192)
- Regular $() exit_expression path (around line 2808)

**What:** After `compute_captures()`, filter out identifiers that are in the segment's `param_names`.

**Impact:** Fixes Category A and B tests (iteration vars removed from captures, correct QRL arrays).

### Step 2: Inject Iteration Variable Params into Segment Bodies

**Where:** `code_move::build_segment_code_with_hoisted()` or earlier during body serialization.

**What:** When a segment has `param_names` with iteration variables (positions 2+), the segment module's exported function must include those params. Two approaches:

**Approach A (String manipulation in code_move):**
Replace the `() =>` in the serialized body with `(_, _1, item) =>` using the segment's param_names. This is conceptually simple but fragile.

**Approach B (Modify during body serialization):**
In `serialize_jsx_lambda_from_source`, parse the lambda, modify its params, and re-serialize. More robust but requires AST manipulation.

**Approach C (Reconstruct in code_move):**
Instead of injecting into the body string, construct the function signature separately from the body. Parse just the body statements and wrap in a new arrow function with the correct params.

**Recommendation:** Approach A (string manipulation) is simplest and matches the existing pattern in `inject_captures_into_body`. The function signature is a well-defined prefix of the body string.

### Step 3: Validate and Iterate

Run snapshot tests after each step to measure progress. Some Category C/D tests may resolve as side effects of the core fixes, or may need separate attention in Phase 12.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Param name extraction | Custom parser for params | Segment's `param_names` field | Already computed and stored |
| Arrow function detection | Complex string parsing | `find_arrow_position()` | Already exists in code_move.rs |
| Capture ordering | Custom sort | `capture_names.sort()` | Already sorted alphabetically |

## Open Questions

### 1. Props Destructuring Overlap

**What we know:** Some capture diffs (Category C) are caused by props destructuring rewrite gaps, not the captures mechanism. SWC rewrites `({ foo }) => ...foo...` to `(_rawProps) => ..._rawProps.foo...`, which changes what gets captured.

**What's unclear:** How many of the 24 tests will be fixed by Phase 10 alone vs requiring Phase 12 (props destructuring)?

**Recommendation:** Implement Phase 10's core param filtering first, then re-measure. Category C tests may need Phase 12 work.

### 2. Non-Event-Handler Param Filtering

**What we know:** SWC's `get_function_params` extracts ALL params from both arrow and regular functions. The filtering applies to all segment types.

**What's unclear:** For `$(() => ...)` calls (not JSX events), does OXC already handle param filtering correctly? The `body_local_decls` in `analyze_lambda_captures` includes parameter names...

**Recommendation:** Verify by checking if `analyze_lambda_captures` adds arrow params to `body_local_decls`. If it does, then `compute_captures` already filters them for non-event-handler segments. The issue may only affect JSX event handlers.

### 3. Inline Strategy Handling

**What we know:** The captures mechanism behaves differently for inline vs segment strategy. Inline strategy injects `_captures` import in the entry module itself and calls `transform_function_expr` to modify the inline expression.

**What's unclear:** Whether the param filtering issue also affects inline strategy tests.

**Recommendation:** Focus on segment strategy first (the default test configuration), then verify inline strategy.

## Sources

### Primary (HIGH confidence)
- SWC reference implementation: `crates/swc-optimizer/core/src/code_move.rs` -- segment module generation with _captures injection
- SWC reference implementation: `crates/swc-optimizer/core/src/transform.rs` lines 696-704 -- param filtering from scoped_idents
- SWC reference implementation: `crates/swc-optimizer/core/src/transform.rs` lines 3486-3580 -- transform_event_handler_with_iter_var
- SWC reference implementation: `crates/swc-optimizer/core/src/transform.rs` lines 3409-3431 -- get_function_params
- SWC reference implementation: `crates/swc-optimizer/core/src/transform.rs` lines 3582-3596 -- compute_scoped_idents
- OXC implementation: `crates/qwik-optimizer-oxc/src/transform.rs` -- current capture analysis
- OXC implementation: `crates/qwik-optimizer-oxc/src/code_move.rs` -- segment module generation
- OXC implementation: `crates/qwik-optimizer-oxc/src/collector.rs` -- compute_captures function
- Snapshot diffs: 24 test files with captures-related differences (analyzed via git diff)

## Metadata

**Confidence breakdown:**
- Architecture: HIGH - Direct comparison of SWC reference and OXC implementation
- Affected tests: HIGH - Enumerated from actual snapshot diffs
- Implementation strategy: HIGH - Clear mapping from SWC patterns to OXC changes needed
- Pitfalls: MEDIUM - Props destructuring interaction needs validation

**Research date:** 2026-02-23
**Valid until:** 2026-03-23 (stable -- core mechanism unlikely to change)
