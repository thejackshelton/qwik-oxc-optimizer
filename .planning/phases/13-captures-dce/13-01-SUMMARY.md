---
phase: 13-captures-dce
plan: 01
subsystem: transform
tags: [captures, props-destructuring, const-literal-inlining, segment-body-codes]

# Dependency graph
requires:
  - phase: 10-captures-mechanism
    provides: "Basic capture analysis with compute_captures(), capture_stack, body_local_decls"
  - phase: 12-signal-wrapping-gaps
    provides: "Inline component _rawProps rewrite, active_props_info, segment body code serialization"
provides:
  - "Nested function/arrow param filtering from captures (prevents param leaks)"
  - "Post-reclassification segment body code rewriting for prop aliases"
  - "QRL capture array rebuilding after reclassification"
  - "Const-literal binding inlining in child segment body codes"
  - "Const-literal filtering from captures and reemitted_imports"
affects: [13-02, 13-03, 13-04]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "const_literal_bindings HashMap for tracking const declarations with literal initializers"
    - "Nested param collection in enter_function/enter_arrow_function_expression"
    - "fix_qrl_captures_in_body for post-reclassification QRL capture array rebuilding"
    - "get_literal_string() helper for extracting string representations from Expression literals"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/props_destructuring.rs"

key-decisions:
  - "Plan premise was wrong: SWC captures _rawProps (not individual props). Existing reclassification direction was correct."
  - "Nested function/arrow params tracked as body_local_decls in enter hooks (prevents capture leaks like aaa)"
  - "Segment body codes post-processed after reclassification to replace prop aliases with _rawProps.propName"
  - "QRL capture arrays rebuilt in component body via fix_qrl_captures_in_body after reclassification"
  - "ArrayExpression elements handled via index-based iteration replacing Identifier with MemberExpression"
  - "Const literal inlining only in child segments (not parent component body) to match SWC behavior"
  - "Const literal filtering also applies to reemitted_imports when local const shadows module import"

patterns-established:
  - "const_literal_bindings: Tracks const declarations with NumericLiteral/StringLiteral/BooleanLiteral initializers for inlining"
  - "fix_qrl_captures_in_body: Walks component body AST to find QRL calls matching child segments by name, rebuilds capture arrays"

# Metrics
duration: ~45min
completed: 2026-02-24
---

# Phase 13 Plan 01: Captures Reclassification and Const-Literal Inlining Summary

**Fixed nested param capture leaks, props reclassification body code rewriting, QRL capture array rebuilding, and const-literal inlining for child segment bodies**

## Performance

- **Duration:** ~45 min
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Nested function/arrow parameters no longer leak into captures (e.g., `aaa` from `({ aaa }) => aaa`)
- Segment body codes properly rewritten after props reclassification (_rawProps.propName)
- QRL capture arrays in component body rebuilt to match reclassified capture_names
- Const-literal bindings (e.g., `const STEP_2 = 2`) inlined in child segment bodies and removed from captures
- ArrayExpression elements in props_destructuring now properly rewritten
- example_functional_component_2: STEP_2 no longer captured, handler body has inlined value `2`
- example_multi_capture: `arg0` no longer captured, child body has inlined `20`; `aaa` no longer leaked
- example_qwik_conflict: `const qrl = 23` inlined as `23`, spurious re-import removed
- No regressions: 63 exact matches maintained

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix props capture reclassification for child segments** - `84faa84` (feat)
2. **Task 2: Inline const-literal bindings and filter from captures** - `e0507e6` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added nested param collection, segment body code post-processing, QRL capture rebuilding, const_literal_bindings tracking, and get_literal_string helper
- `crates/qwik-optimizer-oxc/src/props_destructuring.rs` - Fixed ArrayExpression element handling in rewrite_props_references

## Decisions Made

1. **Plan premise correction**: The plan stated SWC captures individual prop names instead of _rawProps. After examining the SWC golden snapshots (commit e230d3e), SWC actually captures _rawProps and uses _rawProps.foo in body code. The existing reclassification direction (prop aliases -> _rawProps) was correct. Adapted implementation to fix the actual issues (body code rewriting, QRL fixup, nested param leaks) instead of reversing direction.

2. **Nested param collection in enter hooks**: Instead of modifying compute_captures, nested function/arrow parameters are collected into body_local_decls in the capture stack during `enter_function` and `enter_arrow_function_expression`. This naturally filters them from captures.

3. **Const literal inlining scope**: Only inline in child segments (not parent component body). Parent body keeps `const STEP_2 = 2;` declaration -- removing it requires DCE which is out of scope. Module-level consts (like `export const STEP = 1`) are NOT inlined (they get re-imported via reclassify_module_level_decl_captures).

4. **Const literal reemitted_imports filtering**: When a local const literal shadows a module-level import (e.g., `const qrl = 23` shadowing `import { qrl } from "..."` ), the re-import is also filtered out since the segment body uses the inlined value, not the import.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Plan premise was incorrect about SWC behavior**
- **Found during:** Task 1 (investigation)
- **Issue:** Plan said "SWC captures individual prop names (foo, bar) instead of _rawProps" but SWC actually captures _rawProps
- **Fix:** Implemented the correct fixes: body code rewriting, QRL capture array rebuilding, nested param filtering
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Committed in:** 84faa84

**2. [Rule 1 - Bug] ArrayExpression elements not rewritten in props_destructuring**
- **Found during:** Task 1 (testing)
- **Issue:** `array_element_as_expression_mut` always returned None, so prop aliases in QRL capture arrays were never rewritten
- **Fix:** Replaced with index-based iteration handling ArrayExpressionElement::Identifier specifically
- **Files modified:** crates/qwik-optimizer-oxc/src/props_destructuring.rs
- **Committed in:** 84faa84

**3. [Rule 2 - Missing Critical] Const literal reemitted_imports shadowing**
- **Found during:** Task 2 (testing example_qwik_conflict)
- **Issue:** When `const qrl = 23` shadows `import { qrl } from "..."`, the re-import was still being added to the handler segment
- **Fix:** Added filtering of reemitted_imports against const_literal_bindings
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Committed in:** e0507e6

---

**Total deviations:** 3 auto-fixed (2 bugs, 1 missing critical)
**Impact on plan:** Plan premise correction was the major deviation. The actual implementation achieved the same goals (correct capture behavior) via a different mechanism than planned.

## Issues Encountered
- `func.params` is not an Option in OXC (it's a direct `Box<FormalParameters>`) -- removed `if let Some()` wrapper
- `expression_array` in OXC only takes 2 arguments (SPAN, elements), not 3 -- removed trailing None argument
- `argument_as_expression_mut` was initially removed as dead code but was still referenced in NewExpression handling -- kept it

## Next Phase Readiness
- Capture analysis for props destructuring and const literals is now correct
- Component body still retains const literal declarations (DCE deferred)
- 63 exact matches, 99 diffs maintained (no regressions)
- Ready for 13-02 (further capture/DCE improvements)

## Self-Check: PASSED

---
*Phase: 13-captures-dce*
*Completed: 2026-02-24*
