---
phase: 05-jsx-keys-flags
plan: 01
subsystem: jsx-transform
tags: [jsx, keys, base64, root-jsx-mode, should-emit-key]

# Dependency graph
requires:
  - phase: 04-signal-props-transforms
    provides: "JSX transform functions with props wrapping and signal support"
provides:
  - "root_jsx_mode state tracking across all statement-level scopes"
  - "File-hash-based key prefix computation (not hardcoded)"
  - "Correct key-vs-null decisions matching SWC's should_emit_key logic"
affects: [05-jsx-keys-flags, 06-import-ordering]

# Tech tracking
tech-stack:
  added: []
  patterns: ["root_jsx_mode save/restore stack pattern for nested scopes"]

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "Manual base64url encoding (6-bit lookup table) instead of base64 crate dependency"
  - "root_jsx_mode hooks added to all SWC-equivalent statement types: function, arrow, for/for-in/for-of, while, do-while, if, block, return"
  - "is_fn detection via uppercase first char check on Identifier/IdentifierReference + MemberExpression match"

patterns-established:
  - "root_jsx_mode stack: push(current) on enter, set true, pop on exit -- same pattern as loop_depth but for JSX key decisions"
  - "key_prefix threading: computed once in QwikTransform::new(), passed by reference through all JSX transform functions"

# Metrics
duration: 9min
completed: 2026-02-20
---

# Phase 5 Plan 01: JSX Keys Summary

**File-hash key prefix and root_jsx_mode state for correct key-vs-null JSX key generation**

## Performance

- **Duration:** 9 min
- **Started:** 2026-02-20T17:45:33Z
- **Completed:** 2026-02-20T17:54:47Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Key prefix computed from base64url(DefaultHasher(scope?,filename)) instead of hardcoded "u6" -- varies per file correctly
- root_jsx_mode tracks first-element-in-scope across function/arrow/for/while/do-while/if/block/return statements
- should_emit_key = is_fn || root_jsx_mode correctly determines key vs null for each JSX element
- Component tags (uppercase) and root elements always get keys; nested native elements get null
- Fragments always emit keys (is_fn=true equivalent)
- 160 snapshot files affected (key changes + formatting improvements from prior phases)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add root_jsx_mode state and file_hash key prefix computation** - `9c56c89` (feat)
2. **Task 2: Update key generation logic in jsx_transform.rs** - `33ffc92` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added root_jsx_mode/stack/key_prefix fields, computed key prefix in new(), added enter/exit hooks for 9 statement types, threaded params to JSX transform calls
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added root_jsx_mode and key_prefix params to all JSX transform functions, replaced key heuristic with should_emit_key = is_fn || root_jsx_mode

## Decisions Made
- Used manual base64url encoding (6-bit lookup table) to avoid adding base64 crate dependency -- only need 2 chars so a simple table lookup is cleaner
- Added root_jsx_mode hooks to all 9 statement types that SWC handles (function, arrow, for, for-in, for-of, while, do-while, if, block, return) to match SWC's fold method coverage
- Prefixed unused `has_any_visible_prop` with underscore since key generation no longer depends on prop presence

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- JSX key generation now matches SWC behavior for key-vs-null decisions and key prefix computation
- Remaining Phase 5 work: immutability flags (05-02) for correct flag bit computation
- Phase 6: import ordering, deferred QRL hoisting, use*() inlining

## Self-Check: PASSED

---
*Phase: 05-jsx-keys-flags*
*Completed: 2026-02-20*
