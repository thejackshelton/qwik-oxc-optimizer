---
phase: 12-signal-wrapping-gaps
verified: 2026-02-23T18:41:24Z
status: gaps_found
score: 1/3 must-haves verified
re_verification:
  previous_status: gaps_found
  previous_score: 1/3
  gaps_closed: []
  gaps_remaining:
    - "All remaining _fnSignal wrapping gaps resolved (~23 tests)"
    - "All remaining _wrapProp wrapping gaps resolved (~14 tests)"
  regressions:
    - "destructure_args_colon_props3: was exact match, now fails due to strip_prop_alias_bindings in plan 06"
gaps:
  - truth: "All remaining _fnSignal wrapping gaps resolved (~23 tests)"
    status: failed
    reason: "26 of 27 signal-wrapping tests still have diffs; plans 04-06 are in a broken state — committed to git but partially reverted via staged changes in working tree. The one semantic gain from plan 04 (props_wrapping tests) is visible in reduced diff size but tests still fail due to cosmetic differences (import ordering, prop ordering). Plan 05 partially fixed store detection but OXC now over-wraps the class object expression. Plan 06 was committed but most code was subsequently staged for removal, leaving only strip_prop_alias_bindings which introduced a regression."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "BROKEN STATE: staged changes delete 233 lines of plan 06 implementation (enter_export_default inline detection, capture remap for inline components, body code post-processing for prop aliases). Working tree does NOT have the plan 06 inline component logic."
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "BROKEN STATE: staged changes remove the StaticMemberExpression branch that maps destructured prop aliases to _rawProps dep, and reverts has_destructured_raw_props bypass to hardcode _rawProps instead of props_param_name."
      - path: "crates/qwik-optimizer-oxc/src/lib.rs"
        issue: "BROKEN STATE: staged changes remove entry_code_refs_hf check that allowed segment-strategy inline components to inject hoisted _hf* functions."
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "REGRESSION: strip_prop_alias_bindings function (added in plan 06, NOT reverted) incorrectly strips `const test = useSignal(...)` declarations when `test` was a destructured prop alias name — even when the re-declaration is unrelated. Causes destructure_args_colon_props3 regression."
    missing:
      - "Unstage the staged reverts OR fix plan 06 to work correctly without regressions"
      - "Fix strip_prop_alias_bindings to only strip bindings where the initializer is NOT a useSignal/useStore/other reactive hook"
      - "Re-implement or restore inline component _rawProps rewrite without the colon_props3 regression"
      - "Fix props_wrapping tests: remaining diffs are import ordering and prop ordering (cosmetic). Need either import sort or accept them as cosmetic."

  - truth: "All remaining _wrapProp wrapping gaps resolved (~14 tests)"
    status: failed
    reason: "No _wrapProp tests closed. example_functional_component_2 still shows btn.name bare instead of _wrapProp. No phase 12 plan addressed _wrapProp specifically."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "Loop variable member access (btn.name) in children path not wrapped with _wrapProp; STEP_2 constant capture still incorrect"
    missing:
      - "_wrapProp detection for loop variable member access in children path (btn.name -> _wrapProp(btn, 'name'))"

  - truth: "No regressions in existing exact-match tests"
    status: failed
    reason: "destructure_args_colon_props3 was previously an exact match (in the 63 passing tests) and is now failing with a semantic regression: test.value is rendered bare instead of being wrapped with _fnSignal(_hf0, [props], _hf0_str)."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "strip_prop_alias_bindings at line 2812: strips `const test = useSignal(rest['bind:value'])` because 'test' was in non_rest_aliases (from body destructuring `const { test, ...rest } = props`). But test is being RE-DECLARED as a new const unrelated to the prop, so stripping it breaks the code."
    missing:
      - "Guard strip_prop_alias_bindings to only strip bindings where the binding name is NOT being used as a reactive hook result (useSignal, useStore etc.)"
      - "OR: remove strip_prop_alias_bindings entirely and revert the plan 06 partial implementation to a clean state"
---

# Phase 12: Signal Wrapping Gaps — Re-Verification Report

**Phase Goal:** Fix remaining _fnSignal and _wrapProp wrapping gaps — the largest category of remaining semantic diffs
**Verified:** 2026-02-23T18:41:24Z
**Status:** gaps_found
**Re-verification:** Yes — after plans 04-06 gap closure attempts

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | All remaining _fnSignal wrapping gaps resolved (~23 tests) | FAILED | 26/27 signal-wrapping tests still show diffs; plans 04-06 code is in a broken/partial state with staged reverts |
| 2 | All remaining _wrapProp wrapping gaps resolved (~14 tests) | FAILED | No _wrapProp tests closed; not addressed by any plan 04-06 |
| 3 | No regressions in existing exact-match tests | FAILED | destructure_args_colon_props3 is a NEW regression (was exact match before plans 04-06) |

**Score:** 0/3 truths verified (down from 1/3 in previous verification — "no regressions" truth now also failed)

### Snapshot Diff Count

| Point in Time | Diff Count | Composition Note |
|---------------|-----------|------------------|
| Before phase 12 (pre plans 01-06) | 100 | Baseline |
| After plans 01-02 (previous verification) | 99 | `should_wrap_logical_expression_in_template` fixed |
| After plans 04-06 (this verification) | **99** | `should_wrap_logical` still exact; `destructure_args_colon_props3` now regressed |

Net effect of plans 04-06: **+0 fixed, +1 regressed** (zero progress, one regression).

### Critical Issue: Broken Working Tree State

The most important finding is that the code currently on disk (working tree) is in a **broken intermediate state**:

- Git commits (HEAD) contain the full plan 06 implementation
- BUT staged changes delete 233 lines of plan 06 code from transform.rs
- AND delete 38 lines from jsx_transform.rs
- AND delete 11 lines from lib.rs
- Working tree matches the staged (reverted) version, NOT HEAD

This means the binary built and tested by cargo is the **reverted version** of plan 06, not the committed version. The summaries describe what was in the commits but the actual running code does not match.

### What Plans 04-06 Actually Changed (Working Tree)

**Plan 04 changes in working tree (confirmed):**
- `has_destructured_raw_props` bypass in jsx_transform.rs props path — PRESENT
- `is_object_key_position` heuristic in replace_identifier_in_code — PRESENT
- Net result: `example_props_wrapping*` tests now have semantic _fnSignal wrapping correct, but diff remains due to import ordering and prop ordering

**Plan 05 changes in working tree (confirmed):**
- `has_chain_depth(expr, 1) && (is_known_local || has_chain_depth(expr, 2))` for store detection — PRESENT
- `const_bindings` threading through collect_reactive_deps — PRESENT
- Net result: `should_wrap_store_expression` now wraps `stuff` and `class` with _fnSignal; but OXC over-wraps `class` object (SWC doesn't), so test still fails

**Plan 06 changes in working tree (partial — broken):**
- `strip_prop_alias_bindings` function — PRESENT (causes regression)
- `enter_export_default_declaration` inline component detection — ABSENT (staged for deletion)
- `exit_export_default_declaration` inline component param/body rewrite — ABSENT
- `collect_reactive_deps_inner` StaticMemberExpression prop alias branch — ABSENT (staged for deletion)
- `lib.rs` entry_code_refs_hf — ABSENT (staged for deletion)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Fixed collect_reactive_deps with sorting, expression recursion, globals, accept_call_expr, is_any_dep_used_as_object, .value detection, has_destructured_raw_props bypass | SUBSTANTIVE + WIRED | Plans 01-04 features present; plan 06 StaticMember branch ABSENT (staged revert) |
| `crates/qwik-optimizer-oxc/src/transform.rs` | Inline component _rawProps rewrite in enter/exit_export_default, const_bindings threading, strip_prop_alias_bindings | BROKEN | strip_prop_alias_bindings present (causing regression); enter_export_default inline detection ABSENT (staged revert); 233 deletions staged |
| `crates/qwik-optimizer-oxc/src/lib.rs` | entry_code_refs_hf for segment-strategy inline components | ABSENT | Staged for deletion |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `collect_reactive_deps_inner` (StaticMemberExpression) | destructured prop alias detection | prop alias loop in Static branch | NOT_WIRED | Staged for deletion; absent in working tree |
| `enter_export_default_declaration` | `active_props_info` setup | `ExportDefaultDeclarationKind::ArrowFunctionExpression` match | NOT_WIRED | Absent in working tree (staged deletion) |
| `strip_prop_alias_bindings` | body statement stripping | `non_rest_aliases.contains(name)` check | WIRED but INCORRECT | Present and executing; causes regression in colon_props3 |
| `has_chain_depth(expr,1)` + `const_bindings` | store dep detection | `is_known_local || has_chain_depth(expr,2)` | WIRED | Present; correctly handles panelStore.active |
| `has_destructured_raw_props` bypass | `is_any_dep_used_as_object` gate | `deps.iter().any(|d| d.root_name == "_rawProps")` | WIRED (partial) | Works for component$; doesn't cover props_param_name (body destr) due to staged revert |

### Regression Analysis

**destructure_args_colon_props3 (NEW regression)**

Input:
```tsx
export default component$((props) => {
  const { test, ...rest } = props;
  const test = useSignal(rest["bind:value"]);  // Re-uses 'test' name
  return <>{test.value}</>;
});
```

Root cause: `strip_prop_alias_bindings` strips `const test = useSignal(...)` because `"test"` is in `non_rest_aliases` (from the body destructuring `const { test, ...rest } = props`). After stripping, `test` is undefined and `test.value` cannot be wrapped.

Before plan 06 (SWC golden output): `_fnSignal(_hf0, [props], _hf0_str)` where `_hf0 = (p0) => p0.test.value`
After plan 06 partial: `test.value` (bare, unresolved reference)

### Tests Status Overview

| Category | Count | Status |
|----------|-------|--------|
| Total tests | 162 | — |
| Exact matches | 62 | DOWN by 1 from previous (colon_props3 regressed) |
| Snapshot diffs | 99 | Same count as after plans 01-03 |
| New exact matches from plans 04-06 | 0 | None fixed |
| New regressions from plans 04-06 | 1 | destructure_args_colon_props3 |

### Signal-Wrapping Tests Status (Updated)

| Test | Status | Change from Previous |
|------|--------|---------------------|
| destructure_args_colon_props3 | REGRESSION | Was exact match; plan 06 broke it |
| destructure_args_inline_cmp_block_stmt | DIFF | Unchanged; plan 06 code absent from working tree |
| destructure_args_inline_cmp_block_stmt2 | DIFF | Unchanged |
| destructure_args_inline_cmp_expr_stmt | DIFF | Unchanged |
| example_props_wrapping | DIFF (semantic fixed, cosmetic remains) | Plan 04 fixed _fnSignal wrapping; import/prop order diff remains |
| example_props_wrapping2 | DIFF (semantic fixed, cosmetic remains) | Same as above |
| example_props_wrapping_children | DIFF (semantic fixed, cosmetic remains) | Same as above |
| example_props_wrapping_children2 | DIFF (semantic fixed, cosmetic remains) | Same as above |
| should_wrap_store_expression | DIFF (over-wrapping) | Plan 05 added _fnSignal for stuff; but class object also wrapped (SWC doesn't) |
| should_wrap_logical_expression_in_template | EXACT | Fixed in plans 01-02; still exact |
| All other 16 signal-wrapping tests | DIFF | Unchanged |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | 2812 | `strip_prop_alias_bindings` removes const bindings matching prop alias names without checking if the binding is a re-use | BLOCKER | Causes destructure_args_colon_props3 regression |
| Working tree vs HEAD | — | 233 staged deletions in transform.rs | BLOCKER | Plan 06 inline component code never executes; working tree diverged from HEAD |

### No-Regression Check

| Test (Spot-checked) | Status |
|---------------------|--------|
| should_not_wrap_fn | EXACT |
| hoisted_fn_signal_in_loop | EXACT |
| lib_mode_fn_signal | EXACT |
| should_wrap_inner_inline_component_prop | EXACT |
| should_wrap_object_with_fn_signal | EXACT |
| should_wrap_type_asserted_variables_in_template | EXACT |
| should_wrap_logical_expression_in_template | EXACT |
| destructure_args_colon_props3 | **REGRESSED** |

## Gaps Summary

Phase 12 made **real semantic progress** in plans 01-04 (dep sorting, expression recursion, harmless globals, accept_call_expr, is_used_as_object gate, .value detection, _hf dedup, destructured prop alias bypass). This converted exactly 1 test from diff to exact match (`should_wrap_logical_expression_in_template`) and reduced the semantic gap in 4 props_wrapping tests (correct _fnSignal wrapping now, only cosmetic diffs remain).

However, plans 04-06 execution left the codebase in a **broken intermediate state**:

1. **Staged reverts**: 233 lines of plan 06 code are staged for deletion in transform.rs, 38 lines in jsx_transform.rs, 11 lines in lib.rs. Working tree does not contain the inline component _rawProps rewrite.

2. **Orphaned regression code**: `strip_prop_alias_bindings` was added by plan 06 but was NOT included in the staged reverts. It remains in the working tree and causes the `destructure_args_colon_props3` regression.

3. **Net effect of plans 04-06**: Zero new exact matches, one new regression.

**Immediate priority**: Fix the broken state by either:
- (a) Unstaging the plan 06 reverts and fixing the `strip_prop_alias_bindings` regression, OR
- (b) Completing the staged reverts (commit the deletions) and removing `strip_prop_alias_bindings` from the body-destructuring path to restore `destructure_args_colon_props3` to passing

The props_wrapping tests (plan 04's target) are now semantically correct in OXC output — remaining diffs are cosmetic (import ordering, prop ordering within JSX output). These could be considered "goal achieved" for that specific sub-goal pending import ordering fixes.

---

_Verified: 2026-02-23T18:41:24Z_
_Verifier: Claude (gsd-verifier)_
