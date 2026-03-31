---
phase: 14-final-parity
plan: 04
subsystem: optimizer
tags: [oxc, qwik, ctx-kind, diagnostics, source-location]

# Dependency graph
requires:
  - phase: 14-02
    provides: "spread props fixes and const_props algorithm"
  - phase: 14-03
    provides: "entry field, file extension, dev mode fixes"
provides:
  - "JSXProp CtxKind variant for component element JSX prop segment classification"
  - "C03 diagnostic highlight spans with correct source locations"
  - "C05 diagnostic emission for exported $-suffixed functions missing Qrl counterpart"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Element-type-based ctx_kind classification: native -> EventHandler, component -> JSXProp"
    - "compute_highlight_from_span helper for diagnostic source location computation"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/types.rs"
    - "crates/qwik-optimizer-oxc/src/transform.rs"

key-decisions:
  - "JSXProp classification uses element type (native vs component) not attribute name pattern -- matches SWC transpile_jsx:true code path"
  - "C02 diagnostics keep highlights:null (matches SWC golden); only C03 gets highlight spans"
  - "C05 emitted in enter_call_expression when is_dollar_call returns None for exported $-suffixed function calls"
  - "Highlight span formula: lo/hi = OXC offset + 1, startCol = 0-based col + 1, endCol = 0-based col at exclusive end (no +1)"

patterns-established:
  - "byte_offset_to_line_col and compute_highlight_from_span reusable for future diagnostic highlight needs"

# Metrics
duration: 55min
completed: 2026-02-24
---

# Phase 14 Plan 04: CtxKind/Diagnostic Parity Summary

**JSXProp ctx_kind classification for component element props, C03 diagnostic highlight spans, and C05 emission for missing Qrl counterparts -- 86 exact matches (+1)**

## Performance

- **Duration:** ~55 min (across 2 sessions due to context exhaustion)
- **Started:** 2026-02-24T16:58:00Z
- **Completed:** 2026-02-24T17:53:37Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Added JSXProp variant to CtxKind enum with correct `"jSXProp"` serde serialization
- Component element JSX props now classified as JSXProp (not EventHandler), matching SWC's transpile_jsx:true code path
- C03 diagnostics now include highlight spans computed from first argument span
- C05 diagnostic emitted for exported $-suffixed functions missing Qrl counterpart export
- example_missing_custom_inlined_functions is now an exact match (86 total, up from 85)
- example_invalid_segment_expr1 C03 highlights match golden values exactly (lo/hi/line/col)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add JSXProp variant to CtxKind and update classification** - `7b2e299` (feat)
2. **Task 2: Add diagnostic highlight spans and C05 emission** - `bf9bace` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/types.rs` - Added JSXProp variant to CtxKind enum with `#[serde(rename = "jSXProp")]`
- `crates/qwik-optimizer-oxc/src/transform.rs` - ctx_kind classification in record_jsx_event_segment, C03 highlight span computation, C05 emission logic, helper functions

## Decisions Made

1. **Element-type approach for JSXProp classification**: The plan suggested using `is_event_handler_name(attr_name)` to distinguish event handlers from JSX props. Investigation of SWC code revealed two code paths: `handle_jsx_value` (transpile_jsx=false, uses event name check) and `handle_jsx_props_obj` (transpile_jsx=true, uses element type). Since OXC golden snapshots match SWC's transpile_jsx:true output, used element type: native elements -> EventHandler, component elements -> JSXProp.

2. **C02 diagnostics keep highlights:null**: The plan mentioned updating both C02 and C03. However, the golden snapshots show C02 diagnostics have `"highlights": null` while only C03 has actual highlights. Left C02 unchanged to match golden.

3. **C05 emission location**: Placed in `enter_call_expression` inside the `let Some(kind) = kind else { ... }` block which fires for non-core $-suffixed calls. Checks if callee is exported AND missing Qrl counterpart export.

4. **Highlight span byte offset formula**: Derived from SWC's SourceLocation::from in utils.rs. OXC spans are 0-based; SWC BytePos is 1-based. Formula: lo/hi = OXC offset + 1, startCol = 0-based column + 1, endCol = 0-based column at exclusive end (no +1 because SWC's exclusive hi cancels with 1-based adjustment).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] words.rs not modified -- element-type approach used instead**
- **Found during:** Task 1 (JSXProp classification)
- **Issue:** Plan assumed attribute name pattern would distinguish event handlers from JSX props. SWC investigation revealed element type is the correct discriminator.
- **Fix:** Added `is_native_element: bool` parameter to `record_jsx_event_segment` instead of adding `is_event_handler_attr` to words.rs
- **Files modified:** transform.rs only (types.rs as planned)
- **Verification:** All golden snapshots match: component element props get jSXProp, native element props get eventHandler
- **Committed in:** 7b2e299

---

**Total deviations:** 1 auto-fixed (1 bug/correctness)
**Impact on plan:** Simpler implementation using element type instead of attribute name pattern. Correct behavior verified against all golden snapshots.

## Issues Encountered
- SWC has two JSX processing paths (transpile_jsx true/false) with different ctx_kind classification logic. OXC golden snapshots were created from transpile_jsx:true output, so the element-type approach was needed.
- Context exhaustion in first session required continuation in second session.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 14 (Final Parity) is now COMPLETE (all 4 plans executed)
- 86 exact matches out of 162 tests (53%)
- Remaining 76 diffs are in categories: SIGNAL_WRAP (~11), JSX_FLAGS (~17), DCE (~19), CAPTURES (~10), HOIST (~14), JSX_IMPORT (~7), LINE_WRAP/SHORTHAND (~29 aesthetic)
- Ready for Phase 15 (Signal Wrapping & JSX Flags) if desired

## Self-Check: PASSED

---
*Phase: 14-final-parity*
*Completed: 2026-02-24*
