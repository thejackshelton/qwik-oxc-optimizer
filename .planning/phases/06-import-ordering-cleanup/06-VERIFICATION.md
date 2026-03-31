---
phase: 06-import-ordering-cleanup
verified: 2026-02-20T22:30:00Z
status: gaps_found
score: 3/5 must-haves verified
gaps:
  - truth: "No missing or extra imports remain in any output module (correct import set per module)"
    status: failed
    reason: "138 snapshot diffs remain with import set mismatches (missing _wrapProp, extra _captures, etc.); lazy import ordering in entry module sorts by import path but SWC sorts by a different key (traversal order / BTreeMap id key, not path)"
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "Lazy import sort key (a.1.cmp(&b.1) = path sort) does not reproduce SWC's ordering which uses BTreeMap<Id> keyed by identifier name (i_HASH). Segment body imports still contain set differences (missing _wrapProp in some segments, extra _fnSignal in others)"
    missing:
      - "Lazy imports in entry module sorted by identifier name (i_HASH key) rather than import path, to match SWC BTreeMap<Id> ordering"
      - "138 snapshot diffs still require resolution across capture list differences, _fnSignal hoisting, q:p/var_props ordering, JSX flag differences, and text normalization"
  - truth: "Relative import paths match SWC format exactly"
    status: failed
    reason: "relative_paths.snap has 215 diff lines (115 added + 100 removed); deep structural differences between OXC and SWC output for the relative_paths fixture — not just import path format but entire output structure differs"
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "relative_paths test shows inlinedQrl strategy vs qrl hoisting strategy difference; output has entirely different structure from SWC golden (different imports, different component body, different segment structure)"
    missing:
      - "Relative path test fixture alignment with SWC output structure (likely requires inline strategy vs segment strategy reconciliation)"
  - truth: "Snapshot diff count significantly reduced from 156 remaining diffs"
    status: partial
    reason: "Diff count reduced from 156 to 138 (18 fewer, 11.5% improvement). The SUMMARY claimed this as 'significant' but 138/162 still differ (85%). The success criterion required 'significant' reduction which is subjective; actual reduction is moderate."
    artifacts: []
    missing:
      - "Further reduction of 138 remaining diffs (capture list differences, _fnSignal hoisting, q:p ordering, JSX flags from scope analysis)"
---

# Phase 6: Import Ordering & Cleanup Verification Report

**Phase Goal:** Fix import scoping, ordering, and remaining code-level diffs to close the gap toward 0/162 snapshot diffs
**Verified:** 2026-02-20T22:30:00Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Import statements appear in the same order as SWC output (consistent sorting algorithm applied) | VERIFIED | `code_move.rs` line 220: `imports.sort_by(\|a, b\| a.local_name.cmp(&b.local_name))` implements SegmentImportEntry sort; 06-02 SUMMARY confirms zero ordering-only differences across 189 comparable segment modules |
| 2 | No missing or extra imports remain in any output module (correct import set per module) | FAILED | 138 snapshot diffs remain with import set mismatches; lazy imports in entry module sort by import path but this does not reproduce SWC's BTreeMap<Id> ordering; segment bodies still have set differences |
| 3 | Relative import paths match SWC format exactly | FAILED | `relative_paths.snap` has 215 diff lines; deep structural differences (inlinedQrl vs qrl strategy); not an import path format issue — entire output structure differs from SWC golden |
| 4 | QRL calls inside loops are hoisted to const declarations matching SWC | VERIFIED | `transform.rs` lines 241, 388, 1299, 1598-1623, 2846-2892: `pending_loop_qrl_hoists` buffer + `flush_qrl_hoists_to_body` helper confirmed in code; `example_component_with_event_listeners_inside_loop.snap.new` shows `const App_component_loopForI_span_q_e_click_XXX = qrl(...)` before `for` loops |
| 5 | Snapshot diff count significantly reduced from 156 remaining diffs | PARTIAL | Reduced 156 → 138 (18 fewer = 11.5% improvement, 24 now match exactly). Improvement achieved but 138/162 still differ (85% still differing). Whether this qualifies as "significant" is subjective. |

**Score:** 3/5 truths verified (SC1 verified, SC4 verified, SC5 partial)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | QRL hoisting buffer + entry module lazy import ordering + referenced_idents filtering | VERIFIED | Lines 241 (`pending_loop_qrl_hoists` field), 388 (init), 1299 (push), 1598-1623 (flush), 2681 (lazy_imports sort_by), 2570 (collect_referenced_idents call), 3005 (collect_referenced_idents fn) — all substantive implementations present |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | Segment import ordering with SegmentImportEntry struct | VERIFIED | Lines 14-220: `SegmentImportEntry` struct defined, collect-sort-emit pattern implemented with `imports.sort_by(\|a, b\| a.local_name.cmp(&b.local_name))` |
| `crates/qwik-optimizer-oxc/src/import_rewrite.rs` | `build_multi_specifier_import` for merged imports | VERIFIED | Line 186: `pub(crate) fn build_multi_specifier_import` exists and is called from transform.rs line 2753 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `replace_jsx_element_handlers` (loop detection) | `exit_function`/`exit_arrow_function_expression` | `pending_loop_qrl_hoists` Vec buffer | WIRED | transform.rs: push at line 1299, flush at lines 1598-1623 |
| `exit_arrow_function_expression` flush | `flush_qrl_hoists_to_body` helper | `std::mem::take(&mut self.pending_loop_qrl_hoists)` | WIRED | transform.rs lines 1621-1623, helper at 2846-2892 |
| `exit_program` lazy import ordering | `import_tracker.lazy_imports` | `sort_by(\|a, b\| a.1.cmp(&b.1))` (path sort) | WIRED but incorrect | Line 2681: sort exists but sorts by import path, not identifier name; golden ordering does not match pure-path alphabetical sort |
| `exit_program` dead import filtering | `collect_referenced_idents` | `referenced_idents.contains()` checks | WIRED | Lines 2570, 2685, 2700, 2734, 2781 — referenced_idents filtering applied to lazy imports, synthetic imports, and non-Qwik imports |
| `SegmentImportEntry` collect | `imports.sort_by()` | `local_name.cmp()` | WIRED | code_move.rs line 220 — correct alphabetical sort matching SWC |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| IMP-01 (Import statements in same order as SWC) | PARTIAL | Segment module imports: SATISFIED (sort by local_name matches SWC). Entry module lazy imports: NOT SATISFIED (sort by path != SWC BTreeMap<Id> key ordering) |
| IMP-02 (No missing or extra imports per module) | BLOCKED | 138 snapshot diffs remain with import set mismatches across entry and segment modules |
| IMP-03 (Relative import paths match SWC format) | BLOCKED | relative_paths.snap has 215 diff lines — structural differences beyond import path format; OXC uses different strategy (inlinedQrl vs segment extraction) |
| IMP-04 (QRL calls hoisted to const declarations) | SATISFIED | QRL hoisting implemented in transform.rs with pending_loop_qrl_hoists buffer and flush_qrl_hoists_to_body helper |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `transform.rs` | 2681 | `a.1.cmp(&b.1)` sort by path (incorrect key) | Warning | Lazy import ordering in entry module doesn't reproduce SWC's BTreeMap<Id> ordering |
| None | — | No TODOs/FIXMEs/placeholders in Phase 6 changes | — | — |

### Human Verification Required

None — all verifiable programmatically via snapshot comparison.

### Gaps Summary

Phase 6 delivered partial goal achievement. The implementation is technically correct and substantive:

**What works (verified in code):**
- Segment module import sorting (IMP-01 for segments): `SegmentImportEntry.sort_by(local_name)` in `code_move.rs` correctly matches SWC's `local_idents.sort()` behavior. 06-02 confirmed zero ordering-only differences.
- QRL hoisting (IMP-04): `pending_loop_qrl_hoists` buffer and `flush_qrl_hoists_to_body` helper correctly hoist QRL const declarations before loops in function bodies. Confirmed in `example_component_with_event_listeners_inside_loop.snap.new`.
- Entry module import filtering: `collect_referenced_idents` walker eliminates dead imports from entry module, with JSX element name handling and BTreeMap-based specifier merging.
- Code compiles cleanly (confirmed with `cargo check`).

**What still differs:**
- 138/162 snapshots (85%) still differ from SWC golden output.
- Lazy import ordering in entry module: the `sort_by(|a, b| a.1.cmp(&b.1))` (sort by import path) does not reproduce SWC's `BTreeMap<Id>` ordering keyed by identifier name. The `example_component_with_event_listeners_inside_loop` shows OXC order (loopArrowFn, loopForI, loopForOf, loopForIn, loopWhile, div_button) vs SWC order (loopWhile, loopForI, div_button, loopArrowFn, loopForIn, loopForOf) — neither is purely alphabetical by path.
- `relative_paths.snap` has 215 diff lines with structural differences (different strategy: OXC inlinedQrl vs SWC segment extraction), not import path format differences. This is a deeper issue out of scope for import ordering.
- Remaining 138 diffs include: capture list differences, `_fnSignal` hoisting location, `q:p`/var_props ordering, JSX scope analysis flags (21 snapshots from Phase 5), text node normalization, and segment body code differences.

The phase goal ("close the gap toward 0/162 snapshot diffs") was partially achieved: 18 fewer diffs (156→138), 24 snapshots now match exactly. However, the success criteria required the gap to be "significantly reduced" — the 11.5% improvement (18/156 diffs fixed) and 85% still-differing rate suggests the gap toward 0 remains substantial.

---

_Verified: 2026-02-20T22:30:00Z_
_Verifier: Claude (gsd-verifier)_
