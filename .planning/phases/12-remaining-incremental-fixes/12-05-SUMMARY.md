---
phase: 12-signal-wrapping-gaps
plan: 05
subsystem: signal-wrapping
tags: [store, useStore, _fnSignal, reactive-deps, const_bindings, chain-depth]

requires:
  - phase: 12-signal-wrapping-gaps (plans 01-03)
    provides: dep sorting, expression recursion, harmless globals, _hf dedup

provides:
  - Single-level store member access detection (panelStore.active)
  - const_bindings-gated depth check in collect_reactive_deps_inner

affects: [12-06, phase-13]

tech-stack:
  added: []
  patterns:
    - "const_bindings as scope analysis proxy for reactive dep detection"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs

key-decisions:
  - "const_bindings used as proxy for 'locally declared variable' to avoid wrapping free variables"
  - "Depth >= 2 chains bypass const_bindings check (unambiguously store chains)"
  - "Depth 1 chains require const_bindings membership (prevents false positives on free vars)"

patterns-established:
  - "const_bindings threading: pass through collect_reactive_deps -> collect_reactive_deps_inner for scope-aware store detection"

duration: 19min
completed: 2026-02-23
---

# Phase 12 Plan 05: Store Chain Depth Detection Summary

**Lowered store chain depth requirement from 2 to 1 with const_bindings guard for single-level store member access (panelStore.active) recognition**

## Performance

- **Duration:** 19 min
- **Started:** 2026-02-23T17:44:13Z
- **Completed:** 2026-02-23T18:03:23Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- `panelStore.active` (depth 1 store member access from useStore()) now recognized as reactive dep and wrapped with _fnSignal
- `const_bindings` check prevents false positives on free variables (e.g., `state.thing` where `state` is undeclared)
- Multi-level store chains (depth >= 2, e.g., `store.errors.test`) continue to work without needing the const_bindings check
- Zero regressions: 99 snapshot diffs before = 99 after

## Task Commits

Each task was committed atomically:

1. **Task 1: Reduce store chain depth requirement from 2 to 1** - `399cac0` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added const_bindings parameter to collect_reactive_deps/collect_reactive_deps_inner, changed depth check from has_chain_depth(expr, 2) to has_chain_depth(expr, 1) with is_known_local guard

## Decisions Made

1. **const_bindings as scope proxy:** SWC uses full scope analysis to distinguish local variables from free variables. OXC approximates this using `const_bindings` (which contains const declarations and imports). Since stores are always assigned to `const` (from useStore/useSignal), this is a reliable proxy. Free variables like `state` in `example_skip_transform` are correctly excluded because they're not in const_bindings.

2. **Dual condition for depth check:** `has_chain_depth(expr, 1) && (is_known_local || has_chain_depth(expr, 2))`. For depth-1 chains (panelStore.active), the const_bindings check is required. For depth-2+ chains (store.errors.test), they pass unconditionally since deep member chains are unambiguously store access patterns.

3. **const_bindings threading:** Added const_bindings as a parameter through the entire collect_reactive_deps call chain (outer function, inner function, all 28 recursive calls, and both external call sites) rather than using a global or struct field.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Prevented false positive wrapping of free variables**
- **Found during:** Task 1
- **Issue:** Simply changing `has_chain_depth(expr, 2)` to `has_chain_depth(expr, 1)` caused `example_skip_transform` to incorrectly wrap `state.thing` (where `state` is a free variable) with _fnSignal. SWC doesn't wrap free variables because its IdentCollector uses scope analysis.
- **Fix:** Added `const_bindings` parameter to `collect_reactive_deps` functions and gated the depth-1 check on `const_bindings.contains(root_name)`. This ensures only known local declarations (const bindings from useStore/useSignal) trigger depth-1 store detection.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** `example_skip_transform.snap.new` no longer generated (exact match with SWC), while `should_wrap_store_expression.snap.new` shows `panelStore.active` wrapped correctly
- **Committed in:** 399cac0

---

**Total deviations:** 1 auto-fixed (1 bug prevention)
**Impact on plan:** Essential fix to prevent false positive wrapping. The plan didn't account for free variables in the depth check analysis. The const_bindings guard adds the necessary scope awareness.

## Issues Encountered
- File caching between Read/Edit tools and Python-based edits caused apparent "reverts" -- resolved by using Bash-based Python scripts for all edits and avoiding interleaving Read tool calls with Python writes

## Next Phase Readiness
- Store detection now handles both depth-1 (panelStore.active) and depth-2+ (store.errors.test) patterns
- should_wrap_store_expression test now correctly wraps both `stuff` and `class` props with _fnSignal
- Ready for plan 06 (remaining signal wrapping gaps)

---
*Phase: 12-signal-wrapping-gaps*
*Completed: 2026-02-23*
