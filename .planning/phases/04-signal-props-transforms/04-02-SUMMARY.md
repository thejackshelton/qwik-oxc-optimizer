---
phase: 04-signal-props-transforms
plan: 02
subsystem: loop-tracking
tags: [loops, iteration, q:p, paramNames, jsx, event-handlers]

requires:
  - phase: 04-signal-props-transforms
    provides: "Signal wrapping (_wrapProp, _fnSignal) from 04-01"
provides:
  - "Loop depth tracking for for/for-in/for-of/while/iteration methods"
  - "q:p/q:ps injection on JSX elements with event handlers inside loops"
  - "Event handler paramNames with placeholder + iteration variable parameters"
  - "Iteration method detection (.map/.filter/.forEach etc.) with depth counter"
affects: [05-jsx-keys-flags, 06-cleanup]

tech-stack:
  added: []
  patterns: ["enter/exit hook pairs for loop constructs with depth counter", "iteration variable stack flattening for nested loops", "q:p/q:ps var_prop injection based on iteration variable usage analysis"]

key-files:
  created: []
  modified: ["crates/qwik-optimizer-oxc/src/transform.rs", "crates/qwik-optimizer-oxc/src/jsx_transform.rs"]

key-decisions:
  - "Iteration method tracking uses in_callback_depth u32 counter (not bool) to handle nested .map inside .map correctly"
  - "SWC uses '_' for both placeholder params (positions 0 and 1), not '_' and '_1'"
  - "q:p/q:ps keys use string literal format in var_props (colon requires quoting)"
  - "QRL hoisting deferred to Phase 6: OXC Traverse hoisted_function_stmts only injects at module top level via exit_program, not into function-level blocks"
  - "Removed unused loop_depth() getter method (field accessed directly via self.loop_depth)"

duration: 25min
completed: 2026-02-20
---

# Phase 4 Plan 2: Loop Tracking + q:p Injection Summary

**Loop/iteration tracking infrastructure with q:p injection on JSX elements and event handler parameter transformation for iteration variables**

## Performance

- **Duration:** 25 min
- **Started:** 2026-02-20T13:20:00Z
- **Completed:** 2026-02-20T13:45:00Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Full loop tracking infrastructure: ForStatement, ForInStatement, ForOfStatement, WhileStatement enter/exit hooks
- Iteration method detection (.map/.filter/.forEach/.flatMap/.some/.every/.find/.findIndex/.reduce/.reduceRight) with depth counting
- q:p/q:ps injection in JSX var_props for event handlers that reference iteration variables
- Event handler paramNames transformation: placeholder params ("_", "_") + iteration variable params appended
- Recursive expr_uses_ident/stmt_uses_ident/arg_uses_ident helpers for checking iteration variable usage

## Task Commits

Each task was committed atomically:

1. **Task 1: Add loop tracking state and enter/exit hooks** - `6103d24` (feat)
2. **Task 2: Implement q:p injection and event handler parameter transformation** - `c91152a` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Loop tracking state fields (loop_depth, iteration_var_stack, in_callback_depth), enter/exit hooks for all loop constructs, iteration method detection in enter/exit_call_expression, event handler parameter injection, extract_callback_params helper
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - q:p/q:ps injection in transform_jsx_element_inner, loop state threading through all recursive JSX transform functions (transform_jsx_fragment_inner, transform_jsx_children, jsx_attr_value_to_expression, transform_jsx_element_custom_source), expr_uses_ident/stmt_uses_ident/arg_uses_ident helpers

## Decisions Made

1. **Depth counter vs boolean**: `in_callback_depth` is a u32 counter, not a bool. Nested iteration methods (e.g., `arr.map(x => x.items.filter(y => ...))`) must correctly track depth -- exiting the inner `.filter()` decrements to 1 (still inside `.map()`), not 0.

2. **Placeholder param naming**: SWC uses `"_"` for BOTH placeholder positions (index 0 and 1). Initial implementation used `"_"` and `"_1"` which produced `["_", "_1", "item"]` instead of the expected `["_", "_", "item"]`.

3. **q:p key formatting**: var_props keys with special characters (colons, dashes, dollar signs) must use string literal format (`"q:p"`) not identifier format (`q:p`). Applied the same colon/dash/dollar check already used for const_props.

4. **QRL hoisting deferred**: Converting inline QRL calls to `const SegmentName = qrl(...)` variable declarations above loops is deferred to Phase 6. The OXC Traverse architecture's `hoisted_function_stmts` pattern only injects at module top level via `exit_program`, not into function-level blocks where loop-hoisted QRLs need to go.

5. **Removed unused getter**: The `loop_depth()` getter method was never called (field accessed directly as `self.loop_depth` throughout the codebase). Removed to eliminate compiler warning.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] OXC BindingPattern vs BindingPatternKind**
- **Found during:** Task 1
- **Issue:** Plan used `BindingPatternKind::BindingIdentifier(ref ident) = d.id.kind` but OXC v0.113.0 uses `BindingPattern` directly (no `BindingPatternKind` suffix) and `d.id` IS the pattern (no `.kind` field)
- **Fix:** Changed all references to `BindingPattern::BindingIdentifier(ref ident) = d.id` and `BindingPattern::BindingIdentifier(ref ident) = p.pattern`
- **Files modified:** transform.rs
- **Committed in:** 6103d24

**2. [Rule 1 - Bug] expression_array API takes 2 args, not 3**
- **Found during:** Task 2
- **Issue:** Plan used `ctx.ast.expression_array(SPAN, elements, false)` but OXC v0.113.0's expression_array only takes 2 arguments (SPAN, elements)
- **Fix:** Removed the `false` trailing argument
- **Files modified:** jsx_transform.rs
- **Committed in:** c91152a

**3. [Rule 1 - Bug] Placeholder param naming mismatch**
- **Found during:** Task 2
- **Issue:** Generated `["_", "_1", "item"]` but SWC uses `["_", "_", "item"]` -- SWC uses "_" for both placeholder positions
- **Fix:** Changed to use `"_"` for all placeholder params regardless of position
- **Files modified:** transform.rs
- **Committed in:** c91152a

**4. [Rule 1 - Bug] var_props key formatting for q:p**
- **Found during:** Task 2
- **Issue:** `q:p` rendered as identifier key (`q:p: item`) instead of string literal key (`"q:p": item`) because var_props key building lacked the colon/dash/dollar check
- **Fix:** Applied the same string literal key logic to var_props that const_props already had
- **Files modified:** jsx_transform.rs
- **Committed in:** c91152a

---

**Total deviations:** 4 auto-fixed (4 bugs)
**Impact on plan:** All fixes necessary for correct compilation and SWC snapshot parity. No scope creep.

## Issues Encountered

- Pre-existing test failure in `destructure_args_colon_props` from concurrent 04-01 work -- not a regression from this plan, confirmed unrelated.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

Phase 4 is now complete (3/3 plans done):
- 04-01: Signal wrapping (_wrapProp, _fnSignal)
- 04-02: Loop tracking + q:p injection + event handler params (this plan)
- 04-03: Props destructuring completeness

Remaining work:
- Phase 5: JSX keys/flags, _jsxSorted imports (181x), Fragment handling (67x)
- Phase 6: Import ordering (~20x), use*() inlining (2x deferred), QRL hoisting (deferred from this plan), should_extract_single_qrl_2 naming fix

---
*Phase: 04-signal-props-transforms*
*Completed: 2026-02-20*

## Self-Check: PASSED
