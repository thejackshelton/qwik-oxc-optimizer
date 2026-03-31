---
phase: 02-metadata
plan: 01
subsystem: transform
tags: [paramNames, segment-metadata, binding-pattern, windows-path, OXC-AST]

# Dependency graph
requires:
  - phase: 01-naming
    provides: correct display_name, hash, parent fields in segment metadata
provides:
  - paramNames extraction from arrow/function expression arguments for all $() calls
  - paramNames extraction from JSX event handler attribute expressions
  - Forward-slash normalization in rel_dir for Windows-style paths
affects: [03-missing-segments, 04-iteration-vars]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Free functions for AST node-to-string conversion (binding_pattern_to_string)"
    - "Separate extraction functions for different call contexts (Argument vs JSXExpression)"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/lib.rs
    - crates/qwik-optimizer-oxc/tests/test.rs

key-decisions:
  - "BigIntLiteral.raw is Option<Atom> in OXC 0.113 -- handled with as_ref().map_or_else"
  - "FormalParameterRest wraps BindingRestElement with nested .rest.argument path"
  - "23 remaining paramNames mismatches are from q:p iteration variable injection (transform_event_handler_with_iter_var) -- separate feature for Phase 4"
  - "Test support_windows_paths had double backslashes (\\\\) vs SWC's single backslashes (\\) -- fixed to match"

patterns-established:
  - "binding_pattern_to_string: recursive pattern-to-string matching SWC's pat_to_string"
  - "extract_param_names_from_*: family of extraction functions for different AST contexts"

# Metrics
duration: 15min
completed: 2026-02-20
---

# Phase 2 Plan 01: paramNames Extraction & Path Normalization Summary

**paramNames extraction from BindingPattern AST nodes for all segment types, plus rel_dir backslash normalization -- 247/270 metadata fields now match SWC**

## Performance

- **Duration:** 15 min
- **Started:** 2026-02-20T09:52:41Z
- **Completed:** 2026-02-20T10:08:01Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Added `binding_pattern_to_string`, `extract_param_names_from_params`, `extract_param_names_from_argument`, and `extract_param_names_from_jsx_expr` -- 4 free functions for recursive BindingPattern-to-string conversion matching SWC's `pat_to_string` exactly
- Integrated paramNames extraction into `record_segment` (regular $() calls) and `record_jsx_event_segment` (JSX event handlers)
- Fixed `rel_dir` to normalize backslashes to forward slashes, achieving 100% path field parity (270/270 match)
- Props destructuring override (`["_rawProps"]`) confirmed preserved and working correctly
- 247 of 270 paramNames fields now match SWC; remaining 23 are from `q:p` iteration variable injection (a separate transformation)

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement paramNames extraction functions and integrate into record_segment** - `99ac1d8` (feat)
2. **Task 2: Wire paramNames into JSX event handler segments and fix rel_dir path normalization** - `577c316` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added 4 extraction functions, integrated into record_segment and record_jsx_event_segment
- `crates/qwik-optimizer-oxc/src/lib.rs` - Fixed rel_dir to normalize backslashes to forward slashes
- `crates/qwik-optimizer-oxc/tests/test.rs` - Fixed support_windows_paths test to use single backslashes matching SWC

## Decisions Made
- **OXC BindingPattern match arms**: Used the same pattern as existing `collect_binding_pattern_names` for consistency
- **BigIntLiteral.raw**: OXC 0.113 has `raw` as `Option<Atom<'_>>`, handled with `as_ref().map_or_else(String::new, ...)`
- **FormalParameterRest**: OXC wraps with extra `.rest` nesting: `rest.rest.argument` instead of `rest.argument`
- **q:p iteration variable injection**: 23 remaining paramNames mismatches are from `transform_event_handler_with_iter_var` in SWC which adds iteration variables (like `["_", "_", "row"]`) to event handler params -- this is a separate transformation not in scope for Phase 2

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed support_windows_paths test using double backslashes**
- **Found during:** Task 2 (rel_dir fix verification)
- **Issue:** OXC test used `"components\\\\apps\\\\apps.tsx"` (double backslashes = literal `\\`) while SWC uses `r"components\apps\apps.tsx"` (single backslashes = literal `\`). This produced `components//apps/` instead of `components/apps`.
- **Fix:** Changed to single backslash escapes `"components\\apps\\apps.tsx"` matching SWC's raw string literal
- **Files modified:** crates/qwik-optimizer-oxc/tests/test.rs
- **Verification:** Path field now outputs `"components/apps"` matching SWC golden exactly
- **Committed in:** 577c316 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Bug fix essential for correct test behavior. No scope creep.

## Issues Encountered
- OXC AST field naming differs from plan's assumptions: `FormalParameterRest` has nested `.rest.argument` path, and `BigIntLiteral.raw` is `Option<Atom>`. Both discovered during initial compilation and fixed immediately.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- paramNames extraction is complete for all segment types where OXC has the original function parameters available
- 23 remaining paramNames mismatches require `transform_event_handler_with_iter_var` (q:p injection) -- a separate transformation likely in Phase 4 (iteration/loop handling)
- path field is fully normalized, no remaining path mismatches
- Ready to proceed with remaining Phase 2 metadata plans or Phase 3 (missing segments)

## Self-Check: PASSED

---
*Phase: 02-metadata*
*Completed: 2026-02-20*
