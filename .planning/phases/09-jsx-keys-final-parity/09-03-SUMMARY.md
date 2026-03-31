---
phase: 09-jsx-keys-final-parity
plan: 03
subsystem: transform
tags: [qrl, dev-mode, captures, diagnostics, jsx, oxc]

# Dependency graph
requires:
  - phase: 09-01
    provides: "Pure var DCE, entry field, JSX event rename"
  - phase: 09-02
    provides: "_wrapProp, className, capture format, local Qrl self-imports"
provides:
  - "Dev mode QRL emission (qrlDEV/inlinedQrlDEV/_noopQrlDEV) with source metadata"
  - "JSX dev location metadata ({ fileName, lineNumber, columnNumber })"
  - "C02 diagnostics for function/class captures in $() scope"
  - "Function/class declarations excluded from capture lists"
affects: ["09-04", "09-05"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Dev mode metadata injection via QrlDevMetadata/JsxDevLocation structs"
    - "SWC 1-based byte offset conversion (lo+1, hi+1)"
    - "invalid_decl_stack for function/class declaration tracking"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/import_rewrite.rs"
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
    - "crates/qwik-optimizer-oxc/src/code_move.rs"
    - "crates/qwik-optimizer-oxc/src/types.rs"

key-decisions:
  - "body_span uses first argument span (arrow fn), not call expression span -- matches SWC's first_arg.span()"
  - "SWC BytePos is 1-based; OXC spans are 0-based -- add 1 to lo/hi for dev metadata parity"
  - "Function/class declarations tracked in invalid_decl_stack, excluded from captures, emit C02 diagnostics"
  - "Diagnostic field order matches SWC: category, code, file, message, highlights, suggestions, scope"
  - "Dev mode file path uses dev_abs_path() (src_dir + filename) -- test config differences accepted as non-code-bug"

patterns-established:
  - "QrlDevMetadata struct for dev mode source location metadata"
  - "JsxDevLocation struct for JSX element source location"
  - "invalid_decl_stack parallel to capture_stack for fn/class declaration tracking"

# Metrics
duration: 35min
completed: 2026-02-21
---

# Phase 9 Plan 3: Dev Mode QRL Emission & Capture Diagnostics Summary

**Dev mode QRL emission (qrlDEV/inlinedQrlDEV/_noopQrlDEV) with source metadata, JSX dev location, and C02 diagnostics for function/class captures -- 1 new exact golden match**

## Performance

- **Duration:** ~35 min (across two sessions due to context continuation)
- **Started:** 2026-02-21T15:53:37Z
- **Completed:** 2026-02-21T17:10:22Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- Dev mode emits qrlDEV/inlinedQrlDEV/_noopQrlDEV with { file, lo, hi, displayName } metadata
- JSX elements get extra { fileName, lineNumber, columnNumber } argument in dev mode
- Function/class references correctly excluded from captures with C02 diagnostics
- 1 new exact golden match: example_jsx_keyed_dev
- 128 files differ (was 129), ~1655 insertion / ~2080 deletion lines (was ~1676 / ~2113)

## Task Commits

Each task was committed atomically:

1. **Task 1: Dev mode QRL emission with metadata** - `571b00d` (feat)
2. **Task 2: C02 diagnostics for function/class captures** - `1e8740a` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/import_rewrite.rs` - QrlDevMetadata, JsxDevLocation structs and builders
- `crates/qwik-optimizer-oxc/src/transform.rs` - Dev mode metadata generation, invalid_decl_stack, C02 diagnostics
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - JSX dev location computation and argument injection
- `crates/qwik-optimizer-oxc/src/code_move.rs` - DEV variant import detection in segment modules
- `crates/qwik-optimizer-oxc/src/types.rs` - Diagnostic field order and null serialization

## Decisions Made
- **body_span source**: Uses first argument span (the arrow function expression) instead of call expression span, matching SWC's `first_arg.span()`
- **Byte offset conversion**: SWC uses 1-based BytePos while OXC uses 0-based spans; add 1 to lo/hi for parity
- **Dev file path**: `dev_abs_path()` constructs `src_dir + "/" + filename`; test config differences (OXC default `src_dir="."` vs SWC `"/user/qwik/src/"`) cause expected path differences in dev mode test snapshots
- **Function/class capture exclusion**: `invalid_decl_stack` tracks fn/class declaration names per $()-body scope; these are excluded from captures and emit C02 diagnostics instead (matching SWC's `invalid_decl` partition)
- **Diagnostic serialization**: Reordered struct fields and removed `skip_serializing_if` for highlights/suggestions to match SWC's JSON output format

## Deviations from Plan

### Plan Description Corrections

**1. Dead code behavior was incorrectly described in plan**
- Plan stated "preserve dead code inside segment bodies" but SWC actually STRIPS dead code (if(false) blocks, unused declarations)
- OXC currently preserves dead code, which is a divergence from SWC
- Implementing DCE (dead code elimination) deferred -- only affects 3 test files

**2. Capture analysis scope was broader than implementable**
- Plan described fixing 27 files of capture differences
- Most capture differences are pre-existing issues tied to _rawProps transforms, iteration variable handling, and prop classification
- Focused on the implementable fix: function/class declaration exclusion from captures with C02 diagnostics (1 test file directly improved)

## Issues Encountered
- **Context continuation**: Session ran out of context partway through Task 1. Analysis and implementation decisions were preserved in the continuation summary, allowing seamless resumption.
- **SWC vs OXC test config differences**: Different default `src_dir` values between SWC tests (`"/user/qwik/src/"`) and OXC tests (`"."`) cause expected file path differences in dev mode metadata. This is a test configuration issue, not a code bug.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Dev mode QRL emission complete -- all DEV variant function names and metadata objects working
- JSX dev location metadata complete for _jsxSorted and _jsxSplit calls
- C02 diagnostics for invalid captures matching SWC
- Remaining 128 snapshot diffs are primarily:
  - var_props/const_props ordering (~82 positions across many files)
  - Pre-existing capture differences tied to _rawProps/iteration variable transforms
  - DCE differences (3 files)
  - Test config path differences (5 dev mode files)
  - Import assertion stripping (1 file)
  - Comment preservation (1 file)
- Plans 04 and 05 can address remaining prop classification and final cleanup items

---
*Phase: 09-jsx-keys-final-parity*
*Completed: 2026-02-21*

## Self-Check: PASSED
