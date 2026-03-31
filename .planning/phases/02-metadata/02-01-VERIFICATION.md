---
phase: 02-metadata
verified: 2026-02-20T10:15:00Z
status: gaps_found
score: 3/5 must-haves verified
gaps:
  - truth: "Every segment's metadata JSON includes a paramNames array listing the segment function's parameter names"
    status: partial
    reason: "22 segments have None where SWC has ['_', '_', 'var'] — these are event handlers inside loops where SWC injects iteration variables via transform_event_handler_with_iter_var (q:p transform). 1 segment (ModelImg_component_imgLoc_useResource) has '{track}' where SWC has '_rawProps' — useResource$ destructured callback not covered by props override."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "No transform_event_handler_with_iter_var equivalent — 22 event handlers inside .map()/.forEach()/for loops produce empty paramNames instead of ['_', '_', 'iterVar']. Also: props destructuring _rawProps override only applies to component$ (is_component_exit guard at line 1596), missing useResource$ and other $() forms."
    missing:
      - "q:p iteration variable injection: prepend ['_', '_'] + loop variable to paramNames for event handlers inside iteration contexts (Phase 4 scope)"
      - "useResource$ (and other inner closures) _rawProps override: extend props override logic beyond component$ to inner closures with destructured params that capture outer props"
    deferral_note: "22 of 23 mismatches (q:p iteration variable injection) are correctly deferred to Phase 4 per executor decision. 1 mismatch (useResource$ _rawProps) is borderline — assess whether it belongs in Phase 2 or Phase 3/4."
  - truth: "paramNames-related diff lines are eliminated across the ~60 affected snapshots"
    status: partial
    reason: "55 segments have correct non-empty paramNames (matching SWC); 191 segments correctly have empty/None paramNames. 23 segments remain mismatched: 10 tests with only q:p iteration diffs (deferred to Phase 4), 1 test (should_mark_props_as_var_props_for_inner_cmp) with useResource$ _rawProps diff."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/"
        issue: "160 .snap.new files pending acceptance; committed .snap files are stale vs current code. The .snap.new files show: 0 path diffs, 23 paramNames diffs (all categorized above)."
    missing:
      - "q:p iteration variable injection to close 22 event-handler-in-loop paramNames diffs"
      - "Accept/commit updated snapshots after remaining gaps are closed"
---

# Phase 2 Plan 01: Verification Report

**Phase Goal:** Segment metadata blocks in all output modules match SWC structure exactly (paramNames present, path fields populated correctly)
**Verified:** 2026-02-20T10:15:00Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Every segment's metadata JSON includes a paramNames array | PARTIAL | 246/269 segments match; 22 missing q:p iteration vars (Phase 4), 1 missing useResource$ _rawProps override |
| 2 | JSX event handler segments (ctxKind: eventHandler) include paramNames from arrow/function | VERIFIED | extract_param_names_from_jsx_expr wired at line 847-850 of transform.rs; all non-loop event handlers correct |
| 3 | component$ segments with props destructuring have _rawProps override | PARTIAL | Works for direct component$ destructuring (line 1667); NOT for useResource$ callbacks (is_component_exit guard restricts to component$ only) |
| 4 | The path field in segment metadata uses forward slashes on Windows-style paths | VERIFIED | rel_dir() uses .replace('\\', "/") at lib.rs:321; 0 path mismatches across 269 comparable segments |
| 5 | paramNames-related diff lines eliminated across ~60 affected snapshots | PARTIAL | 55 non-empty paramNames segments correct; 23 remaining diffs (22 q:p deferred, 1 useResource$ _rawProps) |

**Score:** 3/5 truths verified (Truths 2 and 4 fully verified; Truths 1, 3, 5 partial)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | binding_pattern_to_string, extract_param_names_from_* functions | VERIFIED | All 4 functions exist at lines 2262-2359; substantive implementations matching SWC's pat_to_string |
| `crates/qwik-optimizer-oxc/src/transform.rs` | record_segment uses extract_param_names_from_argument | VERIFIED | Wired at lines 629-633; replaces vec![] with extraction |
| `crates/qwik-optimizer-oxc/src/transform.rs` | record_jsx_event_segment accepts param_names Vec<String> | VERIFIED | Signature updated at line 693-698; param_names used at line 725 |
| `crates/qwik-optimizer-oxc/src/transform.rs` | create_jsx_event_segments_recursive calls extract_param_names_from_jsx_expr | VERIFIED | Wired at lines 847-850 |
| `crates/qwik-optimizer-oxc/src/lib.rs` | rel_dir normalizes backslashes to forward slashes | VERIFIED | .replace('\\', "/") at line 321 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| record_segment | extract_param_names_from_argument | call.arguments.first() | WIRED | Lines 629-633: if let Some(first_arg) = call.arguments.first() { extract... } |
| create_jsx_event_segments_recursive | extract_param_names_from_jsx_expr | container.expression | WIRED | Lines 847-850: param_names = extract_param_names_from_jsx_expr(&container.expression) |
| record_jsx_event_segment | param_names parameter | Vec<String> passed from caller | WIRED | Line 725: param_names, (used directly in SegmentData struct) |
| component$ exit | seg.param_names = _rawProps | is_component_exit guard | WIRED for component$ | Line 1667; only fires when is_component_exit=true (component$ calls only) |
| useResource$/inner closures | seg.param_names = _rawProps | Missing — guard too narrow | NOT WIRED | is_component_exit at line 1596 excludes useResource$, useTask$, and other closures |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| META-01: paramNames array in segment metadata | PARTIAL | 22 segments missing q:p iteration vars (Phase 4), 1 missing useResource$ _rawProps |
| META-02: path field populated, canonicalFilename no path prefix | VERIFIED | 0 path mismatches; 0 canonicalFilename path-prefix issues across all 269 comparable segments |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None found | — | — | — | — |

No TODO/FIXME/placeholder patterns found in the modified files.

### Gaps Summary

**Fully Resolved:**
- Path field normalization (META-02): 100% match across 269 comparable segments. rel_dir() correctly converts Windows backslashes to forward slashes. No canonicalFilename path-prefix issues.
- JSX event handler paramNames extraction: All non-loop event handlers correctly extract paramNames from arrow/function expressions.
- Regular $() call paramNames: All standard calls (component$, useTask$, useSignal$, etc.) correctly extract paramNames.
- Props destructuring override for component$: `seg.param_names = vec![info.raw_props_name.clone()]` correctly fires for direct component$ calls.

**Remaining Gaps:**

1. **q:p iteration variable injection (22 segments, 10 tests)** — Event handlers inside `.map()`, `for...of`, `for...in`, `for`, and `while` loops produce `None` paramNames when SWC produces `['_', '_', iterVar]`. This is the `transform_event_handler_with_iter_var` transform in SWC. Correctly deferred to Phase 4.

2. **useResource$ _rawProps override (1 segment, 1 test: `should_mark_props_as_var_props_for_inner_cmp`)** — The `useResource$(async ({ track }) => {...})` callback produces paramNames `['{track}']` where SWC produces `['_rawProps']`. The is_component_exit guard at line 1596 restricts the _rawProps override to component$ only. In SWC, `useResource$` callbacks with destructured parameters also get the _rawProps treatment. This is borderline Phase 2 vs Phase 3.

**Executor Note Assessed:**
The executor's statement "247 of 270 paramNames fields match SWC" is close but slightly off — actual measurement against .snap.new files shows 246/269 comparable segments match (slight discrepancy may be due to segment naming mismatches from Phase 3 bugs reducing the comparable set). The characterization of 22 remaining as "q:p iteration variable injection" is correct. The assessment that these are "separate Phase 4 transformation" is correct for 22/23 cases. The 23rd case (useResource$ _rawProps) was not called out explicitly and may need Phase 2 or Phase 3 resolution.

**Snapshot State:**
160 of 162 OXC snapshot files have .snap.new versions (pending acceptance). The committed .snap files are stale vs current code output. The .snap.new files (current code) show 0 path mismatches and 23 paramNames mismatches vs SWC golden reference.

---

_Verified: 2026-02-20T10:15:00Z_
_Verifier: Claude (gsd-verifier)_
