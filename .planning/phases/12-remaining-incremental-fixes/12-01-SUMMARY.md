---
phase: 12-signal-wrapping-gaps
plan: 01
subsystem: transform
tags: [fnSignal, reactive-deps, signal-wrapping, oxc, jsx-transform]

# Dependency graph
requires:
  - phase: 04-signal-props-transforms
    provides: "Initial _fnSignal wrapping and collect_reactive_deps"
  - phase: 09-jsx-keys-final-parity
    provides: "_fnSignal dep constness, _jsxSplit, TS type assertion lookahead"
provides:
  - "Alphabetically sorted deps in _fnSignal calls matching SWC"
  - "Expression type coverage in collect_reactive_deps_inner (10+ types)"
  - "Harmless globals (undefined, NaN, Infinity) pass-through for wrapping"
  - "Props path accept_call_expr=true equivalent"
  - "CallExpression side-effect detection vs ChainExpression pass-through"
affects: [12-02, 12-03, 13-captures-dce, 14-final-parity]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Harmless global classification for wrapping eligibility"
    - "Direct CallExpression = side effect, ChainExpression call = not side effect"
    - "Post-merge alphabetical sort + param renumbering for dep arrays"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "Direct CallExpression sets has_non_reactive_non_const (prevents signal.value() wrapping) but ChainExpression calls do not (allows signal.formData?.get() wrapping) -- matches SWC contains_side_effect behavior"
  - "LogicalExpression added to both collect_reactive_deps_inner and contains_function_call despite not being in plan (was missing)"
  - "TaggedTemplateExpression unconditionally marks has_non_reactive in dep collection AND added to contains_function_call for children path"

patterns-established:
  - "Harmless globals: matches!(name, 'undefined' | 'NaN' | 'Infinity') skip has_non_reactive flag"
  - "Props vs children wrapping: props path removes contains_function_call gate; children path retains it"

# Metrics
duration: 10min
completed: 2026-02-23
---

# Phase 12 Plan 01: Core Dep Collection Fixes Summary

**Alphabetical dep sorting, 10+ missing expression type recursions, harmless globals pass-through, and props accept_call_expr=true in collect_reactive_deps**

## Performance

- **Duration:** 10 min
- **Started:** 2026-02-23T12:12:11Z
- **Completed:** 2026-02-23T12:21:59Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- _fnSignal dependency arrays now sorted alphabetically by root_name, matching SWC's compute_scoped_idents sort
- Added 10+ expression type arms to collect_reactive_deps_inner: TemplateLiteral, TaggedTemplateExpression, ArrayExpression, ComputedMemberExpression, CallExpression, ChainExpression, TSAsExpression, TSSatisfiesExpression, TSNonNullExpression, LogicalExpression
- Harmless globals (undefined, NaN, Infinity) no longer block _fnSignal wrapping -- ternary_prop test now matches SWC exactly on wrapping
- Props path allows function calls (accept_call_expr=true equivalent) -- signal.formData?.get("username") now wrapped via _fnSignal
- Direct CallExpression correctly marks side-effect to prevent wrapping signal.value() while allowing ChainExpression calls

## Task Commits

Each task was committed atomically:

1. **Task 1: Add dep sorting and missing expression type recursion** - `a4bbb64` (feat)
2. **Task 2: Fix harmless globals and accept_call_expr for props** - `395170d` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - collect_reactive_deps sorting, collect_reactive_deps_inner expression type arms, harmless globals, props path gate removal, contains_function_call extensions

## Decisions Made
- **Direct CallExpression = side effect:** Setting `has_non_reactive_non_const = true` for `Expression::CallExpression` prevents wrapping `signal.value()` (matching SWC's `contains_side_effect`), while `ChainExpression` with inner calls does NOT set the flag, allowing `signal.formData?.get("username")` to be wrapped. This matches the SWC behavioral split.
- **LogicalExpression added:** Both `collect_reactive_deps_inner` and `contains_function_call` were missing `LogicalExpression` handling. Added as deviation Rule 2 (missing critical functionality).
- **TaggedTemplateExpression dual handling:** In dep collection, unconditionally sets `has_non_reactive_non_const = true` (prevents wrapping). Also added to `contains_function_call` for the children path, ensuring children with tagged templates skip wrapping in both code paths.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Added LogicalExpression to collect_reactive_deps_inner and contains_function_call**
- **Found during:** Task 1 (expression type recursion)
- **Issue:** LogicalExpression (e.g., `a && b`, `a || b`) was falling through to `_ => {}` catch-all, silently ignoring reactive deps inside logical expressions
- **Fix:** Added LogicalExpression arm that recurses into left and right operands
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** Tests compile and run without panics
- **Committed in:** a4bbb64 (Task 1 commit)

**2. [Rule 1 - Bug] Direct CallExpression side-effect marking**
- **Found during:** Task 2 (accept_call_expr for props)
- **Issue:** Removing contains_function_call gate for props caused signal.value() to be incorrectly wrapped with _fnSignal. SWC's contains_side_effect returns true for direct CallExpression but not ChainExpression calls.
- **Fix:** Added `*has_non_reactive_non_const = true` to the CallExpression arm in collect_reactive_deps_inner. ChainExpression calls do not set this flag.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** example_derived_signals_cmp noInline props correctly NOT wrapped; example_getter_generation optional chain call correctly wrapped
- **Committed in:** 395170d (Task 2 commit)

**3. [Rule 2 - Missing Critical] Added ChainExpression, TemplateLiteral, LogicalExpression to contains_function_call**
- **Found during:** Task 2 (contains_function_call extensions)
- **Issue:** contains_function_call was missing these expression types, causing incorrect classification when used in children path
- **Fix:** Added three additional arms to contains_function_call
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Committed in:** 395170d (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (1 bug, 2 missing critical)
**Impact on plan:** All auto-fixes necessary for correctness. The CallExpression side-effect marking was essential to avoid a regression when removing the contains_function_call gate.

## Issues Encountered
- Store depth detection: `panelStore.active` (depth 1) not recognized as a store pattern by `has_chain_depth(expr, 2)`. This is a pre-existing issue where depth >= 2 check is too strict for single-level store access. Not in scope for this plan.
- Constant folding: `"true" + 1 ? "true" : ""` not folded to `"true"` by OXC (SWC does fold it). Pre-existing OXC limitation.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- collect_reactive_deps now has comprehensive expression type coverage and correct sorting
- Ready for 12-02 (_hf dedup and additional signal wrapping fixes)
- Pre-existing gaps remain: store depth detection, constant folding, dep classification for imports used in expressions

## Self-Check: PASSED

---
*Phase: 12-signal-wrapping-gaps*
*Completed: 2026-02-23*
