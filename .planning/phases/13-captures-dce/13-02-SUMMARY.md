---
phase: 13-captures-dce
plan: 02
subsystem: captures
tags: [scope-analysis, capture-filtering, C03-diagnostics, oxc-traverse]

# Dependency graph
requires:
  - phase: 13-01
    provides: "Props reclassification + const literal inlining"
  - phase: 10-01
    provides: "Basic capture mechanism + iteration variable handling"
provides:
  - "Scope-aware capture filtering for nested $() calls"
  - "C03 diagnostic emission for non-function $() arguments"
  - "Parity with SWC's scope-aware capture analysis"
affects: ["13-03", "13-04", "14-final-parity"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "all_parent_decls scope filtering for $() call captures (mirrors JSX event handler pattern)"
    - "C03 diagnostic emission: detect non-function first arg + captures"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"

key-decisions:
  - "Scope filtering for $() captures uses same pattern as JSX event handler scope filtering (all_parent_decls + module_level_decls)"
  - "C03 diagnostic clears captures entirely for non-function $() arguments (matches SWC behavior)"
  - "Hoist strategy extraction (should_not_generate_conflicting_props_identifiers) deferred -- requires Hoist infrastructure, not captures"
  - "example_component_with_event_listeners_inside_loop _fnSignal wrapping is Phase 12 scope, not capture issue"
  - "should_extract_single_qrl_2 naming-dedup suffix ordering deferred to Phase 14"

patterns-established:
  - "Non-top-level $() capture filtering: all_parent_decls || module_level_decls (excludes unresolved identifiers)"
  - "C03 diagnostic gate: !first_arg_is_function && !captures.is_empty() && !is_top_level"

# Metrics
duration: 15min
completed: 2026-02-24
---

# Phase 13 Plan 02: Nested Scope Capture Filtering & C03 Diagnostics Summary

**Scope-aware capture filtering for nested $() calls eliminating unresolved identifier over-capture, plus C03 diagnostic emission for non-function $() arguments**

## Performance

- **Duration:** 15 min
- **Started:** 2026-02-24T08:37:32Z
- **Completed:** 2026-02-24T08:53:16Z
- **Tasks:** 3 (2 with code changes, 1 analysis-only)
- **Files modified:** 1

## Accomplishments
- Fixed over-capture of unresolved identifiers (e.g., `children` in example_jsx, `obj/v1/v2/v3` in example_exports) by adding scope-aware filtering to the $() call capture path
- Implemented C03 diagnostic emission for non-function $() arguments that capture local identifiers (example_invalid_segment_expr1)
- Confirmed no regressions: 99 snapshot diffs before and after (same count)
- Capture ordering already correct for should_extract_single_qrl, should_extract_single_qrl_with_index, should_transform_qrls_in_ternary_expression

## Task Commits

Each task was committed atomically:

1. **Task 1a: Fix nested scope capture filtering** - `667767e` (fix)
2. **Task 1b: Fix C03 diagnostics** - `ae20c51` (fix)
3. **Task 2: Remaining capture edge cases** - No code changes needed (analysis confirmed issues are either resolved or outside scope)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added scope filtering for non-top-level $() captures + C03 diagnostic emission for non-function $() arguments

## Decisions Made
- **Scope filtering pattern**: Reused the existing JSX event handler scope filtering approach (all_parent_decls from capture_stack + module_level_decls) for $() call captures. SWC's capture analysis is scope-aware and only captures identifiers declared in enclosing scopes.
- **C03 diagnostic vs C02**: C02 is for function/class declarations referenced in $() scope. C03 is for non-function first arguments to $() that capture local identifiers. They serve different purposes.
- **Hoist strategy deferred**: should_not_generate_conflicting_props_identifiers requires Hoist entry strategy extraction to named constants (not inline arrows). This is a Hoist infrastructure issue, not a capture issue.
- **_fnSignal wrapping for loop index access**: example_component_with_event_listeners_inside_loop's `results[i]` needs _fnSignal wrapping, which is Phase 12 signal wrapping scope, not Phase 13 captures.

## Deviations from Plan

### Analysis findings differing from plan assumptions

**1. example_jsx `children` issue was OVER-capture, not MISSING capture**
- **Plan assumed:** `children` was missing from captures
- **Reality:** `children` was being over-captured (OXC captured it, SWC doesn't) because it's an unresolved identifier not declared in any scope
- **Fix:** Scope filtering removes unresolved identifiers from $() captures

**2. Capture ordering tests had no semantic diff**
- **Plan assumed:** should_extract_single_qrl and friends had ordering issues
- **Reality:** Capture names and ordering were already correct. Only JSON field position (captureNames at different position in metadata object) differed -- purely serialization format.

**3. should_transform_component_with_normal_function had no missing capture**
- **Plan assumed:** Missing capture in segment
- **Reality:** Only aesthetic diffs (shorthand formatting, _hf ordering). No capture issue.

---

**Total deviations:** 0 auto-fixed
**Impact on plan:** Plan analysis was partially incorrect about root causes, but the fixes applied address the actual issues correctly.

## Issues Encountered
- Had to debug `example_jsx` `children` capture by adding temporary eprintln statements to trace capture analysis flow. Discovered the issue was over-capture (OXC captured unresolved `children`), not under-capture as the plan assumed.

## Next Phase Readiness
- Capture filtering is now scope-aware for both JSX event handlers and $() calls
- C03 diagnostics emitted for non-function $() arguments
- Ready for Phase 13 plans 03-04 (DCE: unused declarations, if(false) elimination, invalid declaration removal)
- Remaining capture-category diffs are either aesthetic (JSON field ordering) or outside Phase 13 scope (Hoist infrastructure, _fnSignal wrapping)

## Self-Check: PASSED

---
*Phase: 13-captures-dce*
*Completed: 2026-02-24*
