---
phase: 03-bugs-correctness
plan: 01
subsystem: transform
tags: [oxc, codegen, component-options, comments, ast-transform]

# Dependency graph
requires:
  - phase: 01-naming
    provides: "Correct segment naming and display name derivation"
provides:
  - "componentQrl() extra argument passthrough for all named $-suffixed calls"
  - "Comment-preserving segment body codegen via temporary Program + build()"
affects: [03-bugs-correctness remaining plans, 04-features]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "codegen_expression_with_comments: temporary Program construction for comment-aware codegen"
    - "enter_program to capture source comments for later use during traverse"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"

key-decisions:
  - "BUG-02: Extra args passthrough applies to ALL named $-suffixed calls generically, not just component$"
  - "BUG-06: Used temporary Program + build() instead of print_expression for comment preservation, since print_expression doesn't call build_comments()"

patterns-established:
  - "codegen_expression_with_comments helper for any future need to serialize expressions with comments"

# Metrics
duration: 11min
completed: 2026-02-20
---

# Phase 3 Plan 01: BUG-02 + BUG-06 Summary

**Component options passthrough and source comment preservation in segment body codegen**

## Performance

- **Duration:** 11 min
- **Started:** 2026-02-20T11:22:59Z
- **Completed:** 2026-02-20T11:33:51Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- componentQrl() calls now include extra arguments (e.g., `{ tagName: "my-foo" }`) for all named $-suffixed calls
- Source comments like `// Double count watch` are preserved in extracted segment module code
- No regressions: all existing tests pass without panics

## Task Commits

Each task was committed atomically:

1. **Task 1: Preserve extra arguments in named $-suffixed calls (BUG-02)** - `c6f05a4` (fix)
2. **Task 2: Add source text to segment body codegen for comment preservation (BUG-06)** - `23a070b` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Fixed extra argument passthrough in DollarCallKind::Named match arm; added comment-preserving codegen via temporary Program + build()

## Decisions Made
- **BUG-02 fix is generic:** The extra argument passthrough loop applies to ALL named $-suffixed calls (component$, useTask$, etc.), not just component$. This matches SWC's behavior where `fold_call_expr` processes all arguments at index > 0 for all marker functions.
- **BUG-06: temporary Program approach:** OXC's `Codegen::print_expression()` does not call `build_comments()` (which is `pub(crate)` in oxc_codegen), so `with_source_text()` alone is insufficient for comment preservation. Instead, we construct a temporary `Program` containing the expression as an `ExpressionStatement` with filtered comments, and use `Codegen::build()` which properly initializes the comment system.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Comment codegen approach changed from with_source_text to temporary Program**
- **Found during:** Task 2 (comment preservation)
- **Issue:** Plan specified using `Codegen::new().with_source_text(&self.source_code)` with `print_expression()`, but investigation revealed `print_expression()` never calls `build_comments()`, so comments are not emitted regardless of source text being set
- **Fix:** Created `codegen_expression_with_comments()` helper that builds a temporary Program with filtered comments and uses `build()` instead of `print_expression()`
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** Comments verified in example_manual_chunks, example_use_client_effect, and example_use_server_mount snapshot outputs
- **Committed in:** 23a070b (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug - approach correction)
**Impact on plan:** The plan's suggested approach was in the right direction but insufficient. The fix uses a correct mechanism (Program + build) that properly preserves comments.

## Issues Encountered
- OXC's `Codegen::print_expression()` does not support comments because `build_comments()` is only called in `build()` which takes a full `Program`. This required a different approach than the plan specified but achieved the same outcome.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- BUG-02 and BUG-06 are fixed, reducing snapshot diffs by ~15 tests
- Ready for 03-02 (TS stripping, BUG-01) and 03-03 (captures, missing segments, test fixtures)
- No blockers introduced

---
*Phase: 03-bugs-correctness*
*Completed: 2026-02-20*

## Self-Check: PASSED
