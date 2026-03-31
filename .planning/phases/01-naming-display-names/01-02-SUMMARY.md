---
phase: 01-naming-display-names
plan: 02
subsystem: optimizer-transform
tags: [naming, display-names, hash, segments, oxc, swc-parity]

# Dependency graph
requires:
  - phase: 01-01
    provides: "stack_ctxt naming architecture, JSX event handler naming"
provides:
  - "Correct default export naming using file_stem (folder name for index files)"
  - "Hash computation matching SWC (hash without filename prefix, prepend after)"
  - "file_name()/file_stem()/rel_dir() path utilities matching SWC path_data"
  - "Conditional Fragment naming for transpile_jsx mode"
  - "Production mode s_HASH segment naming"
  - "Raw $() calls excluded from stack_ctxt push"
  - "Dead code cleanup: removed unused transform_attr_name_for_display from jsx_transform"
affects: ["02-metadata", "03-missing-segments", "04-captures"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "file_name()/file_stem() methods on QwikTransform for SWC path_data parity"
    - "rel_dir() utility for directory extraction matching SWC's rel_dir"
    - "Conditional naming: Fragment only pushed when transpile_jsx=true"
    - "Mode-dependent segment names: s_HASH for Prod, display_name_HASH otherwise"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/lib.rs"
    - "crates/qwik-optimizer-oxc/src/collector.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "Hash computation: hash on display_name WITHOUT filename prefix, then prepend file_name after (matches SWC line 358-368)"
  - "Fragment naming: conditionally push 'Fragment' only when transpile_jsx=true, since SWC only sees Fragment after JSX transform converts <> to _jsxQ(Fragment, ...)"
  - "Raw $() calls: do NOT push callee name to stack_ctxt (SWC's handle_qsegment returns before push), which enables correct dedup counter naming"
  - "Prod mode naming: use s_HASH format for segment names in EmitMode::Prod (matches SWC line 363-367)"
  - "Collector display name functions kept but documented as internal-only (not used by transform)"

patterns-established:
  - "Path decomposition: file_name() for basename+ext, file_stem() for basename without ext, rel_dir() for directory part"
  - "Naming boundaries: collector derives display names for internal nesting tracking only; transform builds final names from stack_ctxt"

# Metrics
duration: 45min
completed: 2026-02-19
---

# Phase 1 Plan 02: Default Export Naming, Hash, and Edge Cases Summary

**Fixed 8 naming edge cases (default exports, hash inputs, Fragment, prod mode, raw $ calls) achieving 160/162 naming parity with SWC**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-02-19T22:43:48Z
- **Completed:** 2026-02-19T23:28:00Z
- **Tasks:** 2
- **Files modified:** 4 source files + 161 snapshot files

## Accomplishments
- Fixed hash computation: hash display_name WITHOUT filename prefix, then prepend file_name after hashing (matching SWC exactly)
- Added file_name()/file_stem()/rel_dir() path utilities that correctly handle dots in filenames, Windows backslashes, and nested paths
- Fixed default export naming to use file_stem (folder name for index files) instead of literal "default"
- Added conditional Fragment push for transpile_jsx mode (SWC only includes Fragment after JSX transpilation)
- Implemented s_HASH segment naming for production mode
- Fixed raw $() calls to not push callee to stack_ctxt (SWC returns early from handle_qsegment)
- Fixed segment module paths to include directory prefix from source file
- Fixed canonical filename and display name to use basename instead of full path

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix default export naming and hash computation edge cases** - `40ee7ac` (fix)
2. **Task 2: Clean up dead collector code and run comprehensive verification** - `05a7b93` (refactor)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added file_name()/file_stem(), fixed hash computation, Fragment handling, prod mode naming, raw $() exclusion
- `crates/qwik-optimizer-oxc/src/lib.rs` - Added rel_dir(), fixed segment path and canonical filename construction
- `crates/qwik-optimizer-oxc/src/collector.rs` - Added internal-only documentation to display name functions
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Removed dead transform_attr_name_for_display function

## Decisions Made
- **Hash computation order**: SWC hashes with `display_name` (no file prefix), then prepends `file_name` after. OXC was incorrectly including the filename prefix in the hash input.
- **Fragment naming**: Only pushed when `transpile_jsx=true`. In SWC, fragments become `_jsxQ(Fragment, ...)` after JSX transform, and `handle_jsx` pushes "Fragment". When JSX is NOT transpiled, raw `<>` fragments don't push anything.
- **Raw $() exclusion**: SWC's `handle_qsegment` returns early before the callee name push happens. This means aliased `$` calls (like `$ as onRender`) don't add the alias to the display name, instead relying on dedup counters.
- **Prod mode**: SWC uses `s_HASH` format for segment names in Lib/Prod modes (lines 363-367). OXC now matches for Prod mode.
- **Collector cleanup**: Kept display name functions in collector (they're used for internal parent tracking) but documented they're NOT used for final segment naming.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fragment not pushed in create_jsx_event_segments child traversal**
- **Found during:** Task 1 (Fragment naming investigation)
- **Issue:** `create_jsx_event_segments_in_children` and `exit_expression` for JSXFragment did not push "Fragment" to stack_ctxt before recursing into fragment children
- **Fix:** Added conditional Fragment push/pop (when transpile_jsx=true) in both `exit_expression` JSXFragment handler and `create_jsx_event_segments_in_children`
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** immutable_analysis test now shows correct `App_component_Fragment_Div_onEvent` names
- **Committed in:** 40ee7ac

**2. [Rule 1 - Bug] Raw $() calls pushed callee name to stack_ctxt**
- **Found during:** Task 1 (renamed exports investigation)
- **Issue:** OXC pushed the local identifier name for ALL call expressions, but SWC's `handle_qsegment` returns early before the push for raw `$` calls. This caused `$ as onRender` to produce `App_Component_onRender` instead of `App_Component_1`.
- **Fix:** Added check to skip stack_ctxt push when the call is a `DollarCallKind::RawDollar`
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** renamed_exports test now matches SWC exactly
- **Committed in:** 40ee7ac

**3. [Rule 1 - Bug] Missing production mode s_HASH naming**
- **Found during:** Task 1 (prod_node snapshot investigation)
- **Issue:** SWC uses `s_HASH` format for segment names in Prod mode, but OXC always used full display name format
- **Fix:** Added mode check in `register_context_name` to use `s_HASH` format for `EmitMode::Prod`
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** prod_node, build_server, strip_server_code, input_bind tests all match SWC
- **Committed in:** 40ee7ac

---

**Total deviations:** 3 auto-fixed (3 bugs)
**Impact on plan:** All auto-fixes necessary for naming correctness. No scope creep.

## Issues Encountered
- File stem extraction with `rsplit('.')` broke for filenames with dots (e.g., `[[...slug]].tsx` split into `["tsx", "slug]]", "[", "[["]`). Fixed by using `rfind('.')` to strip only the LAST extension.

## Comprehensive Verification Results

**Naming field comparison (displayName, name, parent, canonicalFilename, ctxName, ctxKind):**
- 157/162 tests: perfect naming match with SWC
- 3 tests: segment ordering-only diffs (same names, different order due to traversal differences)
- 2 tests: missing segments from multi-file inputs (not naming issue)

**Full metadata comparison (including captures):**
- 108/162 tests: perfect metadata match
- 49 tests: captures-only diffs (Phase 4)
- 5 tests: naming-adjacent diffs (3 ordering, 2 multi-file)

**Remaining non-naming diffs for later phases:**
- Phase 2 (metadata): paramNames field, import ordering
- Phase 3 (missing segments): multi-file input handling (2 tests)
- Phase 4 (captures): captures/captureNames fields (49 tests)
- Phase 5 (JSX): code formatting, JSX output differences
- Phase 6 (imports): import ordering and deduplication

## Next Phase Readiness
- Naming architecture complete: stack_ctxt + register_context_name fully matches SWC
- All path utilities (file_name, file_stem, rel_dir) ready for use by other phases
- Segment ordering differences (3 tests) may need attention in Phase 3 or later
- Multi-file input handling needs work in Phase 3

## Self-Check: PASSED

---
*Phase: 01-naming-display-names*
*Completed: 2026-02-19*
