---
phase: 09-jsx-keys-final-parity
plan: 02
subsystem: transform
tags: [_wrapProp, _fnSignal, className, signal-wrapping, captures, import-set]

# Dependency graph
requires:
  - phase: 09-01
    provides: "Pure var DCE, entry field, event naming, windows paths"
  - phase: 08
    provides: "JSX flag scope analysis, q:p injection, static_subtree/static_listeners"
provides:
  - "_wrapProp wrapping for generic local variable member expressions (state.text, store.count)"
  - "className->class transform for native HTML elements"
  - "is_text_only element detection (title, textarea, script, etc.) skipping signal wrapping"
  - "Correct is_const flag on WrapPropNamed for const local variables"
  - "Chained const capture format matching SWC single VariableDeclaration"
  - "Local Qrl function self-imports instead of false-positive core imports"
affects: ["09-03", "09-04", "09-05"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "is_text_only_element helper matching SWC text-only element detection"
    - "const_bindings + module_imports for generic local variable signal wrapping"
    - "WrapPropNamed(String, bool) with is_const flag for prop classification"
    - "Chained const capture restoration in segment modules"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
    - "crates/qwik-optimizer-oxc/src/code_move.rs"
    - "crates/qwik-optimizer-oxc/src/transform.rs"

key-decisions:
  - "Generic local variable wrapping uses const_bindings (not scope analysis) to identify known local declarations"
  - "is_text_only elements skip all signal wrapping and mark children as mutable (matching SWC)"
  - "WrapPropNamed carries is_const bool: true for const locals, false for props/_rawProps"
  - "Entry module import ordering left as encounter-order mismatch (SWC traversal-dependent, would need major refactoring)"
  - "Locally-defined Qrl functions reclassified from segment_qrl_names to needed_imports as self-imports"

patterns-established:
  - "is_text_only_element: regex-free match for title|textarea|option|script|style|noscript"
  - "Local Qrl self-import pattern: check module_level_decls before adding to segment_qrl_names"

# Metrics
duration: ~45min
completed: 2026-02-21
---

# Phase 9 Plan 2: Signal Wrapping, className, Captures, and Import Set Summary

**_wrapProp for generic local variable member access (state.text), className->class transform, chained capture format, and local Qrl self-import fix reducing snapshots from 131 to 129**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-02-21T14:48:00Z (estimated)
- **Completed:** 2026-02-21T15:33:24Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- _wrapProp now wraps generic local variable member expressions (state.text, store.count) in both JSX children and props
- className attribute converted to class for native HTML elements, kept as-is for components
- Text-only elements (title, textarea, etc.) correctly skip signal wrapping
- Capture restoration uses chained const format matching SWC exactly
- False-positive synthetic imports for locally-defined Qrl functions eliminated
- 2 new exact golden snapshot matches gained (131->129 diffing files)

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix _wrapProp signal wrapping and className->class transform** - `9199cdc` (feat)
2. **Task 2: Capture formatting, local Qrl imports, entry module import set** - `a1418a0` (fix)

## Files Created/Modified

- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added generic local variable _wrapProp detection, className->class rename, is_text_only_element helper, WrapPropNamed is_const flag
- `crates/qwik-optimizer-oxc/src/code_move.rs` - Changed capture restoration from individual const statements to chained const declaration
- `crates/qwik-optimizer-oxc/src/transform.rs` - Local Qrl function reclassification to self-imports, entry module synthetic import filtering

## Decisions Made

1. **Generic local variable wrapping uses const_bindings**: Rather than implementing full scope analysis, we check `const_bindings.contains(obj_name) && !is_import` to identify known local declarations. This catches useStore/useSignal results without false positives on undeclared free variables.

2. **is_text_only elements skip ALL signal wrapping**: For elements like `<title>`, `<textarea>`, `<script>`, children are kept as-is and marked mutable. This matches SWC's `is_text_only()` check at lines 3787-3791.

3. **WrapPropNamed carries is_const flag**: `true` for const local variables (no mutable flag, stays in const_props), `false` for props/_rawProps (sets mutable flag, goes to var_props). This fixes flag mismatches where wrapped const locals were incorrectly classified.

4. **Entry module import ordering deferred**: SWC's synthetic import order is encounter-order during fold traversal, not alphabetical. Matching this would require tracking the order in which framework imports are first used during transform traversal -- a substantial refactoring. Only 1 remaining test has purely import-order diffs.

5. **Local Qrl self-imports**: When a Qrl-suffixed name (e.g., `useMemoQrl`) is a module-level declaration, it's imported from the self-module (`"./test"`) not from `@qwik.dev/core`. This is done during `finalize_segments` by checking `module_level_decls`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed regression in example_skip_transform (free variable wrapping)**
- **Found during:** Task 1
- **Issue:** First implementation wrapped `state.thing` where `state` is an undeclared free variable (not in any scope). SWC's `decl_stack` wouldn't contain it.
- **Fix:** Added `const_bindings` parameter to `detect_signal_wrap`; only wraps when `const_bindings.contains(obj_name) && !is_import`
- **Files modified:** jsx_transform.rs
- **Committed in:** 9199cdc

**2. [Rule 1 - Bug] Fixed regression in example_spread_jsx (text-only element wrapping)**
- **Found during:** Task 1
- **Issue:** `head.title` inside `<title>` element was being wrapped with `_wrapProp`, but SWC has `is_text_only` check that skips wrapping for text-only elements
- **Fix:** Added `is_text_only_element()` function and `is_text_only` parameter to `transform_jsx_children`
- **Files modified:** jsx_transform.rs
- **Committed in:** 9199cdc

**3. [Rule 1 - Bug] Fixed flag mismatch for const local variable _wrapProp (3 vs 1 flags)**
- **Found during:** Task 1
- **Issue:** `_wrapProp(state, "text")` where `state` is a const local variable was setting `any_child_mutable = true`, breaking `static_subtree` computation
- **Fix:** Added `is_const` bool to `WrapPropNamed` enum variant; const local variables get `is_const=true` and don't set mutable flag
- **Files modified:** jsx_transform.rs
- **Committed in:** 9199cdc

---

**Total deviations:** 3 auto-fixed (3 bugs)
**Impact on plan:** All fixes necessary for correct signal wrapping behavior. No scope creep.

## Issues Encountered

- Entry module synthetic import ordering is encounter-order in SWC (not alphabetical), making it impractical to match without significant refactoring. Only 1 test has purely import-order diffs after the other fixes.
- Import assertions (`with { type: "json" }`) being stripped is a separate issue found during investigation, not addressed in this plan.
- _fnSignal inline component support (destructure_args_* tests using _rawProps with _fnSignal) was deferred per plan guidance ("if context pressure builds, _fnSignal can be deferred").

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Signal wrapping foundation complete for generic local variables
- Remaining 129 snapshot files differ with ~5292 diff lines
- Key remaining areas: inline component _fnSignal wrapping (deferred from this plan), capture analysis differences, inlined segment rendering, dev mode metadata, code stripping
- Import ordering is a minor contributor (only 1 purely import-order diff remaining)

## Self-Check: PASSED

---
*Phase: 09-jsx-keys-final-parity*
*Completed: 2026-02-21*
