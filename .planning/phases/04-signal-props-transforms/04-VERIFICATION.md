---
phase: 04-signal-props-transforms
verified: 2026-02-20T15:30:00Z
status: gaps_found
score: 4/5 must-haves verified
re_verification:
  previous_status: gaps_found
  previous_score: 3/5
  gaps_closed:
    - "Signal prop access generates _wrapProp(obj, 'prop') two-argument form for non-destructured (props) parameters -- transform is now correct"
  gaps_remaining:
    - "QRL hoisting deferred to Phase 6 (unchanged, architectural)"
    - "04-04 must-have 4: 4 affected test cases still differ from SWC golden (Phase 5/6 issues only, not Phase 4 transforms)"
  regressions: []
gaps:
  - truth: "QRL calls are hoisted to variable declarations and referenced by variable"
    status: failed
    reason: "Explicitly deferred to Phase 6. OXC Traverse architecture stores hoisted data as strings via hoisted_function_stmts which only injects at module top level via exit_program. QRL hoisting requires injecting into function-level blocks which the current mechanism does not support. Documented in 04-02-SUMMARY.md key-decisions."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/transform.rs"
        issue: "No QRL hoisting implementation. QRL calls remain inline at each usage site."
    missing:
      - "Phase 6 work: mechanism to hoist QRL calls to function-level const declarations above loop bodies"
      - "Integration point in exit_function / block-level traversal to inject hoisted QRL declarations"
  - truth: "04-04 must-have 4: 4 affected test cases produce output matching SWC golden snapshots"
    status: partial
    reason: "The props-wrapping transforms (_wrapProp, _restProps, _fnSignal) are correct in all 4 cases. Remaining diffs are exclusively Phase 5/6 issues: import leakage to main file, import ordering in segment files, children flags (3 vs 1), tab vs 2-space indentation, null vs string JSX keys. Total snapshot diff count is 160 (unchanged from before 04-04)."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/destructure_args_colon_props.snap"
        issue: "OXC output: correct _wrapProp(props, 'bind:value'), but import leakage (_jsxSorted/_wrapProp/_Fragment in test.js), import ordering, children flag 3 vs 1, tab indent"
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/destructure_args_colon_props2.snap"
        issue: "OXC output: correct useSignal(props['bind:value']) + _wrapProp(test), same Phase 5/6 issues"
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/destructure_args_colon_props3.snap"
        issue: "OXC output: correct _restProps(props, ['test']) + _fnSignal(_hf0, [props], _hf0_str) + p0.test.value, same Phase 5/6 issues + hoisted helpers also emitted in segment"
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/example_derived_signals_children.snap"
        issue: "OXC output: correct _wrapProp(props, 'data-nu') + _wrapProp(props, 'class'), but JSX keys (null vs string), inlined vs extracted segment format, formatting diffs"
    missing:
      - "Phase 5: fix children flags (3 vs 1), JSX key handling (null vs string), indentation"
      - "Phase 6: fix import ordering, import deduplication, prevent import leakage to main file"
human_verification: []
---

# Phase 4: Signal & Props Transforms Verification Report

**Phase Goal:** Signal reactivity wrappers, props destructuring, and QRL hoisting transforms produce output matching SWC exactly
**Verified:** 2026-02-20T15:30:00Z
**Status:** gaps_found
**Re-verification:** Yes -- after 04-04 gap closure

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Derived signals in JSX children wrapped with `_fnSignal(fn, [deps], "expression")` | VERIFIED | `build_fn_signal_wrapping` invoked in `transform_jsx_children`. OXC snapshots confirm `_fnSignal(_hf0, [signal], _hf0_str)` patterns. No regression from 04-04. |
| 2 | Signal prop access generates `_wrapProp(obj, "prop")` two-argument form | VERIFIED | **Gap 1 closed by 04-04.** `detect_signal_wrap` now handles `ComputedMemberExpression` (`props["bind:value"]`) and `StaticMemberExpression` on props param (`props.class`). Body destructuring (`const { "bind:value": bindValue } = props`) is detected by `detect_body_destructuring` and removed; aliases rewritten to `_wrapProp(props, "bind:value")` in JSX or `props["bind:value"]` in non-JSX. OXC output confirmed: `_wrapProp(props, "bind:value")` in destructure_args_colon_props, `_wrapProp(props, "class")` and `_wrapProp(props, "data-nu")` in example_derived_signals_children. |
| 3 | Inline component props destructured via `_restProps` matching SWC's pattern | VERIFIED | `analyze_props_destructuring` handles all patterns. `_restProps(_rawProps, ["count","some","hello","stuff","stuffDefault"])` confirmed. `detect_body_destructuring` adds body-level `_restProps(props, ["test"])` for non-destructured params with rest patterns (destructure_args_colon_props3). No regression from 04-04. |
| 4 | QRL calls hoisted to variable declarations | FAILED (DEFERRED) | Explicitly deferred to Phase 6. Architectural constraint: `hoisted_function_stmts` only injects at module top level via `exit_program`. QRL hoisting requires function-level block injection. |
| 5 | Transforms auto-add required imports (`_fnSignal`, `_wrapProp`, `_restProps`) | VERIFIED | `ImportTracker.needs_wrap_prop`, `needs_fn_signal`, `needs_rest_props` all conditionally set and emit `build_named_import` in `exit_program`. Confirmed in 04-04 outputs: `_wrapProp`, `_fnSignal`, `_restProps` imports appear in segment files when transforms fire. No regression. |

**Score:** 4/5 truths verified (criterion 4 is documented deferral to Phase 6)

**Note on 04-04 must-have 4:** The 4 affected test cases (`destructure_args_colon_props`, `destructure_args_colon_props2`, `destructure_args_colon_props3`, `example_derived_signals_children`) still differ from SWC golden (160 total snapshot diffs, unchanged). However the diffs are exclusively Phase 5/6 issues (import ordering, import leakage, children flags, indentation, JSX keys). The Phase 4 transform logic is correct and verified.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Extended `detect_signal_wrap` for ComputedMemberExpression + props param, `props_param_name` threaded through all JSX transform functions | VERIFIED | 2046 lines. `detect_signal_wrap` handles `ComputedMemberExpression` (props["X"]) and `StaticMemberExpression` on `props_param_name`. `props_param_name: Option<&str>` threaded through `transform_jsx_element_inner`, `transform_jsx_fragment_inner`, `transform_jsx_children`, `collect_reactive_deps`. |
| `crates/qwik-optimizer-oxc/src/transform.rs` | `detect_body_destructuring` called in enter_call_expression and exit_expression; reference rewriting; prop alias binding stripping | VERIFIED | 3343 lines. `detect_body_destructuring` called at lines 1325 and 1953. Body destructuring removal at line 1960. `_restProps` insertion at lines 1964-1974. `rewrite_body_destr_references` at line 1991. `strip_prop_alias_bindings` at line 2007. `props_param_name` extracted and passed to JSX transforms (lines 1772-1796). |
| `crates/qwik-optimizer-oxc/src/props_destructuring.rs` | `props_param_name: Option<String>` field on `PropsDestructuringInfo`; `BodyDestructuringInfo` struct; `detect_body_destructuring` function | VERIFIED | 940 lines. `props_param_name` field at line 46. `BodyDestructuringInfo` struct at line 66. `detect_body_destructuring` function at line 881. `BindingIdentifier` case sets `props_param_name` at line 163. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `detect_signal_wrap` | `_wrapProp(props, "key")` | `ComputedMemberExpression` arm + `props_param_name` check | WIRED | Lines 176-194 in jsx_transform.rs |
| `detect_signal_wrap` | `_wrapProp(props, "class")` | `StaticMemberExpression` + `props_param_name` check | WIRED | Lines 165-170 in jsx_transform.rs |
| `transform.rs enter_call_expression` | `detect_body_destructuring` | Called when `info.props_param_name` is Some | WIRED | Lines 1321-1328 in transform.rs |
| `transform.rs exit_expression` | Body destructuring removal + rewrite | Lines 1949-2009 | WIRED | Statement removal at 1960, _restProps at 1968-1974, rewrite at 1991, binding strip at 2007 |
| `props_param_name` extraction | JSX transform functions | Lines 1772-1796 in transform.rs | WIRED | `props_param_ref` passed to `transform_jsx_element_inner` and `transform_jsx_fragment_inner` |
| `collect_reactive_deps` | prop alias origin substitution | `prop_origin_map` lookup | WIRED | When `test.value` seen and `test` is prop alias, dep becomes `props`, hoisted fn becomes `(p0) => p0.test.value` |
| `WrapPropNamed` source extraction | `ComputedMemberExpression` object | At attribute handler + children handler | WIRED | Both sites handle `StaticMemberExpression` and `ComputedMemberExpression` |
| `ImportTracker.needs_wrap_prop` | `build_named_import("_wrapProp")` | exit_program (transform.rs line 2325) | WIRED | Auto-import fires when any `_wrapProp` call is generated |
| `ImportTracker.needs_fn_signal` | `build_named_import("_fnSignal")` | exit_program (transform.rs line 2329) | WIRED | Auto-import fires when any `_fnSignal` call is generated |
| `ImportTracker.needs_rest_props` | `build_named_import("_restProps")` | exit_program (transform.rs line 2290) | WIRED | Auto-import fires when any `_restProps` call is generated |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| SIG-01: _fnSignal wrapping for derived signals in JSX | SATISFIED | -- |
| SIG-02: _wrapProp two-argument form (all patterns) | SATISFIED | Gap 1 closed: non-destructured (props) params now handled |
| PROP-01: _restProps transform matching SWC | SATISFIED | Default values, skip cases, excluded_keys all correct; body-level rest also correct |
| QRL-01: QRL hoisting | BLOCKED (DEFERRED) | Deferred to Phase 6 per architectural constraint |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| All 4 affected snapshots | -- | 160 total snapshot diffs (Phase 5/6 issues) | Warning (not Phase 4) | Import ordering, children flags, formatting -- Phase 5/6 scope |
| None | -- | No new regressions from 04-04 | -- | Snapshot diff count unchanged at 160 |

Build verified: `cargo build -p qwik-optimizer-oxc` compiles cleanly (Finished dev profile in 0.18s).

`cargo insta test -p qwik-optimizer-oxc` runs without panics. 160 snapshot diffs (unchanged from before 04-04, confirming no regressions).

### Human Verification Required

None -- all items verifiable programmatically.

### Gaps Summary

**Gap 1 (criterion 2) -- CLOSED:** The `detect_signal_wrap` function and surrounding infrastructure now correctly handles all three non-destructured props patterns:
- `props["bind:value"]` (ComputedMemberExpression) -> `_wrapProp(props, "bind:value")`
- `props.class` (StaticMemberExpression on props param) -> `_wrapProp(props, "class")`
- `const { "bind:value": bindValue } = props` (body destructuring) -> statement removed, `_wrapProp(props, "bind:value")` in JSX, `props["bind:value"]` in non-JSX, `_restProps` for rest patterns

All 4 affected test cases produce correct transform output. Remaining diffs from SWC golden are Phase 5/6 issues:
- Phase 5: children flags (OXC emits 3, SWC emits 1 for some cases), JSX key handling (null vs string), indentation (tab vs 2-space)
- Phase 6: import ordering (OXC emits in add-order, SWC has specific ordering), import leakage to main file, import deduplication

**Gap 2 (criterion 4) -- UNCHANGED (DEFERRED):** QRL hoisting remains deferred to Phase 6. The `hoisted_function_stmts` mechanism only supports module-level injection via `exit_program`. Function-level QRL hoisting requires a different approach. This is a known architectural constraint with loop tracking infrastructure already in place.

**04-04 must-have 4 partial status:** The plan stated "produce output matching SWC golden snapshots" for the 4 affected cases. At the transform level this is true. At the full snapshot level, 160 diffs remain due to Phase 5/6 issues. This is expected scope for Phase 5/6, not a Phase 4 gap.

---

_Verified: 2026-02-20T15:30:00Z_
_Verifier: Claude (gsd-verifier)_
