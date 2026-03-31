---
phase: 14-final-parity
verified: 2026-02-24T18:30:00Z
status: gaps_found
score: 5/8 must-haves verified
gaps:
  - truth: "Import ordering in segment modules matches SWC across all tests (~44 tests affected)"
    status: partial
    reason: "Encounter-order mechanism implemented and working (5 new exact matches from import-order-only tests), but ~44 affected tests did not all become exact matches because most have multiple diff categories. The mechanism is correct; remaining import-order diffs are entangled with JSX_FLAGS, SIGNAL_WRAP and other categories that belong to later phases."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "Mechanism is correct and substantive, but success criterion 'matches SWC across all tests' overstates what one mechanism can achieve when tests have multiple diff categories"
    missing:
      - "Resolution of entangled JSX_FLAGS, SIGNAL_WRAP, LINE_WRAP diffs that prevent import-order-fixed tests from becoming exact matches (these are Phase 15/16 scope)"
  - truth: "Dev mode file path format matches SWC (~4 tests)"
    status: partial
    reason: "The src_dir fix is implemented and the path format IS correct (/user/qwik/src/test.tsx appears in both golden and OXC output), but the 4 dev mode tests (example_dev_mode, example_dev_mode_inlined, example_drop_side_effects, example_noop_dev_mode) still fail due to unrelated JSX_FLAGS and prop ordering diffs from other categories."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/tests/test.rs"
        issue: "src_dir fix works correctly; remaining test failures are from JSX_FLAGS/SIGNAL_WRAP categories outside Phase 14 scope"
    missing:
      - "JSX flags and prop ordering fixes (Phase 15 scope) needed for these tests to become exact matches"
  - truth: "~28 new exact matches (72 → ~100/162)"
    status: failed
    reason: "Only 14 new exact matches achieved (72 → 86/162), not 28. The target of ~100 was not reached. ACTUAL RESULT: 86/162 exact matches. Each implemented fix produced correct behavior but most affected tests have multiple diff categories (JSX_FLAGS, SIGNAL_WRAP, LINE_WRAP, DCE) that prevent them from becoming exact matches until Phase 15/16 also land."
    artifacts: []
    missing:
      - "Phase 15 fixes (SIGNAL_WRAP, JSX_FLAGS) needed for ~11+17 additional tests"
      - "Phase 16 fixes (DCE, CAPTURES) needed for ~19+10 additional tests"
---

# Phase 14: Cosmetic & Small Fixes Verification Report

**Phase Goal:** Fix import ordering (biggest cosmetic category), spread props placement, and small metadata/config fixes to maximize exact matches with minimal risk
**Verified:** 2026-02-24T18:30:00Z
**Status:** gaps_found (mechanisms verified; exact match target not met due to entangled diff categories)
**Re-verification:** No - initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Import ordering in segment modules matches SWC encounter order | PARTIAL | `synthetic_import_order` Vec tracking implemented, `sort_by` removed, `_Fragment` deferred after lazy imports. 5 import-order-only tests became exact matches. ~39 affected tests still failing due to entangled JSX_FLAGS/SIGNAL_WRAP/DCE categories. |
| 2 | Spread props: _getConstProps separate 3rd arg / _createElement for simple spreads | VERIFIED | Single-spread `_jsxSplit` correctly places `_getConstProps` as 3rd arg. `_createElement` pattern for spread-only keyed elements. 4 new exact matches confirmed. |
| 3 | Entry field metadata correctly populated | VERIFIED | `stack_ctxt` on SegmentData, `compute_entry_field` with Smart/Component logic, `is_entry = entry.is_none()`. 4 exact matches confirmed. |
| 4 | Dev mode file path format matches SWC | PARTIAL | `/user/qwik/src/` and `/hello/from/dev/` src_dir overrides correctly set in test.rs. Dev path format IS correct in output. 0 new exact matches because JSX_FLAGS/prop ordering diffs (Phase 15 scope) prevent matching. |
| 5 | File extension handling matches SWC | PARTIAL | `preserve_filenames` guards extension transformation (`&& !transform_options.preserve_filenames` condition). Golden shows `test.tsx` and OXC output also shows `test.tsx`. 0 new exact matches due to entangled import ordering and JSX flags diffs. |
| 6 | ctxKind correctly classifies JSX prop events | VERIFIED | `JSXProp` variant added to `CtxKind` with `#[serde(rename = "jSXProp")]`. Native elements → EventHandler, component elements → JSXProp. Both golden and OXC output show matching `"jSXProp"` values. |
| 7 | Diagnostic highlight spans populated | VERIFIED | `compute_highlight_from_span` helper implemented. C03 highlights match exactly (startLine=7, startCol=14, endLine=7, endCol=18 etc.). C05 diagnostic emitted for missing Qrl counterparts (`example_missing_custom_inlined_functions` is exact match). |
| 8 | ~28 new exact matches (72 → ~100/162) | FAILED | 14 new exact matches achieved (72 → 86/162). Target was ~28/~100. Each mechanism is correct; the gap is that most affected tests have multiple diff categories requiring Phase 15/16 fixes. |

**Score:** 5/8 truths verified (3 partial, 1 failed; mechanisms for all 8 truths ARE implemented correctly)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | Encounter-order import tracking, `_Fragment` after lazy imports, `needs_create_element`, `synthetic_import_count`, C03/C05 diagnostics | VERIFIED | `synthetic_import_order: Vec<String>`, `record_synthetic_import()`, `needs_create_element: bool`, `compute_highlight_from_span()`, `byte_offset_to_line_col()`, C05 emission at line ~1952, C03 highlights at line ~3448 |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | `_getConstProps` 3rd arg for single-spread, `_createElement` early-return path, encounter-order `record_synthetic_import` calls | VERIFIED | 3-case `const_props_expr` algorithm (lines ~2804-3018), `_createElement` early-return at line ~2591, `record_synthetic_import` calls at all `needs_*` sites |
| `crates/qwik-optimizer-oxc/src/lib.rs` | `compute_entry_field` with Smart/Component logic, `is_entry` from `entry.is_none()`, `preserve_filenames` extension guard | VERIFIED | `compute_entry_field()` at line ~442, `is_entry = segment_analysis.entry.is_none()` at line ~370, `&& !transform_options.preserve_filenames` at line ~242 |
| `crates/qwik-optimizer-oxc/src/types.rs` | `JSXProp` variant in `CtxKind`, `stack_ctxt` field on `SegmentData` | VERIFIED | `JSXProp` at line 344-346 with `#[serde(rename = "jSXProp")]`, `stack_ctxt: Vec<String>` at line 576 |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | `_createElement` import detection for segment builds | VERIFIED | `body_code.contains("_createElement")` check at line ~211 |
| `crates/qwik-optimizer-oxc/tests/test.rs` | Dev mode src_dir overrides for 4 tests | VERIFIED | `/user/qwik/src/` for example_dev_mode, example_dev_mode_inlined, example_drop_side_effects at lines ~847/853/860; `/hello/from/dev/` for example_noop_dev_mode at line ~989 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `transform.rs` `synthetic_import_order` | `exit_program` Phase 3-7 assembly | Encounter-order iteration, no sort | WIRED | Lines ~3745-3884 build imports from `synthetic_import_order`, `_Fragment` emitted separately after lazy imports |
| `jsx_transform.rs` `_createElement` | `transform.rs` `needs_create_element` | Flag set in jsx_transform, emission in exit_program | WIRED | Flag set at line ~2608-2609, import emitted at line ~3802 |
| `jsx_transform.rs` `_createElement` | `code_move.rs` segment imports | `body_code.contains("_createElement")` pattern | WIRED | Line ~211 in code_move.rs |
| `types.rs` `SegmentData.stack_ctxt` | `lib.rs` `compute_entry_field` | `&seg.stack_ctxt` passed at line ~498 | WIRED |  |
| `transform.rs` `compute_highlight_from_span` | C03 diagnostic emission | Called at line ~3448 | WIRED | Highlight spans match golden exactly |
| `transform.rs` C05 emission | `enter_call_expression` | Fires for exported $-suffixed without Qrl counterpart | WIRED | Line ~1952 |

### Requirements Coverage

| Requirement | Status | Notes |
|-------------|--------|-------|
| Import ordering matches SWC | PARTIAL | Mechanism correct; entangled tests need Phase 15/16 |
| Spread props placement correct | SATISFIED | 4 new exact matches confirmed |
| Entry field metadata | SATISFIED | 4 new exact matches confirmed |
| Dev mode paths | PARTIAL | Mechanism correct; tests fail from JSX_FLAGS |
| File extension handling | PARTIAL | Mechanism correct; tests fail from other categories |
| ctxKind JSXProp | SATISFIED | Both golden and OXC output match |
| Diagnostic highlights | SATISFIED | Spans match exactly; `example_missing_custom_inlined_functions` exact match |
| 28 new exact matches | NOT SATISFIED | 14 new exact matches (86/162 total) |

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| None detected in phase 14 modified files | — | — | — |

No TODO/FIXME/placeholder patterns found in the modified files. All implementations are substantive.

### Gaps Summary

**What actually happened vs what was targeted:**

The phase achieved 14 new exact matches instead of the targeted ~28. All 7 mechanisms were correctly implemented and verified in the code. The gap between mechanism-correctness and exact-match-count is explained by multi-category test entanglement:

- Most tests affected by import ordering ALSO have JSX_FLAGS, SIGNAL_WRAP, LINE_WRAP, or DCE diffs. The import ordering fix alone cannot make these tests pass.
- Dev mode tests and preserve_filenames tests have JSX_FLAGS/prop ordering diffs that prevent exact matches even though the targeted categories (path format, file extension) are now correct.
- The `example_immutable_analysis` test still fails because of LINE_WRAP and JSX_FLAGS diffs (prop positioning, flag values) even though ctxKind now correctly outputs `"jSXProp"`.
- The `example_invalid_segment_expr1` test still fails due to a DCE diff (template literal `${css1}${css2}` appearing in segment output) even though C03 diagnostic highlights are exact.

**Root cause of target miss:** The ROADMAP success criterion 8 ("~28 new exact matches") was optimistic. The diff audit estimated 44 tests as "import-order-affected" but most had multiple diff categories. The plan's individual mechanism contributions were correct (+5 import ordering, +4 spread props, +4 entry field, +1 diagnostics = 14), but the ~28 target assumed more import-order-only tests existed.

**Significance:** 86/162 exact matches (53%) represents genuine semantic parity progress. The 14 new matches are real, verified improvements. The remaining 76 failing tests have clear categorization in SIGNAL_WRAP (~11), JSX_FLAGS (~17), DCE (~19), CAPTURES (~10), HOIST (~14), LINE_WRAP/SHORTHAND (~29 aesthetic) for Phase 15-18.

---

_Verified: 2026-02-24T18:30:00Z_
_Verifier: Claude (gsd-verifier)_
