---
phase: 06-import-ordering-cleanup
plan: 02
subsystem: segment-code-generation
tags: [import-ordering, code-move, segment-modules, sorting]
requires:
  - "05-03 (flag propagation gap closure)"
provides:
  - "Alphabetically sorted segment module imports matching SWC's local_idents.sort()"
  - "SegmentImportEntry struct for collect-sort-emit pattern"
affects:
  - "06-03 (remaining code-level diffs may change import sets)"
tech-stack:
  added: []
  patterns:
    - "Collect-sort-emit pattern for import ordering"
    - "SegmentImportEntry struct for sortable import metadata"
key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/code_move.rs"
key-decisions:
  - id: "06-02-01"
    decision: "Emit _captures import first (before sorted list), not sorted with other imports"
    reason: "SWC's code_move.rs emits _captures as a special case before the sorted local_idents loop"
    alternatives: ["Sort _captures with all other imports"]
  - id: "06-02-02"
    decision: "Use standard Rust string comparison (byte-level lexicographic) for sort order"
    reason: "Matches SWC's Atom::cmp which delegates to string comparison"
    alternatives: ["Case-insensitive sort", "Custom sort order"]
  - id: "06-02-03"
    decision: "Move lazy import declarations (const i_XXX) after all sorted import statements"
    reason: "SWC emits lazy imports via extra_top_items (BTreeMap sorted) after the import block"
    alternatives: ["Keep lazy imports in current position (after qrl, before framework imports)"]
metrics:
  duration: "8min"
  completed: "2026-02-20"
---

# Phase 6 Plan 02: Segment Import Ordering Summary

**Alphabetically sorted segment module imports via collect-sort-emit pattern, matching SWC's local_idents.sort() with _captures emitted first as special case.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | 8min |
| Tasks | 1/1 |
| Files modified | 1 |
| Lines changed | +191 / -105 |
| Ordering matches vs SWC | 141/189 segments (100% of same-set segments) |

## Accomplishments

1. **Replaced hardcoded import ordering with collect-sort-emit pattern**: The previous `build_segment_code_with_hoisted` function emitted imports in a fixed order determined by the if-chain sequence. Now all imports (framework + user-code) are collected into `Vec<SegmentImportEntry>`, sorted alphabetically by `local_name`, then emitted.

2. **Achieved zero ordering-only differences vs SWC**: Across all 189 comparable segment modules, there are zero cases where the same imports appear in different order. All 48 remaining differences are because the import SETS differ (missing `_wrapProp`, extra `_captures`, etc.) -- not ordering issues.

3. **Handled SWC's _captures special case**: SWC emits `_captures` import before the sorted local_idents loop. Since `_captures` doesn't naturally sort first (uppercase letters sort before underscore in ASCII), it needed to be emitted separately before the sorted list.

4. **Moved lazy import declarations to correct position**: `const i_XXX = () => import(...)` statements were moved from after `qrl` import to after all sorted import statements, matching SWC's emission order.

## Task Commits

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Refactor segment import emission to use sorted identifier-based ordering | d79accd | code_move.rs |

## Files Modified

- `crates/qwik-optimizer-oxc/src/code_move.rs`: Complete refactor of `build_segment_code_with_hoisted` from hardcoded if-chain to collect-sort-emit pattern. Added `SegmentImportEntry` struct.

## Decisions Made

1. **[06-02-01] _captures emitted first, not sorted**: SWC's `code_move.rs::new_module` emits `_captures` as a special case before iterating the sorted `local_idents`. While `_captures` often sorts near the top alphabetically, uppercase identifiers like `Lightweight` sort before underscore-prefixed names in standard byte comparison. Emitting `_captures` first matches SWC exactly.

2. **[06-02-02] Standard string comparison for sort**: SWC's `local_idents.sort()` sorts `(Atom, SyntaxContext)` tuples where `Atom::cmp` uses standard string comparison. Our `local_name.cmp()` produces identical ordering.

3. **[06-02-03] Lazy imports after sorted imports**: Lazy import declarations are `const` statements, not import statements. SWC emits them via `extra_top_items` (BTreeMap) after imports. Moved them from their previous position (between qrl and framework imports) to after all sorted imports.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] _captures must not be sorted with other imports**

- **Found during:** Task 1 verification
- **Issue:** Plan suggested including `_captures` in the sorted list, stating it "naturally sorts first among framework imports." This is incorrect: uppercase letters (A-Z = 0x41-0x5A) sort before underscore (_ = 0x5F) in standard comparison. User-code imports like `Lightweight` (L = 0x4C) sort before `_captures`.
- **Fix:** Emit `_captures` separately before the sorted import list, matching SWC's special-case behavior.
- **Files modified:** code_move.rs
- **Commit:** d79accd

## Issues Encountered

None beyond the deviation documented above.

## Next Phase Readiness

- **Remaining diffs are import SET differences, not ordering**: 48 segments have different imports (missing `_wrapProp`, `_fnSignal`, extra `_captures`, etc.). These are code-level issues from prior phases, not ordering problems.
- **Entry module import ordering** is handled by Plan 06-01 (separate plan).
- **Plan 06-03** addresses remaining code-level diffs including QRL hoisting, destructuring, and other deferred items.

## Self-Check: PASSED
