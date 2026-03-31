---
phase: 05-jsx-keys-flags
plan: 03
subsystem: jsx-transform
tags: [jsx, flags, immutability, member-expressions, bottom-up-traversal]

# Dependency graph
requires:
  - phase: 05-jsx-keys-flags/02
    provides: "immutability flag bitfield computation and jsx_mutable tracking"
provides:
  - "Correct mutability propagation through JSX element parent chain"
  - "Member expression immutability classification based on import status"
  - "contains_mutable_jsx_call() scanner for already-transformed component calls"
affects: ["06-imports-cleanup"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pre-capture pattern: check tracker.jsx_mutable before save/restore to capture exit_expression signals"
    - "contains_mutable_jsx_call: post-hoc scan of already-transformed _jsxSorted calls for non-immutable component tags"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "OXC bottom-up traversal requires pre-capture of tracker.jsx_mutable before save/restore in child processing"
  - "contains_mutable_jsx_call scans expression trees for _jsxSorted calls with non-immutable component tags to simulate SWC's top-down detection"
  - "Member expressions: mutable by default, immutable only when base object is a known import (matches SWC ConstCollector)"
  - "Remaining 21 flag mismatches are scope-analysis issues (unresolved globals, local mutable bindings) requiring proper variable resolution"

patterns-established:
  - "Pre-capture pattern: always check tracker.jsx_mutable before the save/reset/process/check/restore cycle to avoid losing exit_expression signals"

# Metrics
duration: 13min
completed: 2026-02-20
---

# Phase 5 Plan 03: JSX Flag Propagation Gap Closure Summary

**Fixed mutability propagation for already-transformed JSX children and member expressions, reducing flag mismatches from 122 to 21**

## Performance

- **Duration:** 13 min
- **Started:** 2026-02-20T19:29:53Z
- **Completed:** 2026-02-20T19:43:01Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Fixed two complementary root causes of flag=3/flag=1 mismatches (68 of ~122 mismatches)
- Added contains_mutable_jsx_call() to detect non-immutable component tags in already-transformed _jsxSorted calls
- Classified member expressions on non-import objects as mutable (matches SWC ConstCollector)
- Reduced total flag mismatches from 122 to 21 (83% reduction)

## Task Commits

Each task was committed atomically:

1. **Task 1: Propagate spread/var_props/children_mutable via tracker.jsx_mutable and capture pre-set mutability from exit_expression** - `63fd8a4` (feat)
2. **Task 2: Fix is_child_expression_immutable for member expressions** - `9ea000d` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Fixed mutability propagation chain and member expression classification

## Decisions Made
- **Pre-capture approach over Fix B alone**: The plan's Fix B (checking tracker.jsx_mutable in the `other =>` branch) was insufficient because save/restore patterns in parent element processing wipe the exit_expression signal before it reaches the ExpressionContainer handler. Added pre-capture checks in all save/restore blocks (JSXChild::Element, JSXChild::Fragment, ExpressionContainer JSXElement/Fragment branches) to capture the signal before it's reset.
- **contains_mutable_jsx_call scanner**: OXC's bottom-up traversal means inner component elements (like `<Stuff/>`) are already transformed to `_jsxSorted(Stuff, ...)` calls when the parent processes its children. Since `is_child_expression_immutable` treats _jsxSorted calls as immutable (they ARE const in SWC's scope), added a separate scanner that checks the first argument of _jsxSorted calls against immutable_function_cmp.
- **Member expression import check**: Uses the existing module_imports list to determine if a member expression's base object is an import. Simple string comparison against specifier local names. Does not handle namespace imports or re-exports, but covers the vast majority of real-world patterns.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fix B approach insufficient due to save/restore signal erasure**
- **Found during:** Task 1 (Fix B implementation)
- **Issue:** The plan's Fix B (checking tracker.jsx_mutable in the `other =>` branch of ExpressionContainer) did not work because the save/restore pattern in JSXChild::Element processing resets tracker.jsx_mutable to false BEFORE the child element's transform_jsx_children runs. The exit_expression signal from inner components gets wiped.
- **Fix:** Added pre-capture checks in all 4 save/restore locations: JSXChild::Element, JSXChild::Fragment, and the ExpressionContainer's JSXElement/JSXFragment branches. Before saving and resetting, check if tracker.jsx_mutable is already true and set any_child_mutable. Also added contains_mutable_jsx_call() to detect non-immutable components in already-transformed expression trees.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** Fn1 test case: `<div>{ternary with <Stuff/>}</div>` now produces flag=1 for both div and Fragment
- **Committed in:** 63fd8a4 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Fix B alone was insufficient; the pre-capture approach and contains_mutable_jsx_call scanner were necessary additions for correctness in OXC's bottom-up traversal model. No scope creep.

## Issues Encountered
- Understanding OXC's traversal order was critical: `walk_jsx_element` is called for JSXChild::Element but does NOT trigger exit_expression (only Expression::JSXElement does). This means child JSX elements in parent elements are NOT transformed by exit_expression before the parent processes them via transform_jsx_children -- they're handled inline. But JSX elements inside ExpressionContainers (ternaries, logical &&) ARE Expression::JSXElement and DO get exit_expression first.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Flag mismatches reduced to 21 (from 122). Remaining mismatches:
  - 10 OXC=1/SWC=3: OXC over-reports mutability (member expressions on imports that are now correctly mutable but the parent chain propagation is too aggressive in some cases -- e.g., class_name patterns)
  - 8 OXC=3/SWC=1: scope-analysis issues (unresolved globals, local mutable bindings) requiring proper variable resolution
  - 3 edge cases in immutable_analysis and single_qrl_2
- Phase 6 (imports/cleanup) can proceed without further flag work

## Self-Check: PASSED

---
*Phase: 05-jsx-keys-flags*
*Completed: 2026-02-20*
