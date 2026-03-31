---
phase: 07-entry-module-emission-fixes
verified: 2026-02-21T09:51:07Z
status: gaps_found
score: 3/4 must-haves verified
gaps:
  - truth: "Snapshot diff count reduced from 138 to <=80"
    status: failed
    reason: >
      The snapshot FILE count remains at 138 — identical to before phase 7.
      Phase 7 reduced diff LINES from 4554 to 4405 (149 lines, 3.3%), not
      diff file count. The ROADMAP success criterion #4 said 'reduced from
      138 to <=80' which was interpreted as a file count goal. The phase's
      own SUMMARY correctly acknowledged this: 'same file count, but fewer
      diff lines.' The criterion is ambiguous but the ROADMAP states it
      explicitly as a count-to-80 goal, which was not achieved.
    artifacts:
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/"
        issue: "138 .snap files still differ from SWC golden references; target was <=80"
    missing:
      - "Remaining diff categories (JSX flags, _fnSignal counter naming, iteration variable captures, prop ordering) need Phases 8-9 to reduce file count"
      - "Phase 7 changes are correct but do not reduce file count — _hf* leakage and per-segment filtering affect diff LINES within files that already differed for other reasons"
---

# Phase 7: Entry Module Emission Fixes Verification Report

**Phase Goal:** Fix entry module output to eliminate _hf* leakage, false-positive imports, and lazy import ordering — the three largest categories of remaining snapshot diffs
**Verified:** 2026-02-21T09:51:07Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `_hf*` declarations only appear in segment files, never in entry module output (segment strategy) | VERIFIED | 0 segment-strategy tests have `_hf*` in their `test.js` entry module section (checked all 138 differing .snap.new files) |
| 2 | `_fnSignal` import only added to segment files whose body_code actually contains `_fnSignal` (no false positives from global hoisted_stmts check) | VERIFIED | The `\|\| !hoisted_stmts.is_empty()` proxy removed at code_move.rs:106. Remaining `_fnSignal` imports in output reflect genuine usage; "apparent" false positives in 3 tests are genuine OXC/SWC transform differences not related to this fix. |
| 3 | Inline/hoist strategy entry modules still contain `_hf*` declarations (no regression) | VERIFIED | All 10 inline-strategy tests with `_hf*` in SWC golden reference also produce `_hf*` in OXC output; is_inline_like_strategy gate works correctly |
| 4 | Snapshot diff count reduced from 138 to <=80 | FAILED | Snapshot FILES differing: 138 (unchanged). Diff LINES: 4554 -> 4405 (149 fewer, 3.3% reduction). The file count metric was not met. |

**Score:** 3/4 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/lib.rs` | Conditional _hf* injection (skip for segment strategy) — `is_inline_like_strategy` variable | VERIFIED | Lines 172-174: `is_inline_like_strategy` variable computed, used in `if !hoisted_stmts.is_empty() && is_inline_like_strategy` gate |
| `crates/qwik-optimizer-oxc/src/code_move.rs` | Per-segment _hf* filtering and correct _fnSignal import detection | VERIFIED | Line 106: `if body_code.contains("_fnSignal")` (proxy removed). Lines 273-288: per-segment filtering loop with `body_code.contains(var_name)` |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `lib.rs` | `entry_strategy::should_inline` | `is_inline_like_strategy` check before hoisted_stmts injection | WIRED | lib.rs:172 calls `entry_strategy::should_inline(&transform_options.entry_strategy)`, result gates hoisted_stmts injection at line 174 |
| `code_move.rs` | `body_code` | `body_code.contains("_fnSignal")` — no proxy | WIRED | code_move.rs:106: exact check, no `\|\| !hoisted_stmts.is_empty()` fallback |
| `code_move.rs` | `body_code` | `body_code.contains(var_name)` for per-segment _hf* | WIRED | code_move.rs:275-279: `strip_prefix("const ")` extracts var_name, then `body_code.contains(var_name)` filters injection |

### Anti-Patterns Found

No anti-patterns found in the modified source files. No TODOs, placeholders, empty implementations, or stub patterns in `lib.rs` or `code_move.rs` changes.

### Gap Analysis

**Gap: Snapshot file count unchanged (138, target <=80)**

Root cause: Phase 7 correctly fixed three narrowly scoped bugs:
1. `_hf*` leaking into entry modules — this caused diff LINES within segment-strategy tests, but those tests already differed for other reasons (JSX flags, prop ordering, etc.). Removing the leakage reduces diff lines in those tests but does not make any test fully match.
2. `_fnSignal` false-positive proxy — same situation: fixes diff lines within tests that already differed.
3. Per-segment _hf* filtering — same situation.

The 138 files differing is explained by categories that Phases 8-9 target: JSX immutability flags (108 affected tests), iteration variable handling, key counter ordering, and capture list differences. Phase 7's fixes are correct prerequisites for Phase 8-9 isolation but do not close any test completely on their own.

**Actual diff reduction measured:**
- Pre-phase-7 baseline: 4,554 diff lines (138 files)
- Post-phase-7 current: 4,405 diff lines (138 files)
- Reduction: 149 lines (3.3%)
- SUMMARY claimed "~4416 to ~4267" — actual measured is 4554 to 4405; same magnitude (149) but different base

**The ROADMAP criterion #4 ("reduced from 138 to <=80") was set before phase 7 planning revealed that _hf* leakage and false-positive imports primarily cause diff LINES, not diff FILES.** The three targeted fixes work correctly, but they don't close whole snapshot files because those files differ for multiple independent reasons. This is documented in the SUMMARY ("same file count, but fewer diff lines") — the criterion needed to be updated to reflect what phase 7 could realistically achieve.

### Human Verification Required

None — all four success criteria are verifiable programmatically. The file count (138) and diff line reduction (149 lines) are exact measurements.

---

_Verified: 2026-02-21T09:51:07Z_
_Verifier: Claude (gsd-verifier)_
