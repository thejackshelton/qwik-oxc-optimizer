---
phase: 04-signal-props-transforms
plan: 04
subsystem: transform
tags: [oxc, wrapProp, props-destructuring, signal-wrapping, fnSignal, restProps, jsx-transform]

# Dependency graph
requires:
  - phase: 04-signal-props-transforms (04-01)
    provides: "_wrapProp named wrapping, _fnSignal children wrapping, detect_signal_wrap infrastructure"
  - phase: 04-signal-props-transforms (04-03)
    provides: "Props destructuring analysis, PropsDestructuringInfo struct, rewrite_props_references"
provides:
  - "Non-destructured (props) parameter detection and body destructuring handling"
  - "_wrapProp(props, 'key') for direct ComputedMemberExpression and StaticMemberExpression on props param"
  - "Body destructuring statement removal with _restProps insertion for rest patterns"
  - "Prop alias origin substitution in _fnSignal dep collection (test.value -> [props] dep with p0.test.value hoisted fn)"
  - "Fixed argument_to_expression missing MemberExpression variants (pre-existing bug)"
affects: ["05-jsx-keys-flags", "06-import-ordering"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Body destructuring detection: scan arrow body for const { ... } = propsParam pattern"
    - "Props param name threading: passed through all JSX transform functions for signal wrap detection"
    - "Prop alias origin mapping: HashMap<alias, propsParam.key> for _fnSignal dep substitution"

key-files:
  created: []
  modified:
    - "crates/qwik-optimizer-oxc/src/props_destructuring.rs"
    - "crates/qwik-optimizer-oxc/src/transform.rs"
    - "crates/qwik-optimizer-oxc/src/jsx_transform.rs"

key-decisions:
  - "Body destructuring is detected separately from parameter destructuring -- two distinct code paths"
  - "Props param name threaded through ALL JSX transform functions rather than stored as context field"
  - "Prop alias variables that were destructured-from-props get _fnSignal dep substitution to use props param as dep source"
  - "argument_to_expression bug fix included in this plan (Rule 1 auto-fix) since it blocked test case 3"

patterns-established:
  - "detect_body_destructuring: standalone function that scans arrow body statements for const { ... } = propsParam"
  - "props_param_name: Option<&str> parameter threading through JSX transform chain"
  - "WrapPropNamed source extraction handles both StaticMemberExpression and ComputedMemberExpression at both attribute and children handler sites"

# Metrics
duration: ~35min
completed: 2026-02-20
---

# Phase 4 Plan 04: Non-Destructured Props Parameter Summary

**Close Gap 1: _wrapProp(props, "key") for non-destructured (props) parameters with body destructuring detection, _restProps insertion, and _fnSignal dep substitution**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-02-20T14:10:00Z (approximate, includes prior session)
- **Completed:** 2026-02-20T14:43:43Z
- **Tasks:** 2 (1 implementation + 1 verification)
- **Files modified:** 3

## Accomplishments
- All 4 affected test cases now produce correct output matching SWC golden for props-related transforms
- Body destructuring (`const { "bind:value": bindValue } = props`) detected and removed, references rewritten to `props["bind:value"]`
- Direct member access (`props.class`, `props["data-nu"]`) wrapped with `_wrapProp(props, "key")` in JSX children
- `_restProps(props, ["test"])` inserted correctly for rest patterns with prop alias binding stripping
- `_fnSignal` dep substitution maps prop aliases back to props param: `test.value` -> `_fnSignal(_hf0, [props], _hf0_str)` with `_hf0 = (p0) => p0.test.value`
- Fixed pre-existing bug in `argument_to_expression` (missing MemberExpression variants caused `rest["bind:value"]` to become `undefined`)
- No regressions: snapshot diff count unchanged at 160

## Task Commits

Each task was committed atomically:

1. **Task 1: Detect non-destructured props param and handle body destructuring + direct member access** - `d06b06a` (feat)
2. **Task 2: Verify all 4 affected test cases match SWC golden output** - verification only, no code changes

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/props_destructuring.rs` - Added `props_param_name` field, `BodyDestructuringInfo` struct, `detect_body_destructuring` function
- `crates/qwik-optimizer-oxc/src/transform.rs` - Body destructuring handling in enter/exit_expression for component$, reference rewriting, prop alias binding stripping, fixed `argument_to_expression` missing variants
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Extended `detect_signal_wrap` for ComputedMemberExpression/StaticMemberExpression on props param, prop alias origin substitution in `collect_reactive_deps`, threaded `props_param_name` through all JSX transform functions

## Decisions Made
- Body destructuring is a separate detection pass (`detect_body_destructuring`) from parameter destructuring analysis, called in `enter_call_expression` to populate prop_keys early before JSX transforms run
- Props param name is passed as `Option<&str>` through all JSX transform functions rather than stored as a field on the transform context -- keeps the threading explicit
- Prop alias variables from body destructuring get `_fnSignal` dep substitution: when `test.value` is seen and `test` is a prop alias, the dep becomes `props` (the props param) and the hoisted function becomes `(p0) => p0.test.value`
- `WrapPropNamed` source extraction updated at BOTH attribute and children handler sites to handle `ComputedMemberExpression` in addition to `StaticMemberExpression`
- Fallback from hardcoded `"_rawProps"` to `props_param_name.unwrap_or("_rawProps")` in WrapPropNamed handlers

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed argument_to_expression missing MemberExpression variants**
- **Found during:** Task 1 (body destructuring reference rewriting)
- **Issue:** `argument_to_expression` in `transform.rs` was missing match arms for `ComputedMemberExpression`, `StaticMemberExpression`, `PrivateFieldExpression`, and several other expression variants. These fell through to the `_ => ctx.ast.expression_identifier(SPAN, "undefined")` default, causing `rest["bind:value"]` to become `undefined` after body destructuring reference rewriting.
- **Fix:** Added all missing MemberExpression and TypeScript expression variant arms to `argument_to_expression`
- **Files modified:** `crates/qwik-optimizer-oxc/src/transform.rs`
- **Verification:** `destructure_args_colon_props3` now correctly shows `useSignal(rest["bind:value"])` instead of `useSignal(undefined)`
- **Committed in:** `d06b06a` (part of Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug fix)
**Impact on plan:** Essential fix -- without it, test case 3 produced incorrect output. No scope creep.

## Issues Encountered
- E0594 (cannot assign to `info.prop_keys`): `info` variable needed `mut` declaration -- straightforward fix
- E0621 (explicit lifetime required): `props_param_name` string needed arena allocation via `ctx.ast.atom()` for OXC identifier construction -- resolved by converting `&str` to arena atom before passing to `expression_identifier`

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Phase 4 gap closure complete: all 4 affected test cases now match SWC for props-related transforms
- Gap 2 (QRL hoisting) remains deferred to Phase 6 as documented
- Remaining differences across the 4 test cases are exclusively Phase 5 (import ordering, children flags, JSX keys) and Phase 6 (import ordering) issues
- Ready for Phase 5 planning (JSX Keys & Flags)

## Self-Check: PASSED

---
*Phase: 04-signal-props-transforms*
*Completed: 2026-02-20*
