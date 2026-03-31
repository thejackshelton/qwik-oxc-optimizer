---
phase: 12-signal-wrapping-gaps
plan: 02
subsystem: jsx-transform
tags: [signal-wrapping, fnSignal, reactive-deps, member-expression, logical-expression]

# Dependency graph
requires:
  - phase: 12-01
    provides: "Core dep collection fixes (sorting, expression recursion, globals, props call gate)"
provides:
  - "is_any_dep_used_as_object gating for _fnSignal wrapping"
  - "is_dep_or_contains_dep helper for logical/parenthesized expression support"
  - "collect_all_idents_as_primary_deps for complex .value detection"
  - "Fixed (x || y).value reactive dep detection"
affects: ["12-03"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "is_used_as_object gate pattern: check dep usage before _fnSignal wrapping"
    - "collect_all_idents_as_primary_deps for non-simple-chain .value expressions"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "is_any_dep_used_as_object gate added to both props and children _fnSignal wrapping paths"
  - "is_dep_or_contains_dep checks through LogicalExpression and ParenthesizedExpression"
  - "When .value accessed on non-identifier-chain expression (e.g., (a||b).value), collect_all_idents_as_primary_deps extracts all branch idents as primary reactive deps"
  - "Children path: when deps exist but no dep used as object, mark mutable without wrapping"

patterns-established:
  - "is_used_as_object gate: before calling build_fn_signal_wrapping, verify at least one dep is used as the object of a member expression"

# Metrics
duration: 5min
completed: 2026-02-23
---

# Phase 12 Plan 02: is_used_as_object Gate and Complex .value Detection Summary

**Added is_any_dep_used_as_object gate before _fnSignal wrapping and fixed .value detection for (x||y).value logical expressions**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-23T12:26:03Z
- **Completed:** 2026-02-23T12:32:01Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Added `is_any_dep_used_as_object` function that checks if any reactive dep is used as the OBJECT of a member expression (not just as a standalone identifier or array index key)
- Added `is_dep_or_contains_dep` helper that sees through LogicalExpression and ParenthesizedExpression wrappers
- Fixed `.value` detection for complex expressions like `(count || count2).value` via `collect_all_idents_as_primary_deps`
- Gated _fnSignal wrapping in both props and children paths -- prevents wrapping when deps are only used as array indices
- Fixed exact match for `should_wrap_logical_expression_in_template` test (was a diff, now passes)

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement is_used_as_object check and fix .value detection** - `05f555a` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added is_any_dep_used_as_object, is_dep_or_contains_dep, collect_all_idents_as_primary_deps; gated _fnSignal wrapping in props and children paths; fixed .value detection for complex base expressions

## Decisions Made
- `is_any_dep_used_as_object` recursively checks all expression types including StaticMemberExpression, ComputedMemberExpression, BinaryExpression, ConditionalExpression, ObjectExpression, ArrayExpression, TemplateLiteral, CallExpression, ChainExpression
- `is_dep_or_contains_dep` specifically handles Identifier, ParenthesizedExpression, and LogicalExpression to match SWC's treatment of `(a || b).value` where both `a` and `b` are considered "used as object"
- In the children path, when deps exist but `is_any_dep_used_as_object` returns false, we still mark `any_child_mutable = true` (deps presence means the expression is dynamic)
- `collect_all_idents_as_primary_deps` only descends into LogicalExpression and ParenthesizedExpression (not arbitrary sub-expressions) to match SWC's IdentCollector behavior

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

- The `should_transform_multiple_event_handlers` test still shows a diff where OXC wraps `item.value.id` with `_fnSignal` but SWC doesn't. This is a pre-existing issue unrelated to the `is_any_dep_used_as_object` gate -- `item.value.id` has chain depth >= 2, making `item` a primary dep, and `item` IS used as the object of `.value`. The SWC behavior for this case likely involves scope analysis of `.map()` callback context that OXC doesn't replicate. Not a regression.
- The `example_component_with_event_listeners_inside_loop` test still shows diffs for `results[i]` not being wrapped. This is a dep collection issue (both `results` and `i` are loop variables classified as local_deps with no primary_deps) outside the scope of this plan.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Signal wrapping gate in place, ready for Plan 03 (remaining signal wrapping gaps)
- Pre-existing diffs for `item.value.id` wrapping and loop variable dep collection remain as known gaps

## Self-Check: PASSED

---
*Phase: 12-signal-wrapping-gaps*
*Completed: 2026-02-23*
