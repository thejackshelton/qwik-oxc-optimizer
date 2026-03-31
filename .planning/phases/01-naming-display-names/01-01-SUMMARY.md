---
phase: 01-naming-display-names
plan: 01
subsystem: transform
tags: [naming, display-names, stack_ctxt, escape_sym, jsx-events, parent-field, dedup]

# Dependency graph
requires: []
provides:
  - "stack_ctxt scope accumulation architecture in QwikTransformer"
  - "escape_sym() for normalizing display names"
  - "jsx_event_to_html_attribute() for native element event naming"
  - "register_context_name() implementing SWC naming algorithm"
  - "segment_stack for correct parent field format"
  - "segment_names dedup counter for repeated display names"
affects:
  - "01-naming-display-names (remaining plans may refine edge cases)"
  - "02-bugs (naming changes affect snapshot baseline)"

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "stack_ctxt push/pop in traverse callbacks for scope accumulation"
    - "Depth marker stacks (var_decl_ctxt_depths, etc.) for safe push/pop tracking"
    - "register_context_name() centralizes all naming logic (join, escape, dedup, hash)"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"

key-decisions:
  - "Combined Tasks 1 and 2 into a single commit since JSX event handler naming is integral to the stack_ctxt architecture"
  - "Used OXC traverse callbacks (enter_jsx_element/exit_jsx_element, enter_jsx_attribute/exit_jsx_attribute) for ALL JSX scope tracking, plus custom recursive walk for $-suffixed event handler segment creation"
  - "Handled JSXElementName::IdentifierReference for component JSX elements (OXC-specific: capital letter JSX elements are IdentifierReference, not Identifier)"
  - "Parent field now stores segment_name (display_name + hash) matching SWC's segment_stack approach"

patterns-established:
  - "stack_ctxt: Single Vec<String> accumulates all scope names, joined with _ to form display names"
  - "Depth markers: Each callback saves stack_ctxt.len() on enter and truncates on exit for safe nesting"
  - "register_context_name(): Centralized naming function mirrors SWC's algorithm exactly"

# Metrics
duration: 15min
completed: 2026-02-19
---

# Phase 1 Plan 1: Stack_ctxt Naming Architecture Summary

**Port of SWC's stack_ctxt scope-accumulation naming to OXC, fixing event handler names (N1), display name context gaps (N2), and parent field format (N3) simultaneously**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-02-19T22:02:08Z
- **Completed:** 2026-02-19T22:17:32Z
- **Tasks:** 2 (combined into 1 commit due to deep coupling)
- **Files modified:** 1

## Accomplishments
- Display names now include ALL intermediate scope elements (variable names, function names, JSX element tags, attribute names) matching SWC output
- Event handler segments on native elements correctly use q-e:click format for $-suffixed attrs, original names for non-$-suffixed attrs
- Parent field uses segment_name with hash (e.g., renderHeader_XXXXXXXXXXXX) instead of display_name string
- Deduplication counter produces _1, _2 suffixes for repeated display names
- Snapshot diff reduced from 4025+/9415- to 3162+/8552- (naming-related diffs)

## Task Commits

1. **Task 1+2: Add stack_ctxt infrastructure, escape_sym, jsx_event_to_html_attribute** - `7bf29e5` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added stack_ctxt, segment_stack, segment_names fields; escape_sym(), jsx_event_to_html_attribute(), register_context_name() functions; traverse callbacks for variable declarators, functions, export defaults, call expressions, JSX elements, JSX attributes; rewrote record_segment and record_jsx_event_segment to use stack_ctxt; updated finalize_segments for new parent format

## Decisions Made
- Combined Tasks 1 and 2 into one commit because JSX event handler naming (Task 2) is fundamentally part of the stack_ctxt push/pop logic (Task 1)
- Used OXC's IdentifierReference variant for component JSX elements (discovery: OXC uses Identifier for native HTML elements, IdentifierReference for component elements)
- Kept dollar_call_stack alongside segment_stack for backward compatibility with finalize_segments and pending_segment_qrl_imports matching

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Handle IdentifierReference for component JSX elements**
- **Found during:** Task 1 (JSX element scope tracking)
- **Issue:** OXC represents component JSX elements (`<Cmp>`) as JSXElementName::IdentifierReference, not JSXElementName::Identifier. The plan only mentioned Identifier, causing component element names to be missing from display names.
- **Fix:** Added IdentifierReference handling in enter_jsx_element, exit_jsx_element, and create_jsx_event_segments_recursive
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** transform_qrl_in_regular_prop now produces Cmp_component_Cmp_foo matching SWC
- **Committed in:** 7bf29e5

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Essential fix for correct component JSX element naming. No scope creep.

## Issues Encountered
- OXC traverse callback ordering: exit_expression fires BEFORE exit_jsx_element for a JSX element expression, meaning the custom recursive walk in create_jsx_event_segments_recursive correctly handles $-suffixed event handlers (which run in exit_expression) while traverse callbacks handle bare $() calls in non-$-suffixed attributes
- The transform_attr_name_for_display function in jsx_transform.rs is now unused (dead code warning) -- can be cleaned up in a future plan

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Stack_ctxt naming architecture is in place and producing correct names for most cases
- Remaining plans in Phase 1 may address edge cases (default export naming, class declarations, nested function names)
- The 3162+/8552- remaining snapshot diffs are primarily whitespace/formatting, import ordering, and body code differences -- not naming issues

## Self-Check: PASSED

---
*Phase: 01-naming-display-names*
*Completed: 2026-02-19*
