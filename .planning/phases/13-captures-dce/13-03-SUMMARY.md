---
phase: 13-captures-dce
plan: 03
subsystem: transform
tags: [dce, dead-code-elimination, segment-body, minify, oxc-parser]

# Dependency graph
requires:
  - phase: 13-01
    provides: "Segment body codes with capture reclassification and const literal inlining"
  - phase: 13-02
    provides: "Scope-aware capture filtering and C03 diagnostics"
provides:
  - "Segment body DCE: unused declaration stripping, if(false) elimination, invalid_decl removal"
  - "Post-DCE import filtering for segment modules"
  - "Parse-transform-reserialize DCE pipeline via apply_segment_body_dce()"
affects: ["13-04", "future signal wrapping phases"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Parse-transform-reserialize for body code string post-processing"
    - "Reference collection via recursive AST walking including JSX"
    - "force_remove_names parameter for C02 invalid declaration removal"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/lib.rs"

key-decisions:
  - "Combined Task 1 (DCE) and Task 2 (invalid_decl removal) into single commit since force_remove_names is integral to apply_segment_body_dce signature"
  - "Conservative destructuring DCE: keep destructuring patterns when init is not a simple identifier (safer than SWC which removes them)"
  - "example_props_optimization constant-folding sub-issue documented as pre-existing signal wrapping difference, not DCE"
  - "JSX reference collection uses as_expression() for JSXExpression (OXC inherit_variants macro flattens Expression into JSXExpression)"

patterns-established:
  - "apply_segment_body_dce: Parse body code as 'var __body__ = <arrow>', DCE the body statements, reserialize"
  - "collect_all_references_in_stmts: Comprehensive reference collector including JSX elements, fragments, and children"
  - "Post-DCE import filtering: retain only imports whose specifiers appear in DCE'd body code"

# Metrics
duration: 12min
completed: 2026-02-24
---

# Phase 13 Plan 03: Segment Body DCE Summary

**Parse-transform-reserialize DCE pipeline strips unused declarations, eliminates if(false) branches, and removes C02 invalid function/class declarations from segment body code strings**

## Performance

- **Duration:** ~12 min
- **Started:** 2026-02-24T09:24:24Z
- **Completed:** 2026-02-24T09:36:09Z
- **Tasks:** 2 (combined into 1 commit)
- **Files modified:** 2

## Accomplishments
- Unused const/let/function/class declarations stripped from segment bodies (example_9, example_10, example_8)
- if(false) branches eliminated from segment bodies (example_dead_code)
- C02 invalid function/class declarations force-removed from component body output (example_capturing_fn_class)
- Post-DCE import filtering removes unused segment imports
- JSX reference collection prevents false positives (example_multi_capture regression fixed)
- 5 of 6 targeted DCE tests show reduced diffs matching SWC behavior

## Task Commits

Each task was committed atomically:

1. **Task 1+2: Segment body DCE and invalid_decl removal** - `ab15859` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added ~1070 lines: apply_segment_body_dce(), apply_dce_to_statements(), reference collector, JSX ref walking, DceAction enum, helper functions
- `crates/qwik-optimizer-oxc/src/lib.rs` - Added DCE call loop on body_codes, post-DCE import filtering

## Decisions Made
- **Combined Task 1+2**: The force_remove_names parameter is integral to the apply_segment_body_dce function design, making Task 2's implementation inseparable from Task 1's code. Single commit with both tasks documented.
- **Conservative destructuring DCE**: Keep `const {a, b} = expr;` when init is not a simple identifier. SWC removes these, but our approach is safer (avoids dropping bindings that might be used). Known minor diff in example_8.
- **example_props_optimization**: The constant-folding sub-issue (`_hf0_str` having `1+2` instead of `3`) is a pre-existing signal wrapping difference. SWC doesn't use `_fnSignal`/`_hf` for this test at all, so the `_hf_str` format difference is moot. Documented, not fixed.
- **JSXExpression API**: Used `container.expression.as_expression()` instead of `JSXExpression::Expression(expr)` because OXC's `inherit_variants!` macro flattens Expression variants into JSXExpression.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] JSX expression reference collection missing**
- **Found during:** Task 1 (reference collector implementation)
- **Issue:** example_multi_capture regression: `const fn = ({aaa}) => aaa;` incorrectly removed because JSX expression containers and children were not scanned for references
- **Fix:** Added collect_refs_in_jsx_element() and collect_refs_in_jsx_child() functions with JSXExpression.as_expression() API
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** example_multi_capture snapshot preserves `const fn` declaration
- **Committed in:** ab15859

**2. [Rule 1 - Bug] JSXExpression::Expression compilation error**
- **Found during:** Task 1 (JSX reference collection)
- **Issue:** OXC's `inherit_variants!` macro flattens Expression variants into JSXExpression enum, so `JSXExpression::Expression` doesn't exist
- **Fix:** Used `container.expression.as_expression()` method instead of pattern matching
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** Clean compilation
- **Committed in:** ab15859

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Both fixes necessary for correctness. No scope creep.

## Issues Encountered
- Multiple compilation errors during development (BindingPattern API, init_expr mutability, allocator parameter threading, ChainElement exhaustiveness, UpdateExpression.argument type, JSXExpression variant) -- all resolved iteratively
- Stash contamination during regression testing caused false alarm about regressions in example_component_with_event_listeners_inside_loop -- verified no actual regressions after cleanup

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- DCE foundation complete for segment body code
- Plan 13-04 can proceed with remaining captures/DCE work
- Known remaining diffs: example_8 destructuring conservative behavior, example_10 comma expression format, example_props_optimization signal wrapping differences

---
*Phase: 13-captures-dce*
*Completed: 2026-02-24*

## Self-Check: PASSED
