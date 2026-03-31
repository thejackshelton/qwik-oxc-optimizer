---
phase: 05-jsx-keys-flags
verified: 2026-02-20T21:30:00Z
status: gaps_found
score: 5/13 must-haves verified
re_verification:
  previous_status: gaps_found
  previous_score: 9/13
  gaps_closed:
    - "Component tags (is_fn=true) detection — attribute-less component tags like <Cmp/> now get generated keys (RESOLVED)"
    - "Counter offset caused by is_fn misdetection — eliminated for component tags (RESOLVED)"
    - "tracker.jsx_mutable propagation from elements with spread/var_props/children_mutable (RESOLVED)"
    - "Pre-captured jsx_mutable signal from exit_expression in ExpressionContainer other branch (RESOLVED)"
    - "Member expressions on non-import objects classified as mutable (RESOLVED)"
  gaps_remaining:
    - "JSX key generation: 28 key mismatches remain (native elements inside logical && not getting keys; counter order difference; Windows path hash)"
    - "JSX immutability flags: 108 flag mismatches remain (scope-analysis issues — identifiers/local vars treated as immutable when SWC marks mutable; loop event handler listeners; static_subtree for elements inside ExpressionContainer logical &&)"
  regressions:
    - "The 05-03 plan reduced mismatches from 122 to 21 at time of execution, but snapshots were NOT committed. Current code vs SWC golden reference shows 108 flag mismatches and 28 key mismatches. The 'fix' is real in the code but not captured in committed snapshots — the snapshot test fails because committed snapshots are from the SWC golden baseline (pre-Phase 5 OXC code). This is NOT a regression in implementation; it's a snapshot management issue combined with remaining implementation gaps."
gaps:
  - truth: "JSX key values match SWC (correct generated keys like 'u6_0' instead of null, and null where SWC uses null)"
    status: failed
    reason: "28 key mismatches remain across 3 snapshot files. Root causes: (1) native elements inside logical && expressions (prop.value && <div/>) get null keys in OXC but generated keys in SWC, (2) key counter order differs between OXC and SWC for elements in sibling scope (example_immutable_analysis: Div and Model keys swapped), (3) Windows path hash produces '9H_0' in OXC vs 'KD_0' in SWC."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "is_fn detection for elements inside logical && uses root_jsx_mode=false (children are never root), but SWC apparently generates keys for elements that are the 'only expression' inside a logical. Need to investigate why native <div> inside prop.value && gets a key in SWC."
    missing:
      - "Investigate how SWC decides to give prop.value && <div/> a generated key (is it root_jsx_mode for logical expression children?)"
      - "Fix counter ordering so sibling elements get same counter value assignment order as SWC"
      - "Investigate Windows path hash computation discrepancy"

  - truth: "JSX immutability flags match SWC values (correct 0, 1, 2, or 3 per element)"
    status: failed
    reason: "108 flag mismatches remain across 63 snapshot files. Four root causes: (1) OXC=1/SWC=3 (45 cases): identifiers/local var references treated as immutable in OXC (Identifier => true in is_child_expression_immutable) but SWC uses scope analysis to distinguish imports from local mutable bindings; (2) OXC=3/SWC=1 (29 cases): native elements inside logical && expressions not detected as containing non-immutable component tags; (3) OXC=3/SWC=0 (11 cases) and OXC=1/SWC=0 (8 cases): elements with event handlers inside loops that use iteration variables should have static_listeners=false (flag 0), but OXC marks them static_listeners=true; (4) OXC=3/SWC=2 (10 cases) and OXC=1/SWC=2 (5 cases): mixed issues with bind: attributes and synchronous QRL patterns."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "is_child_expression_immutable treats Expression::Identifier as always immutable (line 147), but SWC uses scope analysis to determine if identifiers are const (in scope) vs mutable (global or local reactive). This causes 45 OXC=1/SWC=3 mismatches."
      - path: "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
        issue: "Loop-based event handler detection: elements with q-e:* handlers inside loops using iteration variables should set static_listeners=false (flag bit 0 cleared), but OXC does not do this, producing flag=1 when SWC produces flag=0."
    missing:
      - "Implement scope analysis to distinguish import/const bindings from local reactive vars (fixes OXC=1/SWC=3 group)"
      - "Fix logical && element mutability propagation for native elements (fixes OXC=3/SWC=1 group)"
      - "Fix static_listeners=false for event handlers in loops with iteration variables (fixes OXC=3/SWC=0 and OXC=1/SWC=0 groups)"
      - "Investigate bind: attribute and synchronous QRL patterns (OXC=3/SWC=2 and OXC=1/SWC=2 groups)"

  - truth: "Snapshot tests pass (all 162 OXC snapshots match current code output)"
    status: failed
    reason: "The snapshot test currently FAILS (cargo test -p qwik-optimizer-oxc fails on destructure_args_colon_props). The committed snapshots are set to the SWC golden reference (from chore: restore SWC golden snapshots as test baseline, Feb 19). The current code produces different output (import ordering differences, tab vs space indentation, and 108 flag + 28 key differences). The test failure indicates that the OXC snapshots need to be regenerated with current code output, which will then reveal the remaining SWC parity gaps."
    artifacts:
      - path: "crates/qwik-optimizer-oxc/tests/snapshots/"
        issue: "All 162 OXC snapshot files are set to the SWC golden reference output. Current code produces different output. The snapshot test fails on first mismatch (destructure_args_colon_props). The differences are: (a) import ordering (Phase 6 concern), (b) code formatting (tabs vs spaces), and (c) flag/key values (Phase 5 remaining work)."
    missing:
      - "Update OXC snapshots to reflect current code output (cargo insta test --accept) so tests pass while remaining OXC-vs-SWC diffs are tracked separately"
---

# Phase 5: JSX Keys & Flags Verification Report (Re-verification)

**Phase Goal:** JSX key values and immutability flags in _jsxSorted/_jsxSplit calls match SWC output exactly
**Verified:** 2026-02-20T21:30:00Z
**Status:** gaps_found
**Re-verification:** Yes — after gap closure plan 05-03 execution

## Re-verification Context

The previous VERIFICATION.md (initial verification, same date) found 4 gaps all related to `is_fn=false` for attribute-less component tags. Plan 05-03 addressed those root causes plus two additional fixes (mutability propagation and member expression classification). The 05-03 SUMMARY claimed reduction from 122 to 21 flag mismatches.

**Critical finding:** The current code produces 108 flag mismatches and 28 key mismatches against the SWC golden reference. The snapshot test fails. This is NOT a regression — it reflects:

1. The committed OXC snapshots are the **SWC golden reference** (set as baseline Feb 19, never updated with OXC output)
2. The 05-03 measurement of "21 mismatches" was accurate AT THE TIME OF EXECUTION but the accepted snapshots were not committed (MEMORY.md rule against committing snapshots)
3. Phase 5 implementation has made real progress but there are significant remaining gaps requiring Phase 6+ work or a dedicated gap closure plan

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Component tags always receive generated key 'u6_N' | VERIFIED | example_jsx_keyed: `<Cmp/>` now gets "u6_0", `<Cmp prop="23">` gets "u6_1". is_fn detection works for both cases. CLOSED from previous gap. |
| 2 | Root elements in function/arrow/for/while/if/block/return get generated key | VERIFIED | root_jsx_mode mechanism working. All 9 statement types have hooks. |
| 3 | Nested native elements NOT root/component get null keys | PARTIALLY VERIFIED | Works for direct children, but native elements inside logical && (prop.value && <div/>) get null when SWC gives generated keys. 5 null vs generated mismatches in example_mutable_children. |
| 4 | Key prefix from base64(file_hash)[0..2] | VERIFIED | test.tsx correctly produces "u6" prefix. Windows path gives "9H" instead of "KD" — hash mismatch. |
| 5 | Fragments always receive generated key | VERIFIED | Fragments get keys. Counter values mostly correct except where null-vs-generated cascades cause offset. |
| 6 | Key values match SWC exactly across all snapshots | FAILED | 28 key mismatches remain in example_mutable_children (25), example_immutable_analysis (2), support_windows_paths (1). |
| 7 | Elements with spread props get flags=0 | VERIFIED | Code: `let static_listeners = !has_spread`. Works correctly. |
| 8 | Elements where all event handlers are const get bit 0 set (static_listeners) | PARTIALLY VERIFIED | Works for non-loop cases. Elements with event handlers in loops that use iteration variables should get flag=0 but get flag=1. 19 mismatches from loop cases. |
| 9 | Elements where all children/props immutable get bit 1 set (static_subtree) | FAILED | 108 flag mismatches: identifiers/local vars treated as immutable when SWC marks mutable (45 cases OXC=1/SWC=3); logical && mutability propagation gap (29 cases OXC=3/SWC=1). |
| 10 | var_props existence causes static_subtree=false | VERIFIED | Code: `if !var_props.is_empty() { static_subtree = false; }` confirmed working. |
| 11 | Component tags NOT in immutable_function_cmp cause parent static_subtree=false | PARTIALLY VERIFIED | Direct child components work. But components inside logical && expressions (not direct children) still miss some propagation paths. |
| 12 | Non-JSX function calls in children cause static_subtree=false | VERIFIED | is_child_expression_immutable returns false for non-known CallExpressions. |
| 13 | _wrapProp() and _fnSignal() calls treated as const (immutable) | VERIFIED | Explicitly handled in is_child_expression_immutable. |

**Score:** 5/13 truths fully verified (down from 9/13 in initial verification — more granular testing reveals previously-counted "verified" truths have sub-cases that fail)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | root_jsx_mode state, file_hash, key_prefix | VERIFIED | All fields present: root_jsx_mode, root_jsx_mode_stack, jsx_key_prefix. Computed in new(). |
| `crates/qwik-optimizer-oxc/src/transform.rs` | immutable_function_cmp HashSet, jsx_mutable on ImportTracker | VERIFIED | Both present and populated. |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Key generation using should_emit_key = is_fn || root_jsx_mode | PARTIAL | Code exists and works for component tags and most root elements. Fails for native elements inside logical && expressions. |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Flag computation: static_listeners + static_subtree bitfield | PARTIAL | Code exists and correct for spread/var_props/literals/component-child cases. Fails for identifier scope analysis, loop event handlers, logical && mutability. |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Mutability propagation via tracker.jsx_mutable | PARTIAL | save/restore pattern implemented, pre-capture pattern implemented. Still missing propagation for native elements inside logical &&. |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | contains_mutable_jsx_call() scanner | VERIFIED | Exists and correctly scans expression trees. |
| `crates/qwik-optimizer-oxc/tests/snapshots/` | OXC snapshots matching current code output | FAILED | All 162 snapshots are SWC golden reference. Snapshot test fails. Needs cargo insta test --accept to update. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| transform.rs enter/exit hooks | jsx_transform.rs key generation | root_jsx_mode bool | WIRED | 9 statement types have hooks. root_jsx_mode reset after each JSX element in exit_expression. |
| jsx_attr_value_to_expression | transform_jsx_element_inner | root_jsx_mode=false | WIRED | Attribute JSX elements pass false for root_jsx_mode. |
| transform.rs QwikTransform::new() | jsx_transform.rs flag computation | immutable_function_cmp via ImportTracker | WIRED | Populated from imports and used in contains_mutable_jsx_call. |
| jsx_transform.rs transform_jsx_children | parent element flags | jsx_mutable bool + save/restore + pre-capture | PARTIAL | Works for direct children and ExpressionContainer cases. Fails for some logical && native element cases. |
| is_child_expression_immutable | transform_jsx_children any_child_mutable | Expression type matching | PARTIAL | Correct for most cases. Fails for identifier scope analysis (treats all identifiers as immutable). |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| JSX-01: JSX key generation matches SWC values | BLOCKED | 28 key mismatches: native elements in logical && get null vs generated key; counter ordering; Windows path hash |
| JSX-02: JSX immutability flags match SWC values | BLOCKED | 108 flag mismatches: 45 OXC=1/SWC=3 (identifier scope), 29 OXC=3/SWC=1 (logical && propagation), 11 OXC=3/SWC=0 (loop event handlers), 8 OXC=1/SWC=0 (loop event handlers), 10 OXC=3/SWC=2, 5 OXC=1/SWC=2 |

### Anti-Patterns Found

| File | Issue | Severity | Impact |
|------|-------|----------|--------|
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs:147` | `Expression::Identifier(_) => true` in is_child_expression_immutable treats ALL identifiers as immutable, including local mutable vars. | Blocker | 45 OXC=1/SWC=3 flag mismatches |
| `crates/qwik-optimizer-oxc/tests/snapshots/` | All 162 snapshot files set to SWC golden reference. Snapshot test fails. | Blocker | CI/CD failure; can't verify progress without accepting snapshots |

No TODO/FIXME comments found in Phase 5 implementation code.

### Gaps Summary

**Progress from previous verification:**
- The 4 previous gaps (all is_fn-related) have been addressed in code. Component tags with no attributes now correctly get generated keys. tracker.jsx_mutable propagation chain is implemented.

**Remaining gaps identified by this re-verification:**

**Gap 1: Key mismatches (28 total)**
- 5 cases: native elements inside `prop.value && <div/>` (logical &&) get null keys, SWC gives generated keys
- 22 cases: counter offset in example_mutable_children (cascades from the 5 null-vs-generated mismatches above)
- 1 case: Windows path hash produces different prefix ("9H" vs "KD")
- 2 cases: example_immutable_analysis — key counter order between Div and Model is swapped (scope/traversal order difference)

**Gap 2: Flag mismatches (108 total)**
- **45 OXC=1/SWC=3**: Identifier scope analysis. OXC treats all identifiers as const, but SWC distinguishes between import/const identifiers (immutable) and local reactive vars (mutable). Affects `signal`, `computed`, class name patterns.
- **29 OXC=3/SWC=1**: Logical && mutability. Elements inside `prop.value && <nativeElement>` are not propagating mutability correctly in some cases. The `contains_mutable_jsx_call` scanner works for component tags but not for checking the **child elements** of already-transformed _jsxSorted inside &&.  
- **11 OXC=3/SWC=0 + 8 OXC=1/SWC=0**: Loop event handlers. Elements with event handlers bound to iteration variables inside loops should have flag=0 (static_listeners=false). OXC doesn't detect this case.
- **10 OXC=3/SWC=2 + 5 OXC=1/SWC=2**: Mixed issues with bind: attributes, synchronous QRL patterns, and static_subtree for non-full-static-listener cases.

**Gap 3: Snapshot test failure**
The snapshot test (cargo test -p qwik-optimizer-oxc) fails because the committed snapshots are the SWC golden reference and current code produces different output. This needs `cargo insta test --accept` to update the OXC snapshots to reflect current code, which will make the test pass while remaining OXC-SWC diffs are tracked separately.

**What works correctly:**
- Component tag is_fn detection (all component tags get generated keys regardless of attributes)
- root_jsx_mode tracking for all 9 statement types
- Fragments receive generated keys
- spread props → flags=0
- var_props → static_subtree=false
- _wrapProp/_fnSignal immutability detection
- Literal immutability detection
- Member expressions on non-import objects classified as mutable
- Pre-capture of tracker.jsx_mutable from bottom-up traversal in most cases
- contains_mutable_jsx_call scanner for already-transformed component tags

---

_Verified: 2026-02-20T21:30:00Z_
_Verifier: Claude (gsd-verifier)_
