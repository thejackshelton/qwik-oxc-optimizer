---
phase: 08-jsx-flags-iteration-variables
verified: 2026-02-21T12:22:39Z
status: gaps_found
score: 5/6 must-haves verified
gaps:
  - truth: "JSX flag mismatches reduced from 108 to ≤15"
    status: failed
    reason: "33 flag mismatches remain across 33 element-level cases in 28+ test files, per the 08-03 SUMMARY audit. Goal was ≤15."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "Flag computation correct per SWC semantics, but remaining mismatches are caused by upstream prop/transform differences not yet implemented (e.g., _fnSignal dep constness, _noopQrl, dev mode transforms, code stripping, inline component _fnSignal wrapping)"
    missing:
      - "_fnSignal children dep constness checking (affects ~9 cases: SWC=0/OXC=2, SWC=1/OXC=3)"
      - "Inline component _fnSignal wrapping for non-component$ exports (affects 3 cases: destructure_args_inline_cmp_*)"
      - "_noopQrl/dev mode transforms (affects ~6 cases: example_dev_mode, example_noop_dev_mode)"
      - "signal.value reactive detection in const analysis (affects 1 case: should_transform_qrls_in_ternary_expression)"
      - "Client code stripping (affects 1 case: example_strip_client_code)"
      - "_fnSignal placement for non-component$ contexts (1 case: should_wrap_object_with_fn_signal)"
---

# Phase 8: JSX Flags & Iteration Variables Verification Report

**Phase Goal:** Fix JSX immutability flags to match SWC exactly by implementing identifier scope analysis, logical && propagation, loop event handler detection, and completing q:p injection

**Verified:** 2026-02-21T12:22:39Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Identifier references in JSX children classified as immutable only when they are imports or const bindings | VERIFIED | `is_child_expression_immutable` checks `const_bindings.contains(ident.name.as_str())` — snapshots show mutable identifier children correctly classified |
| 2 | Elements inside logical `&&` expressions correctly propagate mutability to parent | VERIFIED | `LogicalExpression` arm in `is_child_expression_immutable` recursively checks both sides with scope awareness |
| 3 | Event handlers inside loops have `static_listeners=false` when q:p/q:ps present | PARTIALLY VERIFIED | `has_qp` check clears `static_listeners`; q:p injection works for all 10 key test files. But 2 cases (example_functional_component_2, example_functional_component_capture_props) show OXC=1/SWC=0 — q:p ends up in const_props due to `for (const item of ...)` misclassification deferring to Phase 9 |
| 4 | q:p and q:ps iteration variable props injected for all affected cases | VERIFIED | All 10 affected test files confirmed to have correct q:p/q:ps counts matching SWC golden |
| 5 | `_rawProps` override applies to `useResource$` hooks | VERIFIED | `is_props_rewrite_exit` check covers `component$ \|\| useResource$`; confirmed in `should_mark_props_as_var_props_for_inner_cmp` snapshot |
| 6 | JSX flag mismatches reduced from 108 to ≤15 | FAILED | 33 flag mismatches remain (confirmed by 08-03 SUMMARY audit and validated against actual snapshot diffs): SWC=0/OXC=1 (2), SWC=0/OXC=2 (7), SWC=1/OXC=0 (1), SWC=1/OXC=3 (9), SWC=2/OXC=0 (1), SWC=2/OXC=3 (4), SWC=3/OXC=1 (6), SWC=3/OXC=2 (3) |

**Score:** 5/6 truths verified (criterion 6 failed — 33 mismatches remain vs ≤15 goal)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | `const_bindings` HashSet on ImportTracker | VERIFIED | Field exists at line 116, populated from imports in `new()` (lines 361-369) and from const declarations via `enter_variable_declaration` hook (lines 1653-1665) |
| `crates/qwik-optimizer-oxc/src/is_const.rs` | `is_const_expression_with_scope()` function | VERIFIED | 83-line fully recursive implementation with all expression arms (identifier, member, binary, conditional, template, unary, object, array, parenthesized) |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Scope-aware `is_const_jsx_value` and `is_child_expression_immutable` | VERIFIED | Both functions accept `const_bindings` param; identifiers checked against set |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | `is_const_event_handler()` function | VERIFIED | Event handler const classification: qrl()/inlinedQrl() = const, _qrlSync()/serverQrl() = non-const |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Corrected `static_listeners`/`static_subtree` computation | VERIFIED | `has_event_in_var_props` check for static_listeners; static_subtree no longer incorrectly affected by var_props presence |
| `crates/qwik-optimizer-oxc/src/transform.rs` | `iter_var_usage_by_handler` HashMap | VERIFIED | HashMap exists at line 252, populated during handler analysis, consumed in `replace_jsx_element_handlers` for q:p injection |
| `crates/qwik-optimizer-oxc/src/transform.rs` | `analyze_lambda_deep_ident_refs` + `walk_*_deep_idents` | VERIFIED | Three deep scanning functions implemented at lines 3709+, handling all expression and statement types including nested closures |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `transform.rs:ImportTracker.const_bindings` | `jsx_transform.rs:is_const_jsx_value` | `&tracker.const_bindings` argument | WIRED | Line 1541: `is_const_jsx_value(&value, &tracker.const_bindings)` |
| `transform.rs:ImportTracker.const_bindings` | `jsx_transform.rs:is_child_expression_immutable` | `&tracker.const_bindings` argument | WIRED | Line 2172: `is_child_expression_immutable(&other, module_imports, &tracker.const_bindings)` |
| `transform.rs:enter_variable_declaration` | `transform.rs:ImportTracker.const_bindings` | `collect_const_binding_names()` helper | WIRED | Lines 1659-1665: const declarations populate const_bindings |
| `transform.rs:iter_var_usage_by_handler` | `transform.rs:replace_jsx_element_handlers` | HashMap lookup by span | WIRED | Line 1302: `self.iter_var_usage_by_handler.get(&span_start)` |
| `transform.rs:replace_jsx_element_handlers` | q:p/q:ps injection | Direct JSX attribute creation | WIRED | Lines 1382-1433: q:p (single var) and q:ps (multiple vars) injected as JSX attrs |
| `jsx_transform.rs:has_qp` | `jsx_transform.rs:static_listeners` | Boolean check | WIRED | Line 1601-1603: `has_qp` clears `static_listeners` |
| `transform.rs:useResource$` | `_rawProps` param name | `is_props_rewrite_exit` guard | WIRED | Line 2286: guard covers `component$ \|\| useResource$` |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| Identifier scope analysis (const_bindings) | SATISFIED | — |
| Logical && mutability propagation | SATISFIED | — |
| Loop event handler static_listeners detection | PARTIALLY SATISFIED | 2/19 loop handler cases remain (for-of const var edge case → Phase 9) |
| q:p/q:ps injection for all 18 cases | SATISFIED | All 10 affected test files verified |
| useResource$ _rawProps override | SATISFIED | — |
| Flag mismatches ≤15 | NOT SATISFIED | 33 remain; all caused by upstream transform gaps (Phase 9) |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| No blockers found | — | — | — | — |

Phase 8 implementation is substantive: no placeholder/stub patterns detected. The remaining 33 flag mismatches are all correctly identified as upstream transform differences (prop classification, missing transforms), not flag computation bugs. The flag computation logic itself is correct and matches SWC semantics.

### Human Verification Required

None — all verification is automated via snapshot comparison.

## Gaps Summary

The only unmet criterion is the quantitative goal of ≤15 flag mismatches. The implementations are complete and correct; the gap is caused by **upstream transform features not yet implemented in OXC** that affect flag computation indirectly:

1. **_fnSignal dep constness** (~9 cases): SWC treats `_fnSignal` children as mutable when deps are non-const (let variables). OXC doesn't yet implement this check, causing `static_subtree` to be wrong for elements with `_fnSignal` children.

2. **Inline component _fnSignal wrapping** (3 cases, `destructure_args_inline_cmp_*`): OXC doesn't wrap non-`component$` exports with `_fnSignal`, causing these inline components to produce different prop/child structures.

3. **_noopQrl/dev mode transforms** (~6 cases): OXC doesn't implement `_noopQrlDEV`, dev mode metadata args (fileName, lineNumber), or noop QRL variants.

4. **signal.value reactive detection** (1 case): `signal.value` in const classification should be detected as reactive but isn't.

5. **Client code stripping** (1 case): `example_strip_client_code` — OXC doesn't implement this transform.

6. **_fnSignal placement for non-component$ contexts** (1 case): `should_wrap_object_with_fn_signal` — diff in where _fnSignal wrapping is placed.

These are all deferred to Phase 9 per the 08-03 audit decision. The flag computation itself is correct; fixing these requires implementing the upstream transforms.

**Phase 8 achieved a 69% reduction in flag mismatches (108 → 33). The goal of ≤15 requires Phase 9 upstream transform work.**

---

_Verified: 2026-02-21T12:22:39Z_
_Verifier: Claude (gsd-verifier)_
