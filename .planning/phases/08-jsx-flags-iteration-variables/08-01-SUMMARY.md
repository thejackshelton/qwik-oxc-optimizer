---
phase: 08-jsx-flags-iteration-variables
plan: 01
subsystem: jsx-transform
tags: [jsx, flags, immutability, scope-analysis, const-bindings]

# Dependency graph
requires:
  - phase: 05-jsx-keys-flags
    provides: JSX flag computation infrastructure (is_const_expression, is_child_expression_immutable, immutable_function_cmp)
provides:
  - const_bindings HashSet on ImportTracker populated from imports and const declarations
  - Scope-aware is_const_expression_with_scope function
  - Scope-aware is_child_expression_immutable (only const-bound identifiers are immutable)
  - 29 JSX flag mismatch reductions
affects: [08-02, 08-03]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "const_bindings scope tracking on ImportTracker for SWC ConstCollector approximation"
    - "enter_variable_declaration hook for const-declaration tracking during traversal"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs
    - crates/qwik-optimizer-oxc/src/is_const.rs

key-decisions:
  - "const_bindings populated from imports at init + from const declarations via enter_variable_declaration hook"
  - "is_const_expression_with_scope is fully recursive for compound expressions (binary, conditional, template literal, etc.)"
  - "Member expressions in children: use const_bindings instead of module_imports scan for consistency"

patterns-established:
  - "Scope-aware identifier classification via const_bindings HashSet threaded through JSX analysis functions"

# Metrics
duration: 5min
completed: 2026-02-21
---

# Phase 08 Plan 01: Scope-Aware JSX Flag Classification Summary

**const_bindings HashSet tracking imports + const declarations for scope-aware JSX prop/children immutability classification, fixing 29 flag mismatches**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-21T11:18:24Z
- **Completed:** 2026-02-21T11:23:42Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Added `const_bindings: HashSet<String>` to ImportTracker, populated from all import specifiers and const declarations during traversal
- Implemented `is_const_expression_with_scope()` with full recursive scope-aware classification for compound expressions
- Updated `is_child_expression_immutable` to use scope-aware identifier checks instead of blanket "all identifiers are immutable"
- Fixed 29 JSX flag mismatches: 16 identifier props correctly classified as const (3->1), 13 mutable identifier children correctly classified as mutable (1->3)

## Task Commits

Each task was committed atomically:

1. **Task 1: Build const_bindings set and thread through ImportTracker** - `779df44` (feat)
2. **Task 2: Update is_const_jsx_value and is_child_expression_immutable for scope awareness** - `4d10b9a` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added const_bindings field to ImportTracker, populated from imports in new(), added enter_variable_declaration hook and collect_const_binding_names helper
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Updated is_const_jsx_value and is_child_expression_immutable to use const_bindings for scope-aware classification
- `crates/qwik-optimizer-oxc/src/is_const.rs` - Added is_const_expression_with_scope() with full recursive scope-aware compound expression handling

## Decisions Made
- **const_bindings populated from imports at init + const declarations via hook:** Mirrors SWC's ConstCollector which tracks both. The enter_variable_declaration hook catches const declarations during AST traversal.
- **Fully recursive is_const_expression_with_scope:** Initially delegated compound expressions to the non-scope-aware version, but this caused `dep.thing + "stuff"` to be incorrectly classified as non-const when `dep` is an import. Fixed by making all compound expression arms recurse with scope awareness.
- **Member expressions in children use const_bindings:** Replaced the module_imports scan in is_child_expression_immutable's member expression handling with a simpler const_bindings.contains() check. The const_bindings already includes all imports, making the module_imports scan redundant for this purpose.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] is_const_expression_with_scope not recursing through compound expressions**
- **Found during:** Task 2 (scope-aware classification)
- **Issue:** Initial implementation delegated compound expressions (binary, conditional, etc.) to the non-scope-aware `is_const_expression`, causing `dep.thing + "stuff"` to be incorrectly classified as non-const even when `dep` is an import
- **Fix:** Made `is_const_expression_with_scope` fully recursive -- all compound expression arms (binary, conditional, template literal, unary, object, array, parenthesized) now recurse with scope awareness
- **Files modified:** `crates/qwik-optimizer-oxc/src/is_const.rs`
- **Verification:** Snapshot diffs show correct classification of compound import expressions
- **Committed in:** `4d10b9a` (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Essential for correctness of compound expression classification. No scope creep.

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- const_bindings infrastructure ready for use in 08-02 (iteration variable capture) and 08-03 (additional flag fixes)
- 29 of 34 expected flag mismatches fixed (exceeds the 20 minimum target)
- Remaining flag mismatches likely involve more complex scope patterns (globals, function params in specific contexts)

## Self-Check: PASSED

---
*Phase: 08-jsx-flags-iteration-variables*
*Completed: 2026-02-21*
