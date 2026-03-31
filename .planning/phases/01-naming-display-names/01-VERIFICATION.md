---
phase: 01-naming-display-names
verified: 2026-02-19T23:45:00Z
status: passed
score: 4/4 must-haves verified
notes: |
  162/162 tests pass against insta snapshots.
  160/162 tests have perfect naming parity with SWC golden reference.
  2 failures (example_qwik_react, relative_paths) are multi-file input handling gaps
  deferred to Phase 3 (BUG-04) — not naming issues.
  3 tests have ordering-only diffs (identical name sets, different traversal order),
  also deferred (Phase 3 scope).
  REQUIREMENTS.md NOTE: NAME-01 description is misleading — it says "div_onClick not
  div_q_e_click" but both SWC and OXC correctly use q_e_click for $-suffixed native
  events. The actual fix was that bare $() in non-$-suffixed attrs now correctly uses
  the original attribute name (e.g. onClick, not q_e_click).
---

# Phase 1: Naming & Display Names Verification Report

**Phase Goal:** Segment names, display names, and parent fields match SWC output exactly, clearing the most pervasive diff noise across the entire snapshot suite
**Verified:** 2026-02-19T23:45:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Event handler segments use correct names matching SWC (q_e_click for $-suffixed native, onClick for non-$-suffix) | VERIFIED | 160/162 name fields match SWC; example_1 shows `renderHeader_div_onClick_XXXXXXXXXXXX` matching SWC exactly; should_convert_jsx_events shows all 8 segments with correct q_e/q_d/q_w prefixes |
| 2 | Display names include all intermediate scope elements (no dropped parent elements) | VERIFIED | example_3 shows `test.tsx_App_Header_component` (not dropped `App`); example_component_with_event_listeners_inside_loop shows `App_component_loopArrowFn_span_q_e_click` with full scope chain |
| 3 | Parent field uses segment name with hash format (e.g. `renderHeader_XXXXXXXXXXXX`) | VERIFIED | example_1 parent=`renderHeader_XXXXXXXXXXXX`; example_3 parent=`App_Header_component_XXXXXXXXXXXX`; all 160/162 parent fields match SWC's hash format |
| 4 | Segment filenames derived from corrected names match SWC filenames | VERIFIED | 160/162 canonicalFilename fields match SWC exactly; example_prod_node, example_default_export, should_convert_jsx_events all match |

**Score:** 4/4 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/qwik-optimizer-oxc/src/transform.rs` | stack_ctxt, segment_stack, escape_sym, register_context_name, jsx_event_to_html_attribute | VERIFIED | 2647 lines; all 5 key functions/fields present and substantive |
| `crates/qwik-optimizer-oxc/src/lib.rs` | rel_dir(), segment path construction | VERIFIED | Modified to include rel_dir() and correct path construction |
| `crates/qwik-optimizer-oxc/src/collector.rs` | Internal-only documentation | VERIFIED | Documentation added; display_name functions kept for internal use |
| `crates/qwik-optimizer-oxc/src/jsx_transform.rs` | Dead code removal (transform_attr_name_for_display) | VERIFIED | Dead function removed |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `enter_variable_declarator` | `stack_ctxt` | push variable name on enter | WIRED | Lines 1275-1279 in transform.rs |
| `exit_variable_declarator` | `stack_ctxt` | truncate on exit | WIRED | Lines 1286-1289 in transform.rs |
| `enter_jsx_element` | `stack_ctxt` | push element name | WIRED | Lines 768-831 in transform.rs |
| `exit_jsx_element` | `stack_ctxt` | pop element name | WIRED | Lines 985-987 in transform.rs |
| `record_segment` | `escape_sym` | `escape_sym(stack_ctxt.join("_"))` | WIRED | Line 550 in transform.rs |
| `record_segment` | `segment_stack` | `parent = segment_stack.last()` | WIRED | Lines 611-612, 674-675 in transform.rs |
| `jsx_event_to_html_attribute` | `stack_ctxt` push in JSX attr handling | via attr_ctx_name | WIRED | Lines 812, 822 in transform.rs |

### Requirements Coverage

| Requirement | Status | Blocking Issue |
|-------------|--------|----------------|
| NAME-01 (event handler names match SWC) | SATISFIED | 160/162 tests pass; 2 failures are multi-file scope (Phase 3). Note: REQUIREMENTS.md description is misleading — SWC uses q_e_click for $-suffixed events, not onClick |
| NAME-02 (display names include all intermediate elements) | SATISFIED | 160/162 tests pass; all intermediate scope elements present in display names |
| NAME-03 (parent field uses segment name + hash format) | SATISFIED | 160/162 tests pass; all parent fields use `segname_XXXXXXXXXXXX` format |

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `transform.rs:1498,1499,1515,1516,1542,1779,1781` | `placeholder` variable | Info | AST manipulation technique (null literal swapping), not stub code |

No blockers or warnings found.

### Human Verification Required

None. All success criteria are verifiable programmatically via snapshot comparison.

### Gaps Summary

No gaps blocking phase goal achievement.

**Not-naming issues excluded from phase scope (not gaps):**

1. `example_qwik_react` — Missing segments from multi-file input (2 segments produced by SWC from a dependent file not produced by OXC). Deferred to Phase 3 (BUG-04).
2. `relative_paths` — Multi-file input segments not processed from secondary file. Deferred to Phase 3 (BUG-04). Additionally uses `main_component` instead of `Local_component` (wrong component name from multi-file dep) — also Phase 3 scope.
3. `example_functional_component_2`, `should_not_transform_events_on_non_elements`, `should_transform_nested_loops` — Ordering-only diffs. Identical segment name sets, different traversal order. Deferred to Phase 3.

**All 4 success criteria from ROADMAP.md are met:**
1. Event handler segments use correct names matching SWC — VERIFIED (160/162)
2. Display names include all intermediate scope elements — VERIFIED (160/162)
3. Parent field uses segment name with hash format — VERIFIED (160/162)
4. Segment filenames match SWC filenames — VERIFIED (160/162)

The 2/162 failures are multi-file input handling bugs in Phase 3 scope, not naming issues.

---
*Verified: 2026-02-19T23:45:00Z*
*Verifier: Claude (gsd-verifier)*
