---
phase: 09-jsx-keys-final-parity
plan: 04
subsystem: jsx-transform
tags: [jsx-keys, root-jsx-mode, import-assertions, hoist-strategy, text-normalization]

# Dependency graph
requires:
  - phase: 09-03
    provides: "Dev mode QRL emission, capture diagnostics"
  - phase: 05-01
    provides: "JSX key generation infrastructure"
provides:
  - "Hoist strategy named-const extraction for inlinedQrl callbacks"
  - "Correct root_jsx_mode save/restore matching SWC handle_jsx pattern"
  - "Conditional/logical expression root_jsx_mode=true hooks"
  - "Import assertion/attribute clause preservation (with { type: json })"
  - "JSX text normalization matching Babel/SWC cleanJSXElementLiteralChild"
affects: ["09-05"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "root_jsx_mode save/restore in enter/exit_jsx_element (matches SWC handle_jsx lines 877-919)"
    - "Conditional and logical expressions set root_jsx_mode=true (matches SWC fold_cond_expr/fold_bin_expr)"
    - "Import assertion carried through ImportInfo -> ReemittedImport -> SegmentImportEntry"

key-files:
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"
    - "crates/qwik-optimizer-oxc/src/code_move.rs"
    - "crates/qwik-optimizer-oxc/src/collector.rs"
    - "crates/qwik-optimizer-oxc/src/types.rs"

key-decisions:
  - "SWC handle_jsx saves root_jsx_mode before setting false for children, restores after -- OXC bottom-up needs equivalent save/restore in enter/exit_jsx_element"
  - "SWC assigns JSX keys bottom-up within a subtree (children get lower counter values, parent gets higher) -- OXC's bottom-up exit_expression ordering naturally matches this"
  - "SWC fold_cond_expr and fold_bin_expr set root_jsx_mode=true -- OXC needs enter_conditional_expression and enter_logical_expression hooks"
  - "Import assertions stored as Vec<(String, String)> key-value pairs on ImportInfo"

patterns-established:
  - "root_jsx_mode save/restore: enter hook saves and sets false, exit hook restores"
  - "Expression-level root_jsx_mode hooks: conditional/logical expressions set root_jsx_mode=true"
  - "Import assertion threading: ImportInfo -> ReemittedImport -> SegmentImportEntry -> format_with_clause"

# Metrics
duration: 23min
completed: 2026-02-21
---

# Phase 9 Plan 4: Hoist Strategy, Key Ordering, Import Assertions Summary

**Hoist strategy named-const extraction, root_jsx_mode save/restore for correct key counters, conditional/logical expression key hooks, import assertion preservation, JSX text normalization**

## Performance

- **Duration:** 23 min
- **Started:** 2026-02-21T20:13:25Z
- **Completed:** 2026-02-21T20:36:00Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- Hoist strategy extracts inlinedQrl callbacks to named const declarations matching SWC
- root_jsx_mode save/restore in enter/exit_jsx_element matches SWC's handle_jsx pattern (key counters now correct)
- Added enter/exit hooks for ConditionalExpression and LogicalExpression setting root_jsx_mode=true (elements inside ternary and && get keys)
- Import assertion/attribute clauses (with { type: "json" }) preserved through collection, capture analysis, and segment emission
- JSX text normalization updated to match Babel/SWC cleanJSXElementLiteralChild algorithm
- 3 new exact golden matches: example_import_assertion, example_jsx_keyed, example_jsx_keyed_dev
- 125 files differ (down from 128), 37 exact matches (up from 34)

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement Hoist strategy named-const extraction** - `57ffbe4` (feat)
2. **Task 2: Fix key counter ordering, import assertions, text normalization** - `d741648` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - root_jsx_mode save/restore in enter/exit_jsx_element/fragment, conditional/logical expression hooks, import assertion fields
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - JSX text normalization fix
- `crates/qwik-optimizer-oxc/src/code_move.rs` - Import assertion emission in segment code (with clause formatting)
- `crates/qwik-optimizer-oxc/src/collector.rs` - Import assertion collection from AST, threading to ReemittedImport
- `crates/qwik-optimizer-oxc/src/types.rs` - assertion field on ImportInfo struct

## Decisions Made

1. **SWC key assignment order is bottom-up (children before parent)**: Discovered that SWC's handle_jsx is called from fold_call_expr which processes children via handle_jsx_props_obj before assigning the parent's key. This means OXC's bottom-up exit_expression naturally produces the same counter ordering -- no pre-assignment was needed.

2. **root_jsx_mode must be saved/restored per JSX element, not just set to false**: SWC saves root_jsx_mode, sets false for children, processes, restores. In OXC's bottom-up traversal, this means enter_jsx_element saves and sets false, exit_jsx_element restores. Previously exit_expression set root_jsx_mode=false without restore, causing parent elements to lose their key assignment.

3. **Conditional and logical expressions need root_jsx_mode=true**: SWC's fold_cond_expr and fold_bin_expr both set root_jsx_mode=true. This ensures elements inside `cond ? <A/> : <B/>` and `cond && <A/>` patterns get auto-generated keys matching SWC.

4. **Import assertions threaded through 3-layer pipeline**: ImportInfo (collection) -> ReemittedImport (capture analysis) -> SegmentImportEntry (code emission). format_with_clause() formats the assertion as ` with { key: "value" }`.

## Deviations from Plan

None - plan executed as written. The root_jsx_mode fix required deeper investigation into SWC's save/restore pattern and key assignment order (bottom-up, not top-down as initially assumed in the plan), but the implementation matches the plan's objectives.

## Issues Encountered

1. **Initial misunderstanding of SWC key counter ordering**: Initially assumed SWC assigns keys top-down (source order). Attempted a pre-assignment approach during enter_jsx_element which produced incorrect counter values. After deeper analysis of SWC's handle_jsx flow (children processed via handle_jsx_props_obj before parent key assignment), discovered SWC actually assigns keys bottom-up. Reverted pre-assignment approach and used simple save/restore pattern instead.

2. **Total diff lines increased from ~3735 to ~4208**: The root_jsx_mode fix correctly assigns keys to more elements (those inside conditional/logical expressions and at function body root level). While this increases the number of key-related matches, it also reveals more differences in flags and other attributes that are counted as diff lines. The key values themselves are now more correct.

## Next Phase Readiness
- Key generation now matches SWC for conditional/logical expression contexts
- 37 exact matches (3 new: example_import_assertion, example_jsx_keyed, example_jsx_keyed_dev)
- 125 files still differ -- remaining issues are: _fnSignal wrapping, capture lists, _auto_ exports, prop classification, DCE, formatting

## Self-Check: PASSED

---
*Phase: 09-jsx-keys-final-parity*
*Completed: 2026-02-21*
