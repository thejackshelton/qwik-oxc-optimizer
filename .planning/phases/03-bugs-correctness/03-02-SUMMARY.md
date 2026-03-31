---
phase: 03-bugs-correctness
plan: 02
subsystem: ast-transform
tags: [oxc, typescript, transformer, type-stripping, pipeline]

# Dependency graph
requires:
  - phase: 01-naming
    provides: "segment naming and hash computation"
  - phase: 02-metadata
    provides: "paramNames extraction and path normalization"
provides:
  - "TypeScript type stripping pipeline step before Qwik transform pass"
  - "oxc transformer feature enabled in Cargo.toml"
  - "Correct TS-free AST for collector and traverse phases"
affects: [03-bugs-correctness, 04-features, 05-codegen]

# Tech tracking
tech-stack:
  added: ["oxc transformer feature (oxc_transformer 0.113)"]
  patterns: ["conditional TS stripping before Qwik transform pass", "SemanticBuilder scoping rebuild after transformer"]

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/Cargo.toml"
    - "crates/qwik-optimizer-oxc/src/lib.rs"

key-decisions:
  - "Use JsxOptions::disable() to prevent JSX transform during TS stripping -- default enables jsx_plugin"
  - "Rebuild SemanticBuilder scoping after transformer consumes it, using with_excess_capacity(2.0)"

patterns-established:
  - "Conditional transformer step: guard with transpile_ts && source_type.is_typescript()"
  - "Scoping rebuild after transformer: SemanticBuilder::new().with_excess_capacity(2.0).build(&program)"

# Metrics
duration: 5min
completed: 2026-02-20
---

# Phase 3 Plan 02: TypeScript Type Stripping Summary

**Enabled oxc_transformer for TS type stripping before Qwik transform pass, fixing BUG-01 (~20+ snapshot improvements)**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-20T11:23:38Z
- **Completed:** 2026-02-20T11:28:33Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Enabled `transformer` feature in oxc dependency (Cargo.toml)
- Added conditional TypeScript stripping pipeline step in lib.rs, running before collector::collect
- Correctly disabled JSX transform (JsxOptions::disable()) to prevent unwanted React JSX conversion
- Rebuilt semantic scoping after transformer to ensure correct capture analysis
- Verified 160 snapshot changes with zero panics, TypeScript annotations correctly stripped from output

## Task Commits

Each task was committed atomically:

1. **Task 1: Enable transformer feature in Cargo.toml** - `4a73784` (chore)
2. **Task 2: Add TypeScript stripping pipeline step in lib.rs** - `f8cc5db` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/Cargo.toml` - Added "transformer" feature to oxc dependency, updated NOTE comment
- `crates/qwik-optimizer-oxc/src/lib.rs` - Added conditional TS stripping step after parse, before collect

## Decisions Made

1. **JsxOptions::disable() required** -- OXC's `JsxOptions::default()` calls `enable()` which sets `jsx_plugin: true`. Without explicitly disabling it, the transformer would convert JSX to `React.createElement` calls before the Qwik pass, producing completely wrong output (React imports instead of Qwik imports). Discovered during first test run.

2. **Scoping rebuild pattern** -- Used `SemanticBuilder::new().with_excess_capacity(2.0).build(&program)` matching the existing pattern in parse.rs. The transformer consumes the original Scoping, so rebuilding is mandatory.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] JSX transform enabled by default in OXC TransformOptions**
- **Found during:** Task 2 (TS stripping implementation)
- **Issue:** Using `..Default::default()` for TransformOptions enabled the JSX transform plugin, causing JSX to be converted to React.createElement calls (showed React imports in output)
- **Fix:** Added `jsx: oxc::transformer::JsxOptions::disable()` to explicitly disable JSX transformation
- **Files modified:** crates/qwik-optimizer-oxc/src/lib.rs
- **Verification:** Tests run without React imports in output; JSX preserved correctly for Qwik transform
- **Committed in:** f8cc5db (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Essential fix -- without it, all JSX would be converted to React format before Qwik could process it.

## Issues Encountered
None beyond the JSX default issue documented above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- BUG-01 (TS stripping) is now fixed, unblocking accurate snapshot comparison for remaining Phase 3 bugs
- 160 snapshots show changes -- many will be cascading improvements from TS stripping
- BUG-02 (component options) was already fixed in 03-01
- Ready to proceed with 03-03 (remaining bugs: captures, missing segments, comments, test fixtures)
- Some snapshot diffs are Phase 4 symptoms (missing _wrapProp, _fnSignal, props destructuring)

## Self-Check: PASSED

---
*Phase: 03-bugs-correctness*
*Completed: 2026-02-20*
