---
phase: 05-jsx-keys-flags
plan: 02
subsystem: jsx-transform
tags: [jsx, flags, bitfield, static_listeners, static_subtree, immutability]

# Dependency graph
requires:
  - phase: 05-jsx-keys-flags/01
    provides: JSX key generation (root_jsx_mode, key prefix)
  - phase: 04-signal-props-transforms
    provides: Signal wrapping (_wrapProp, _fnSignal) in props and children
provides:
  - Proper static_listeners + static_subtree bitfield flag computation per JSX element
  - immutable_function_cmp set for component tag mutability classification
  - jsx_mutable tracking for parent-child mutability propagation
affects: [06-imports-cleanup]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Bitfield flag encoding: bit 0 = static_listeners, bit 1 = static_subtree"
    - "jsx_mutable save/restore pattern for nested element processing"
    - "Mutability check BEFORE signal wrapping for named props, AFTER for signal.value"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs

key-decisions:
  - "immutable_function_cmp built in QwikTransform::new() from collected imports (Fragment, RenderOnce, Link, ?jsx/.md)"
  - "jsx_mutable and immutable_function_cmp stored on ImportTracker (accessible from jsx_transform.rs)"
  - "WrapPropSignal (signal.value) keeps immutable (SWC is_const=true); WrapPropNamed (props) marks mutable (SWC is_const=false)"
  - "Identifiers and member expressions treated as immutable in children (approximates SWC scope analysis; may miss global refs)"
  - "transform_jsx_children returns 3-tuple (expr, count, any_child_mutable) for flag computation"

patterns-established:
  - "Save/restore tracker.jsx_mutable around nested JSX element/fragment processing"
  - "Component tag mutability: is_fn && !immutable_function_cmp.contains(name) -> tracker.jsx_mutable = true"
  - "Flag bitfield: flags |= 1 (static_listeners) | 2 (static_subtree) per element"

# Metrics
duration: 14min
completed: 2026-02-20
---

# Phase 5 Plan 02: JSX Immutability Flags Summary

**Proper static_listeners + static_subtree bitfield flag computation replacing simplistic children_count heuristic, with immutable_function_cmp set and jsx_mutable parent-child propagation**

## Performance

- **Duration:** 14 min
- **Started:** 2026-02-20T17:59:00Z
- **Completed:** 2026-02-20T18:13:00Z
- **Tasks:** 2 (+ 1 deviation fix)
- **Files modified:** 2

## Accomplishments

- Replaced `children_count > 1 ? 1 : 3` with proper `static_listeners | static_subtree` bitfield
- Built immutable_function_cmp set from collected imports (Fragment, RenderOnce, Link, ?jsx/.md sources)
- Added jsx_mutable tracking with save/restore around nested elements for parent-child propagation
- Correct flag values across ~160 snapshot files (3880 insertions, 7050 deletions in snapshot diffs)
- WrapPropNamed correctly marks children as mutable while WrapPropSignal keeps immutable

## Task Commits

Each task was committed atomically:

1. **Task 1: Build immutable_function_cmp set and add jsx_mutable tracking** - `e09d66d` (feat)
2. **Task 2: Implement proper flag computation** - `2376885` (feat)
3. **Deviation fix: Correct mutability check ordering** - `496950f` (fix)

## Files Created/Modified

- `crates/qwik-optimizer-oxc/src/transform.rs` - Added immutable_function_cmp HashSet and jsx_mutable bool to ImportTracker, populated from collected imports in QwikTransform::new()
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Replaced flag computation, added is_child_expression_immutable helper, updated transform_jsx_children to return 3-tuple, added component tag mutability check, updated all callers

## Decisions Made

1. **immutable_function_cmp on ImportTracker** -- Stored on ImportTracker rather than QwikTransform because jsx_transform.rs functions already have &mut ImportTracker access, avoiding new parameter threading.

2. **Mutability check ordering** -- For WrapPropNamed (destructured props, props.X), mutability is determined BEFORE signal wrapping (marking mutable). For WrapPropSignal (signal.value), the wrapping itself makes it immutable. For _fnSignal, the result is treated as immutable. This matches SWC's convert_to_signal_item behavior.

3. **Identifier constness approximation** -- Without full scope analysis, all identifiers and member expressions are treated as immutable in is_child_expression_immutable. This matches SWC for the vast majority of cases (local variables, imports) but may incorrectly treat global references as immutable. The alternative (marking all identifiers as mutable) would cause far more mismatches.

4. **Fragment flags** -- Fragments have no props, no spread, no event handlers, so static_listeners is always true (bit 0 = 1). static_subtree depends only on children mutability.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Mutability check ordering for signal wrapping**
- **Found during:** Task 2 verification
- **Issue:** Plan said to check mutability "AFTER signal wrapping attempts" but SWC checks mutability on the ORIGINAL expression before wrapping. WrapPropNamed should mark mutable (SWC is_const=false), WrapPropSignal should keep immutable (SWC is_const=true). Initial implementation incorrectly checked AFTER, causing _wrapProp(props, "bind:value") to get flag=3 instead of SWC's flag=1.
- **Fix:** Restructured the flow: WrapPropSignal continues without marking mutable, WrapPropNamed explicitly marks any_child_mutable=true, _fnSignal continues without marking mutable, and non-wrapped expressions fall through to is_child_expression_immutable.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** destructure_args_colon_props.snap now produces flag=1 matching SWC
- **Committed in:** 496950f

**2. [Rule 1 - Bug] is_child_expression_immutable too restrictive for identifiers**
- **Found during:** Task 2 verification
- **Issue:** Initial is_child_expression_immutable delegated to is_const_expression which returns false for ALL identifiers. But SWC treats in-scope identifiers (locals, imports) as const, only marking unresolved globals as non-const. This caused `signal`, `dep`, `dep.thing` to get flag=1 instead of SWC's flag=3.
- **Fix:** Rewrote is_child_expression_immutable to treat identifiers and member expressions as immutable. Added explicit handling for binary, conditional, logical, template, unary expressions. Only function calls (non-known) and tagged templates remain mutable.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** derived_signals_children.snap now matches SWC for signal, dep, dep.thing cases
- **Committed in:** 496950f

---

**Total deviations:** 2 auto-fixed (2 bugs discovered during verification)
**Impact on plan:** Both fixes were necessary for correctness. The plan's description of mutability check ordering was incorrect -- SWC's actual behavior differs from the plan's analysis in subtle ways.

## Issues Encountered

- **Remaining globalThing mismatch**: SWC marks `globalThing` (unresolved global identifier) as mutable (flag=1) while our code marks it as immutable (flag=3) because we treat all identifiers as const. This affects only ~3 test expressions in the example_derived_signals_children fixture. Fixing this properly requires scope analysis (knowing which identifiers are in-scope vs global), which is architectural (Rule 4). The impact is minimal in real Qwik code where global references in JSX children are rare.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Phase 5 (JSX Keys & Flags) is complete
- Phase 6 (imports cleanup, deferred items) is next:
  - Import ordering
  - Indentation/formatting normalization (tabs vs spaces)
  - use*() return value destructuring inlining (2 fixtures)
  - QRL hoisting (deferred from 04-02)
  - should_extract_single_qrl_2 dedup suffix naming issue
- Remaining snapshot diffs are primarily formatting/import order issues, not semantic

## Self-Check: PASSED

---
*Phase: 05-jsx-keys-flags*
*Completed: 2026-02-20*
