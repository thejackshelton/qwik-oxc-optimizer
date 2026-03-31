---
phase: 10-captures-mechanism
verified: 2026-02-23T10:11:36Z
status: gaps_found
score: 4/6 must-haves verified
gaps:
  - truth: "Placeholder param names are de-duplicated: position 0 gets '_', position 1 gets '_1' (matching SWC's private_ident! codegen de-duplication)"
    status: partial
    reason: "Metadata intentionally stores duplicate '_' (matching SWC golden), de-duplication to '_1' happens correctly in inject_iteration_params() codegen. All 10 affected test segments show '(_, _1, X) =>' signatures matching golden references."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/code_move.rs"
        issue: "none - inject_iteration_params() correctly de-duplicates via seen HashSet"
    missing: []
  - truth: "Iteration variable names (positions 2+) in param_names are NOT present in capture_names for JSX event handler segments"
    status: verified
    reason: "All 10 iteration-variable tests show no iteration vars in captureNames. QRL call sites show [selectedItem] not [row, selectedItem]."
    artifacts: []
    missing: []
  - truth: "Segment module exported functions include iteration variable params in their signatures (e.g., (_, _1, row) =>) instead of bare () =>"
    status: verified
    reason: "All 10 affected tests show matching (_, _1, X) => counts between golden ref and new output."
    artifacts: []
    missing: []
  - truth: "QRL call sites in entry module have correct capture arrays (iteration vars excluded)"
    status: verified
    reason: "should_extract_single_qrl shows qrl(i_X, 'name', [selectedItem]) not [row, selectedItem]. All verified tests exclude iteration vars from QRL arrays."
    artifacts: []
    missing: []
  - truth: "Segment modules import _captures and inject const var = _captures[N] statements only for true captures (not iteration variable params)"
    status: partial
    reason: "Correct for main iteration-var tests. Two tests have broken capture scope: should_transform_nested_loops inner handler misses 'row' capture (capture_stack filter removes outer-loop vars); should_extract_single_qrl_with_nested_components and should_transform_component_with_normal_function incorrectly capture 'item'/'props' for nested component$ (capture_stack scope doesn't track regular function/callback scopes). These are pre-existing issues noted in SUMMARY."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "capture_stack only tracks $()-body scopes, not regular function/callback scopes. Outer-loop vars used in inner $()-handler bodies get filtered out by capture_stack instead of captured."
    missing:
      - "capture_stack scope tracking for regular functions/callbacks (Phase 11 or later)"
  - truth: "The $() exit_expression path is unaffected (analyze_lambda_captures already adds arrow params to body_local_decls, so compute_captures naturally filters them)"
    status: verified
    reason: "The $() path in transform.rs exit_expression (around line 2802) was not modified. Behavior confirmed: the iter_var filtering only applies in the JSX event handler path. Pre-existing $() tests (example_multi_capture, etc.) show the same diffs they had before phase 10 - no regressions introduced."
    artifacts: []
    missing: []
---

# Phase 10: Captures Mechanism Verification Report

**Phase Goal:** Implement `_captures[N]` array access pattern for extracted segments, replacing function parameter capture approach
**Verified:** 2026-02-23T10:11:36Z
**Status:** gaps_found (partial success - core mechanism works, pre-existing scope gaps remain)
**Re-verification:** No - initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Placeholder param names de-duplicated: `_` → `_1` in codegen | PARTIAL | Metadata stores `["_", "_", "row"]` matching SWC golden. Code generates `(_, _1, row) =>` via inject_iteration_params() de-duplication. All 10 tests match. |
| 2 | Iteration vars NOT present in captureNames for JSX event handlers | VERIFIED | 0 iteration vars in captureNames for all 10 affected tests. QRL arrays exclude row/item/i/key. |
| 3 | Segment exports include iter var params: `(_, _1, row) =>` | VERIFIED | 10/10 tests have matching `(_, _1, X)` count between golden and new output. |
| 4 | QRL call sites have correct capture arrays (no iteration vars) | VERIFIED | `should_extract_single_qrl` shows `[selectedItem]` not `[row, selectedItem]`. Confirmed across all 10 tests. |
| 5 | Segment modules import _captures and inject `const var = _captures[N]` only for true captures | PARTIAL | Correct for 8/10 tests. 2 tests (should_transform_nested_loops inner handler, should_extract_single_qrl_with_nested_components) have broken capture scope due to pre-existing capture_stack issue. |
| 6 | $() exit_expression path is unaffected | VERIFIED | No changes to $() path. Pre-existing tests show same diffs as before - no regressions. |

**Score:** 4/6 fully verified (2 partial - one is a known correct deviation, one is a pre-existing gap)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | Param filtering from captures after compute_captures() | VERIFIED | 39 lines added. iter_var_param_names extraction (line 1198), iter_var_set filtering (line 1269), current_iteration_vars() uses .last() (line 491). |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | Iteration variable param injection into segment function signatures | VERIFIED | 66 lines added. inject_iteration_params() function (line 419), Phase 6 restructuring with param injection before capture injection (line 343). |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| transform.rs capture filtering | code_move.rs param injection | SegmentData.param_names and capture_names | WIRED | param_names[2..] drives both the capture filter and the inject_iteration_params() call. Verified in should_extract_single_qrl: param_names=["_","_","row"], captureNames=["selectedItem"], signature="(_, _1, row)". |
| current_iteration_vars() | JSX event handler path | iteration_var_stack.last() | WIRED | Changed from .iter().flatten() (all scopes) to .last() (innermost only), matching SWC. |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| ~20 tests with capture-mechanism diffs resolved | PARTIAL | Core mechanism works for 10 tests. 2 tests have pre-existing capture_stack scope gaps (not introduced by phase 10). Remaining diffs in other tests are from _fnSignal signal optimization (separate domain) or JSON field ordering (aesthetic). |
| _captures import emitted in segment files that use mechanism | VERIFIED | 6/6 _captures imports match in example_component_with_event_listeners_inside_loop; 2/2 in should_extract_single_qrl; 0/0 in tests without captures. |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None found | - | - | - | - |

### Human Verification Required

None - all verification done programmatically via snapshot diff analysis.

### Gaps Summary

**Pre-existing gap (not introduced by Phase 10, documented in SUMMARY):**

1. **should_transform_nested_loops inner handler** - The inner handler `Foo_component_div_div_p_q_e_click` should capture `row` from the outer loop (golden: `captures: true, captureNames: ["row"]`), but OXC outputs `captures: false`. Root cause: the capture_stack filter removes `row` because it's present in the outer $()-body's scope frame, preventing it from being recognized as a capture for the inner handler. This is a pre-existing capture_stack scope issue.

2. **should_extract_single_qrl_with_nested_components and should_transform_component_with_normal_function** - The outer `component$` body incorrectly captures `item`/`props` from the `.map()` callback's scope. Root cause: capture_stack only tracks `$()` body scopes, not regular function/callback scopes. OXC incorrectly includes outer `component$`'s captures from inner `.map()` callback iteration vars.

3. **should_extract_single_qrl_2** - Segment naming suffix order is swapped (`_1` assigned to the non-iteration-var segment instead of the iteration-var segment). This appears to be a separate naming/ordering issue unrelated to the captures mechanism itself.

**What Phase 10 successfully delivered:**

For the 10 primary iteration-variable tests:
- Function signatures: All 10 tests now show `(_, _1, X) =>` matching golden reference (0 had this before phase 10 since inject_iteration_params() didn't exist)
- Capture filtering: All 10 tests show no iteration vars in captureNames 
- QRL call sites: All 10 tests show correct capture arrays excluding iteration vars
- `_captures` injection: Correct for 8/10 tests (2 have pre-existing scope issues)

**Remaining diffs in Phase 10 test set by category:**
- `_fnSignal` signal optimization differences: `should_transform_multiple_event_handlers`, `should_transform_multiple_event_handlers_case2`, `should_transform_nested_loops`, `example_component_with_event_listeners_inside_loop` - NOT captures mechanism, separate issue
- JSON field ordering (captureNames before/after paramNames): `should_extract_single_qrl`, `should_extract_single_qrl_with_index` - aesthetic, not semantic
- Pre-existing capture_stack scope: `should_transform_nested_loops`, `should_extract_single_qrl_with_nested_components`, `should_transform_component_with_normal_function`

---

_Verified: 2026-02-23T10:11:36Z_
_Verifier: Claude (gsd-verifier)_
