---
phase: 13-captures-dce
verified: 2026-02-24T09:57:12Z
status: passed
score: 5/5 must-haves verified
---

# Phase 13: Captures & DCE Verification Report

**Phase Goal:** Fix captures edge cases and dead code elimination to match SWC
**Verified:** 2026-02-24T09:57:12Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | apply_segment_body_dce function exists in transform.rs | VERIFIED | Line 6183: pub(crate) fn apply_segment_body_dce(body_code: &str, force_remove_names: Option<&HashSet<String>>) -> String |
| 2 | Capture reclassification handles _rawProps properly | VERIFIED | Lines 3100-3177: reclassify child segment captures, replace prop alias captures with _rawProps, post-process body codes |
| 3 | C03 diagnostic emission works for non-function $() arguments | VERIFIED | Lines 3318-3358: first_arg_is_function check, C03 message "Qrl($) scope is not a function", captures cleared |
| 4 | Const literal bindings filtered from captures | VERIFIED | Lines 1337-1360: const_literal_bindings filtering of capture_names and reemitted_imports |
| 5 | No regressions: exact match count >= 63 | VERIFIED | cargo insta test confirmed 95 diffs from golden = 67 exact matches (162 - 95 = 67 >= 63) |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | apply_segment_body_dce, const_literal_bindings, fix_qrl_captures_in_body, C03 diagnostics | VERIFIED | 7246 lines; all functions present and substantive |
| `crates/qwik-optimizer-oxc/src/const_replace.rs` | VisitMut DCE pre-pass with module doc | VERIFIED | 299 lines; doc explains VisitMut recursion into inline strategy code |
| `crates/qwik-optimizer-oxc/src/lib.rs` | apply_segment_body_dce call loop, post-DCE import filtering | VERIFIED | Lines 249-276: DCE loop and post-DCE import filtering wired |
| `crates/qwik-optimizer-oxc/src/props_destructuring.rs` | Fixed ArrayExpression element handling | VERIFIED | 945 lines; index-based iteration for ArrayExpressionElement::Identifier |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `lib.rs` | `apply_segment_body_dce` | `body_codes.iter_mut()` loop at line 253 | WIRED | Called for each segment body code when MinifyMode::Simplify |
| `apply_segment_body_dce` | `apply_dce_to_statements` | Direct call at line 6237 | WIRED | Parses body code as arrow, runs DCE, reserializes |
| `const_replace` DeadBranchEliminator | inline strategy body code | VisitMut recursion | WIRED | Module doc confirms VisitMut walks all AST nodes including inlinedQrl callbacks |
| `transform.rs` exit_call_expression | C03 diagnostic | Lines 3318-3358: is_top_level_dollar_call gate | WIRED | C03 emitted when first arg not function + has captures + not top-level |
| `const_literal_bindings` | capture filtering | Lines 1337-1360: filter() call | WIRED | Filters both capture_names and reemitted_imports |
| `fix_qrl_captures_in_body` | QRL capture arrays | Lines 3167-3175: AST walk | WIRED | Called after reclassification to rebuild capture arrays |

### Requirements Coverage

| Requirement | Status | Notes |
|-------------|--------|-------|
| Remaining captures edge cases resolved (~17 tests, reduced by Phase 12 cascade) | PARTIALLY_SATISFIED | 4 tests fully resolved (example_multi_capture, example_9, example_capturing_fn_class, example_dead_code). 7 improved. 15 unchanged due to non-captures root causes or deferred to Phase 14 |
| DCE matches SWC for unused const/if(false)/function/class patterns (~9 tests) | PARTIALLY_SATISFIED | example_dead_code (if false) and example_9 (unused decls) fully resolved. Const-fold patterns (example_use_optimization, example_optimization_issue_4386) deferred as SWC MinifyMode::Simplify feature |
| No regressions in existing exact-match tests | SATISFIED | 67 exact matches (up from 63 pre-Phase 13); 0 regressions |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| transform.rs | 2859, 2887, 2920, etc. | "placeholder" | INFO | Idiomatic Rust std::mem::replace pattern, not code stubs |
| const_replace.rs | 223 | "placeholder" | INFO | Same Rust pattern -- not a stub |

No TODO/FIXME/XXX anti-patterns found in modified files.

### Human Verification Required

None -- all verification achievable programmatically for this phase.

### Gaps Summary

No gaps blocking goal achievement. Phase 13 succeeds on its primary success criteria:
- 67 exact matches confirmed via `cargo insta test` (95 .snap.new files = 95 diffs; 162 - 95 = 67)
- All 4 claimed-resolved tests verified as exact matches: example_multi_capture, example_9, example_capturing_fn_class, example_dead_code
- C03 diagnostics emitting correctly (confirmed in example_invalid_segment_expr1.snap.new)
- apply_segment_body_dce fully implemented and wired (Lines 6183-6261 + lib.rs lines 249-276)
- Const literal filtering fully implemented and wired (Lines 1337-1360)
- _rawProps reclassification implemented and wired (Lines 3100-3177)

Remaining diffs (95) are correctly categorized as either:
1. Aesthetic diffs (OXC codegen shorthand, import ordering) -- accepted per phase policy
2. Deferred to Phase 14 (const-fold SWC MinifyMode::Simplify, signal wrapping, hoist strategy)
3. Diagnostic highlights (C03 emitted but without source location spans)

Phase goal "Fix captures edge cases and dead code elimination to match SWC" achieved at the level Phase 13 targets.

---

_Verified: 2026-02-24T09:57:12Z_
_Verifier: Claude (gsd-verifier)_
