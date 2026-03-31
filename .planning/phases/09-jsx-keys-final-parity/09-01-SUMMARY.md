---
phase: 09-jsx-keys-final-parity
plan: 01
subsystem: transform
tags: [oxc, qwik, dce, pure-annotation, entry-field, windows-paths, jsx-event-naming]

# Dependency graph
requires:
  - phase: 08-jsx-flags-iteration-variables
    provides: "JSX flag computation, q:p injection, event handler classification"
provides:
  - "Unused pure variable DCE (simplify_unused_pure_var_decls)"
  - "Entry field computation from EntryStrategy"
  - "Windows path normalization in hash, origin, and JSX keys"
  - "JSX event attribute renaming (onClick$ -> q-e:click) for non-transpiled JSX"
affects: [09-02, 09-03, 09-04, 09-05]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Post-hoc pure var DCE: check CallExpression.pure flag before dropping unused bindings"
    - "JSX event attribute rename as post-processing pass in exit_expression (after segment extraction)"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/lib.rs"
    - "crates/qwik-optimizer-oxc/src/hash.rs"

key-decisions:
  - "Pure flag check for DCE: only drop unused var decls with CallExpression.pure=true init (not all call inits)"
  - "Export specifier references collected by collect_referenced_idents to prevent incorrect DCE"
  - "EntryStrategy::Hook mapped same as Segment (returns None for entry field)"
  - "JSX event rename in exit_expression AFTER segment extraction (not in exit_jsx_attribute which breaks extraction)"

patterns-established:
  - "Post-processing JSX rename: rename_jsx_event_attrs runs after create_jsx_event_segments_recursive and replace_jsx_event_handler_values"
  - "Path normalization: backslash-to-forward-slash in hash.rs, lib.rs, and transform.rs JSX key prefix"

# Metrics
duration: ~45min
completed: 2026-02-21
---

# Phase 9 Plan 1: Const Assignment, Entry Field, Windows Paths, Event Naming Summary

**Pure-annotated unused var DCE, entry field strategy computation, Windows path normalization, and JSX event attribute renaming (onClick$ -> q-e:click) for non-transpiled output**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-02-21T14:13:00Z
- **Completed:** 2026-02-21T14:58:26Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Implemented unused pure variable DCE that checks CallExpression.pure flag, correctly dropping `const X = /* @__PURE__ */ qrl(...)` while preserving `const Y = componentQrl(qrl(...))` (16 files improved)
- Added entry field computation from EntryStrategy matching SWC behavior
- Normalized Windows backslash paths to forward slashes in hash computation, origin fields, and JSX key prefix (1 file fixed)
- Added JSX event attribute renaming (onClick$ -> q-e:click) as post-processing pass that runs after segment extraction (5 files improved, 3 exact golden matches gained)

## Task Commits

Each task was committed atomically:

1. **Task 1: Unused pure var DCE, entry field computation, windows path normalization** - `40a45a6` (fix)
2. **Task 2: JSX event handler attribute renaming for non-transpiled JSX** - `6811861` (fix)

**Plan metadata:** (pending)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added simplify_unused_pure_var_decls, export specifier reference collection, JSX key path normalization, rename_jsx_event_attrs post-processing pass
- `crates/qwik-optimizer-oxc/src/lib.rs` - Added compute_entry_field, origin/main_path backslash normalization, segment_data_to_analysis strategy parameter
- `crates/qwik-optimizer-oxc/src/hash.rs` - Backslash normalization in segment hash computation

## Decisions Made
- **Pure flag for DCE:** SWC's MinifyMode::Simplify drops unused variable declarations whose init is a pure-annotated call expression. OXC's AST has `CallExpression.pure: bool` which maps directly. Only pure-annotated calls (like `qrl()`) are candidates; non-pure calls (like `componentQrl(...)`) are preserved.
- **Export specifier traversal:** `export { RouterOutlet, Form }` specifier local names must be collected by collect_referenced_idents. Without this, DCE incorrectly drops bindings that are re-exported.
- **EntryStrategy::Hook = Segment:** Both return None for entry field, matching SWC's entry_strategy.rs behavior.
- **Event rename timing:** The critical insight was that `exit_jsx_attribute` fires BEFORE `exit_expression` for the parent JSXElement. Since `create_jsx_event_segments_recursive` runs in `exit_expression` and checks for `$`-suffixed attribute names, renaming in `exit_jsx_attribute` breaks segment extraction. The fix is a separate recursive rename pass in `exit_expression`, after segment extraction and QRL value replacement.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Plan described const assignment direction incorrectly**
- **Found during:** Task 1
- **Issue:** Plan said OXC drops const bindings and SWC preserves them. Investigation revealed the opposite: SWC's MinifyMode::Simplify DROPS unused const bindings with pure call inits (e.g., `const Header = qrl(...)` -> `qrl(...)`). OXC was correctly keeping them but should drop them.
- **Fix:** Implemented simplify_unused_pure_var_decls to match SWC behavior (drop unused pure var decls)
- **Files modified:** transform.rs
- **Verification:** 16 snapshots improved, 0 regressions

**2. [Rule 3 - Blocking] Export specifier references not collected**
- **Found during:** Task 1
- **Issue:** After implementing DCE, `export { RouterOutlet, Form }` caused regression because specifier local names weren't tracked as references
- **Fix:** Added export specifier traversal in collect_idents_from_statement
- **Files modified:** transform.rs
- **Verification:** Regression eliminated, net 16 improvements

**3. [Rule 1 - Bug] exit_jsx_attribute rename broke segment extraction**
- **Found during:** Task 2
- **Issue:** Renaming onClick$ to q-e:click in exit_jsx_attribute (which fires before exit_expression) caused create_jsx_event_segments_recursive to miss the $ suffix, preventing segment extraction for 7 test cases
- **Fix:** Moved rename to a separate post-processing pass (rename_jsx_event_attrs) called in exit_expression AFTER segment extraction and QRL replacement
- **Files modified:** transform.rs
- **Verification:** 5 improvements, 0 regressions, 3 exact golden matches gained

---

**Total deviations:** 3 auto-fixed (2 bugs, 1 blocking)
**Impact on plan:** All fixes were essential for correctness. The plan's const assignment direction was wrong, and the event rename timing required significant debugging to identify the traversal order issue.

## Issues Encountered
- The "under 100 remaining diff files" success criterion was not met (131 remain vs target of <100). The plan overestimated the impact (~50+ files). The actual improvements were more targeted (22 files improved total, 3 exact matches). Remaining diffs are caused by other issues (import ordering, formatting, missing transforms) addressed in subsequent plans 09-02 through 09-05.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Pure var DCE foundation ready for plans 09-02+ to build on
- Event naming now correct for non-transpiled JSX; transpiled JSX already handles this
- 131 snapshot files still differ (~4180 diff lines, down from ~4208)
- 3 new exact golden matches gained (example_default_export_index, example_default_export_invalid_ident, example_transpile_ts_only)

## Self-Check: PASSED

---
*Phase: 09-jsx-keys-final-parity*
*Completed: 2026-02-21*
