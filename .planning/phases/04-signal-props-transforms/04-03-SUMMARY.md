---
phase: 04-signal-props-transforms
plan: 03
subsystem: props-destructuring
tags: [props, destructuring, default-values, skip-cases, _restProps, _rawProps, nullish-coalesce]

requires:
  - phase: 03-bugs-correctness
    provides: "All segments extracted, correct ordering"
provides:
  - "Props destructuring with default values: _rawProps.key ?? defaultValue"
  - "Complete _restProps excluded_keys (all destructured prop names including defaults)"
  - "Skip cases for nested destructuring and non-const default values"
  - "Import-aware const checking for default value expressions"
affects: [04-02, 05-jsx-keys-flags]

tech-stack:
  added: []
  patterns: ["parse-and-clone across allocator boundaries for default expression rebuilding", "import-aware const checking for destructuring analysis"]

key-files:
  created: []
  modified: ["crates/qwik-optimizer-oxc/src/props_destructuring.rs", "crates/qwik-optimizer-oxc/src/transform.rs"]

key-decisions:
  - "Default value expressions serialized to strings during analysis, rebuilt via parse-and-clone during rewrite (avoids lifetime issues with arena-allocated AST nodes)"
  - "Import identifiers treated as const for default value checking (matches SWC's is_const_expr behavior)"
  - "use*() return value destructuring inlining deferred to Phase 6: only 2 test fixtures affected (example_use_optimization, should_wrap_prop_from_destructured_array)"

duration: 11min
completed: 2026-02-20
---

# Phase 4 Plan 3: Props Destructuring Completeness Summary

**One-liner:** Default value handling with ?? rewriting, skip cases for nested/non-const patterns, complete _restProps excluded_keys matching SWC

## What Changed

### Props Destructuring Analysis (`analyze_props_destructuring`)

Extended to handle all OXC `BindingProperty` value patterns:

1. **Simple BindingIdentifier** (`{ count }`, `{ stuff: hey }`): Already worked, unchanged
2. **AssignmentPattern with const default** (`{ some = 1 + 2 }`, `{ hello = CONST }`, `{ stuffDefault: hey2 = 123 }`):
   - Extracts local name from `assign.left`
   - Checks default expression constness via `is_const_default_value()`
   - Serializes default code, stores in `prop_defaults` HashMap
   - Includes prop in `prop_keys` for excluded_keys and body rewriting
3. **AssignmentPattern with non-const default** (`{ stuff = hola() }`):
   - Sets `skip = true` -- entire destructuring left unchanged
4. **Nested ObjectPattern/ArrayPattern** (`{ stuff: { hey } }`):
   - Sets `skip = true` -- entire destructuring left unchanged

### Import-Aware Const Checking

New `is_const_default_value()` function checks if a default expression is a compile-time constant:
- Literals (string, number, boolean, null, bigint, regexp): always const
- Identifiers: const only if they're imports or well-known globals (undefined, NaN, Infinity)
- Binary/unary/parenthesized expressions: const if all sub-expressions are const
- Template literals: const if all interpolation expressions are const
- Everything else (calls, member expressions, arrows): NOT const

The `analyze_props_destructuring` now accepts `import_names: &HashSet<String>` parameter built from `CollectResult.module_imports`.

### Default Value Rewriting

When `rewrite_props_references` encounters an identifier that has a default:
- Builds `_rawProps.key ?? defaultExpr` instead of just `_rawProps.key`
- Default expression is rebuilt from source code string via `build_expression_from_code()`:
  - Simple cases (numbers, strings, booleans, identifiers) built directly with AST builder
  - Complex cases (binary expressions) parsed in a temporary allocator then cloned to the traversal context via `clone_expression_to_ctx()`

### Excluded Keys Completeness

`_restProps(_rawProps, [...])` now includes ALL destructured prop names:
- Before: `_restProps(_rawProps, ["count", "stuff"])` (only BindingIdentifier props)
- After: `_restProps(_rawProps, ["count", "some", "hello", "stuff", "stuffDefault"])` (all props including those with defaults)

## Task Commits

| Task | Description | Commit | Key Files |
|------|------------|--------|-----------|
| 1 | Default value handling + skip cases + excluded_keys | c0b1adb | props_destructuring.rs, transform.rs |
| 2 | Verification + use*() decision (no code changes) | -- | -- |

## Decisions Made

1. **Default expression storage**: Serialized to strings during analysis, rebuilt during rewrite. This avoids lifetime issues with arena-allocated AST nodes crossing allocator boundaries. The alternative (storing Expression references) would require same-allocator constraints that don't hold between analysis and rewrite phases.

2. **Import-aware const checking**: Import identifiers are treated as const for default value checking, matching SWC's `is_const_expr` behavior. This is necessary because `{ hello = CONST }` where CONST is imported should be transformable (it IS a const value at runtime).

3. **use*() inlining deferred**: Only 2 test fixtures (`example_use_optimization`, `should_wrap_prop_from_destructured_array`) show diffs from missing use*() return value destructuring inlining. This is well below the plan's threshold of 5, so it's deferred to Phase 6 cleanup.

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build -p qwik-optimizer-oxc` compiles cleanly (no warnings)
- `cargo insta test -p qwik-optimizer-oxc` runs without panics (160 pre-existing snapshot diffs from Phase 4/5/6 work, none new)
- `example_props_optimization` snapshot: Works component correctly transforms with all 5 excluded_keys and ?? defaults
- `NoWorks2` (nested destructuring) correctly skips transformation
- `NoWorks3` (non-const default `hola()`) correctly skips transformation
- Pre-existing test failure in `destructure_args_colon_props` confirmed to be from concurrent 04-01 work, not this plan

## Next Phase Readiness

Props destructuring is now feature-complete for Phase 4. Remaining Phase 4 work:
- Plan 04-02: Loop tracking + q:p injection + QRL hoisting (~22 snapshot diffs)
- Phase 5: JSX keys/flags, import ordering
- Phase 6: use*() inlining (2 deferred diffs), should_extract_single_qrl_2 naming

## Self-Check: PASSED
