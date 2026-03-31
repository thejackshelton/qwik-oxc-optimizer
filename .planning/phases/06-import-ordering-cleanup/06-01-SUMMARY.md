---
phase: 06-import-ordering-cleanup
plan: 01
subsystem: transform
tags: [oxc, imports, dead-code-elimination, entry-module, specifier-merging]

# Dependency graph
requires:
  - phase: 05-jsx-keys-flags
    provides: "Complete JSX transform with keys and immutability flags"
  - phase: 06-import-ordering-cleanup (plan 02)
    provides: "Segment import ordering (alphabetical sort by local name)"
provides:
  - "Entry module unused import filtering (both framework and user imports)"
  - "Non-dollar Qwik core specifier scoping (only emit when referenced in entry-level code)"
  - "Multi-specifier import merging by source module"
  - "Correct entry module body ordering (synthetic -> lazy -> non-dollar -> user imports -> code)"
  - "collect_referenced_idents walker for post-hoc identifier scanning"
  - "build_multi_specifier_import AST builder"
affects: [06-03-cleanup]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Post-hoc referenced-ident scanning for dead import elimination (alternative to DCE)"
    - "BTreeMap grouping for multi-specifier import merging with stable ordering"
    - "7-phase body assembly in exit_program"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/import_rewrite.rs"

key-decisions:
  - "Post-hoc filtering instead of scope-tracking: scan non-import statements for referenced identifiers after traversal, rather than tracking import usage during traversal. Simpler and matches SWC's DCE approach conceptually."
  - "collect_referenced_idents descends into nested function/arrow bodies (needed for inline strategy where segment code stays in entry module)"
  - "JSX element names require dedicated walker functions (JSXIdentifier/JSXElementName are separate from Expression::Identifier)"
  - "BTreeMap for specifier grouping preserves insertion order within each source module (matches SWC's original specifier ordering)"
  - "Side-effect imports (no specifiers) always kept in entry module regardless of reference scanning"

patterns-established:
  - "7-phase exit_program assembly: (1) separate imports from body, (2) collect references, (3) build synthetic imports, (4) filter synthetic by reference, (5) collect/filter/group non-dollar specifiers, (6) filter non-Qwik user imports, (7) assemble body"
  - "collect_referenced_idents as reusable identifier scanning utility for any statement list"

# Metrics
duration: 20min
completed: 2026-02-20
---

# Phase 6 Plan 01: Entry Module Import Scoping Summary

**Post-hoc referenced-ident filtering in exit_program eliminates segment-only imports from entry module, plus BTreeMap-based specifier merging for same-source Qwik core imports**

## Performance

- **Duration:** 20 min
- **Started:** 2026-02-20T21:09:37Z
- **Completed:** 2026-02-20T21:30:10Z
- **Tasks:** 3 (all implemented together -- tightly coupled in exit_program rewrite)
- **Files modified:** 2

## Accomplishments
- Entry module no longer emits framework imports (e.g., `_jsxSorted`, `_wrapProp`, `_Fragment`) that are only used inside segment bodies
- Entry module no longer emits non-Qwik user imports (e.g., `mongodb`, `threejs`, `css1`) that are only referenced by segments
- Non-dollar Qwik core specifiers (`useStore`, `mutable`, etc.) only appear when referenced in entry-module-level code
- Multi-specifier imports from the same source are merged (e.g., `import { useStore, mutable } from "@qwik.dev/core"`)
- 8 snapshot test cases now match SWC golden output (156 diffs reduced to 148)

## Task Commits

All 3 tasks were tightly coupled in a single `exit_program` rewrite and committed together:

1. **Task 1: Implement unused import filtering and body reordering** - `369e836` (feat)
2. **Task 2: Fix entry module non-dollar Qwik core specifier scoping** - `369e836` (same commit -- tightly coupled)
3. **Task 3: Merge entry module non-dollar Qwik core specifiers by source** - `369e836` (same commit -- tightly coupled)

**Plan metadata:** `4e79743` (docs: complete plan)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Rewrote `exit_program` with 7-phase body assembly; added ~400 lines of `collect_referenced_idents` walker functions (handles expressions, statements, JSX elements/fragments, class elements, binding patterns, assignment targets)
- `crates/qwik-optimizer-oxc/src/import_rewrite.rs` - Added `build_multi_specifier_import` function for creating import statements with multiple specifiers from `(imported_name, local_name)` pairs

## Decisions Made

1. **Post-hoc filtering over scope-tracking:** Rather than tracking which imports are used during traversal (complex and error-prone with segment extraction), scan the final entry module body for referenced identifiers after all transforms complete. This is conceptually equivalent to SWC's DCE approach.

2. **Walker descends into nested bodies:** The `collect_referenced_idents` walker descends into arrow functions, regular functions, and class methods. This is correct for inline/hoist strategies where segment code remains in the entry module.

3. **JSX element names need dedicated handling:** JSX element names like `<Counter>` use `JSXIdentifier`/`JSXElementName::IdentifierReference`, not regular `Expression::Identifier`. Discovered this when `issue_476` test regressed (Counter import was incorrectly filtered out).

4. **BTreeMap for specifier grouping:** Uses `BTreeMap<String, Vec<(String, String)>>` to group specifiers by source module, preserving insertion order within each group (matching SWC's behavior of keeping original specifier order, not sorting alphabetically).

5. **Side-effect imports preserved:** Import declarations with no specifiers (e.g., `import "side-effect-module"`) are always kept regardless of reference scanning.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] JSX element name walker missing**
- **Found during:** Task 1 (collect_referenced_idents implementation)
- **Issue:** The initial `collect_idents_from_expression` function didn't handle `Expression::JSXElement` or `Expression::JSXFragment`. JSX element names (`<Counter>`) use `JSXIdentifier`/`JSXElementName::IdentifierReference`, separate from regular expression identifiers. This caused the `issue_476` test to regress (Counter import filtered out incorrectly).
- **Fix:** Added `collect_idents_from_jsx_element`, `collect_idents_from_jsx_child`, and `collect_idents_from_jsx_member_expr` functions to walk JSX trees and collect referenced component identifiers.
- **Files modified:** `crates/qwik-optimizer-oxc/src/transform.rs`
- **Verification:** `issue_476` snapshot matches golden (Counter import correctly preserved)
- **Committed in:** `369e836`

**2. [Rule 1 - Bug] Rust 2024 binding modifier compatibility**
- **Found during:** Task 1 (compilation)
- **Issue:** Used `ref import_decl` pattern binding which is not allowed under Rust 2024 edition's default binding mode.
- **Fix:** Removed `ref` and used `&import_decl.specifiers` explicitly.
- **Files modified:** `crates/qwik-optimizer-oxc/src/transform.rs`
- **Verification:** `cargo check` compiles cleanly
- **Committed in:** `369e836`

**3. [Rule 1 - Bug] OXC BindingRestElement API mismatch**
- **Found during:** Task 1 (compilation)
- **Issue:** Used `rest.rest.argument` but OXC's `BindingRestElement` has `argument` directly (not nested).
- **Fix:** Changed to `rest.argument` (2 occurrences).
- **Files modified:** `crates/qwik-optimizer-oxc/src/transform.rs`
- **Verification:** `cargo check` compiles cleanly
- **Committed in:** `369e836`

---

**Total deviations:** 3 auto-fixed (3 bugs)
**Impact on plan:** All auto-fixes necessary for correctness. No scope creep.

## Issues Encountered
- All 3 tasks were too tightly coupled to commit separately -- the `exit_program` rewrite implements all three concerns (import filtering, specifier scoping, specifier merging) in a single cohesive 7-phase body assembly. Committed as a single atomic unit.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Entry module import scoping complete -- 8 snapshots now match golden
- 148 remaining snapshot diffs are addressed by other plans:
  - 06-02 (segment import ordering) -- already complete
  - 06-03 (remaining cleanup: QRL hoisting, use*() destructuring inlining, dedup suffix naming)
- The `collect_referenced_idents` walker can be reused by future plans if needed

---
*Phase: 06-import-ordering-cleanup*
*Completed: 2026-02-20*

## Self-Check: PASSED
