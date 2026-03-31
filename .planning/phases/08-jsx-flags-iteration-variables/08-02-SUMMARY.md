---
phase: 08-jsx-flags-iteration-variables
plan: 02
subsystem: jsx-transform
tags: [qwik, oxc, jsx, iteration-variables, q:p, static-listeners, useResource$]

# Dependency graph
requires:
  - phase: 08-01
    provides: "Scope-aware JSX flag classification with const_bindings"
  - phase: 04-02
    provides: "Iteration variable tracking (loop_depth, iteration_var_stack, in_callback_depth)"
provides:
  - "q:p/q:ps injection via side-channel iteration variable tracking at correct element level"
  - "static_listeners clearing when q:p/q:ps present in var_props"
  - "_rawProps parameter override for useResource$ hooks"
  - "Deep identifier scanning for nested lambda iteration variable detection"
affects: [08-03, snapshot-parity]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "iter_var_usage_by_handler HashMap for per-element q:p injection"
    - "analyze_lambda_deep_ident_refs for full-depth ident scanning"
    - "q:p/q:ps injection during replace_jsx_element_handlers (pre-JSX-transform)"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "q:p injection moved from transform_jsx_element_inner to replace_jsx_element_handlers for correct element-level injection"
  - "Deep ident scan (analyze_lambda_deep_ident_refs) descends into nested functions for iteration variable detection"
  - "iter_var_usage_by_handler keyed by lambda span start enables per-handler tracking"
  - "useResource$ gets full props destructuring rewrite (param + body), not just parameter replacement"
  - "Body-level destructuring detection gated to component$ only (non-destructured props pattern)"
  - "Child segment capture reclassification gated to component$ only"

patterns-established:
  - "Side-channel pattern: data recorded during enter phase, consumed during pre-transform phase"
  - "Deep vs shallow ident scanning: captures use shallow (function scope), iteration vars use deep"

# Metrics
duration: 13min
completed: 2026-02-21
---

# Phase 8 Plan 2: q:p Injection & Iteration Variable Fixes Summary

**Side-channel q:p/q:ps injection at element level via iter_var_usage_by_handler, static_listeners clearing, and useResource$ _rawProps override**

## Performance

- **Duration:** 13 min
- **Started:** 2026-02-21T11:28:35Z
- **Completed:** 2026-02-21T11:42:10Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- q:p/q:ps attributes now injected at the correct JSX element (where the event handler is), not at the top-level loop element
- static_listeners correctly cleared (bit 0 = false) when q:p/q:ps present in var_props, matching SWC behavior
- Deep identifier scanning handles iteration variables captured by nested closures (e.g., `row` inside `.findIndex((d) => ... row.value.id ...)`)
- useResource$ hooks get full _rawProps parameter rewriting (paramNames shows ["_rawProps"])
- All 10 affected test files now have q:p/q:ps lines in their snapshots
- Removed dead code (expr_uses_ident, stmt_uses_ident, arg_uses_ident)

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix q:p injection via side-channel iteration variable tracking** - `d89a80a` (feat)
2. **Task 2: Extend _rawProps override to useResource$ + full verification** - `970cfe6` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - iter_var_usage_by_handler HashMap, q:p injection in replace_jsx_element_handlers, deep ident scanning, useResource$ props rewrite guard
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Removed broken q:p injection from transform_jsx_element_inner, static_listeners clearing for q:p/q:ps, removed dead expr_uses_ident functions

## Decisions Made

1. **q:p injection location changed from plan** - Plan specified q:p injection via pre_recorded_iter_vars parameter to transform_jsx_element_inner. This was incorrect because nested child elements (not the top-level element) have the event handlers. Changed to inject during replace_jsx_element_handlers where each element's handlers are individually processed.

2. **Deep ident scanning required** - Plan didn't anticipate that analyze_lambda_captures doesn't descend into nested functions. For iteration variable detection, SWC does full deep scanning (body_contains_ident). Added analyze_lambda_deep_ident_refs to match SWC behavior.

3. **useResource$ gets full body rewrite** - Plan said "parameter replacement only, no body destructuring". SWC actually does full rewrite (`{track}` -> `_rawProps`, `track(...)` -> `_rawProps.track(...)`). Implemented full rewrite matching SWC.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] q:p injection at wrong element level**
- **Found during:** Task 1
- **Issue:** Plan's approach (draining pending_iter_var_usage at top-level exit_expression) would inject q:p at the outermost element in the loop body instead of the specific element with event handlers
- **Fix:** Changed to per-handler tracking via iter_var_usage_by_handler HashMap keyed by lambda span start, injecting q:p in replace_jsx_element_handlers
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Committed in:** d89a80a

**2. [Rule 1 - Bug] Missing iteration variables in nested closures**
- **Found during:** Task 1
- **Issue:** analyze_lambda_captures doesn't descend into nested arrow/function expressions (correct for capture scoping). But iteration variable detection needs deep descent since vars can be captured by nested closures
- **Fix:** Added analyze_lambda_deep_ident_refs + walk_expression_deep_idents + walk_statement_deep_idents for full-depth scanning
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Committed in:** d89a80a

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Both bugs were fundamental correctness issues that would have produced wrong output. No scope creep.

## Issues Encountered
- The plan's approach to q:p injection was architecturally flawed (injecting at wrong element level). Redesigned to use per-handler tracking with HashMap keyed by lambda span start, consumed during the element-level handler replacement phase.

## Next Phase Readiness
- q:p/q:ps injection working correctly at element level for all 10 affected test files
- Flag mismatches reduced (static_listeners now correctly false when q:p present)
- useResource$ _rawProps matching SWC
- Ready for 08-03 (remaining flag edge cases and cleanup)
- 138 snapshot files still differ but diff line count reduced

## Self-Check: PASSED

---
*Phase: 08-jsx-flags-iteration-variables*
*Completed: 2026-02-21*
