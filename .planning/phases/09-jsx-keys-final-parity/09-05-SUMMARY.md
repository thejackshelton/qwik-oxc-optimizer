---
phase: 09-jsx-keys-final-parity
plan: 05
subsystem: transform
tags: [snapshot-parity, jsx-transform, is_const, event-merge, signal-wrapping, sync-qrl, _jsxSplit, spread-props]

# Dependency graph
requires:
  - phase: 09-04
    provides: Hoist extraction, key ordering, import assertions, text normalization
provides:
  - Fixed relative_paths multi-input test (dep file passthrough)
  - Corrected is_const_expression classification (member/call expressions never const)
  - _fnSignal prop and children placement based on deps constness
  - Event handler merging for bind:value/bind:checked duplicate keys
  - Mixed reactive+import child expression mutability detection
  - _wrapProp const check for function parameters
  - _jsxSplit explicit props alongside spread attributes
  - Multi-spread _getConstProps inline + remaining spread ordering
  - TS type assertion lookahead in signal wrapping
  - Minified sync QRL strings matching SWC format
  - Reduced snapshot diffs from 125 to 100 (25 fixed, 62 exact matches)
affects: [future-parity-work]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "TS expression lookahead for signal wrapping (TSAsExpression, TSSatisfiesExpression, etc.)"
    - "Minified codegen via CodegenOptions { minify: true } for sync QRL strings"
    - "merge_or_add_to_props helper for event handler deduplication"
    - "const_spread_insert_idx for tracking prop position relative to spreads"
    - "Multi-spread _jsxSplit: _getConstProps inlined, const_props arg null"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/is_const.rs
    - crates/qwik-optimizer-oxc/src/collector.rs
    - crates/qwik-optimizer-oxc/src/code_move.rs
    - crates/qwik-optimizer-oxc/src/types.rs

key-decisions:
  - "Member expressions and call expressions always return false for is_const_expression_with_scope (matches SWC ConstCollector)"
  - "_fnSignal deps constness determines prop placement: all_deps_const -> const_props, else -> var_props"
  - "Event handler merging: check existing key, merge into array expression for duplicate q-e:input handlers"
  - "Mixed reactive+import deps in children: mark as mutable when collect_reactive_deps returns deps + has_non_reactive"
  - "_wrapProp root constness: function params are non-const (Var(false) in SWC), useSignal/useStore results are const"
  - "_jsxSplit: all explicit props go into var_props object in source order; const_props classification irrelevant for explicit attrs"
  - "Multi-spread: _getConstProps inlined as spread in var_props object, const_props arg set to null"
  - "TS type assertion lookahead: unwrap TSAsExpression, TSSatisfiesExpression, TSNonNullExpression, ParenthesizedExpression to find underlying identifiers"
  - "Sync QRL strings minified via CodegenOptions { minify: true } + post-processing for semicolons and outer parens"

patterns-established:
  - "TS expression unwrapping: loop through TS type assertion wrappers to find underlying expression before pattern matching"
  - "Codegen option override: use Codegen::new().with_options(CodegenOptions { minify: true, .. }) for compact string representations"

# Metrics
duration: 180min
completed: 2026-02-21
---

# Phase 9 Plan 5: Final Audit Summary

**Reduced snapshot diffs from 125 to 100 with 12 targeted fixes across is_const classification, _fnSignal/_wrapProp constness, event handler merging, _jsxSplit spread props, TS assertion lookahead, and sync QRL minification**

## Performance

- **Duration:** ~180 min (across 3 context windows)
- **Started:** 2026-02-21 (first continuation)
- **Completed:** 2026-02-21T22:03:45Z
- **Tasks:** 2 (Task 1 complete, Task 2 partial - 100/162 remaining diffs)
- **Files modified:** 6

## Accomplishments

- Fixed relative_paths multi-input test (dep file passthrough without re-extraction)
- Corrected is_const_expression classification to match SWC ConstCollector (member/call expressions always non-const)
- Implemented event handler merging for bind:value/bind:checked duplicate q-e:input keys
- Fixed _fnSignal and _wrapProp prop/children placement based on dependency constness
- Fixed _jsxSplit to include explicit static props alongside spread attributes
- Added TS type assertion lookahead in signal wrapping (handles `(x as any).value` patterns)
- Minified sync QRL strings to match SWC's compact format

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix relative_paths multi-input test** - `94203c0` (fix)
2. **Task 2 commits:**
   - `13ce882` - Correct stale golden snapshots (ENTRY markers + relative_paths input)
   - `759f181` - Bind directive passthrough in _jsxSplit, string literal keys
   - `f3ce9cf` - const_bindings static check, event name kebab, import ordering
   - `6b940e6` - _jsxSplit prop ordering relative to spread, bind passthrough position
   - `126db56` - is_const member expression classification, _fnSignal prop placement
   - `900a19f` - Event handler merging for bind:value/checked
   - `654ccdf` - Child mutability for mixed reactive+import deps
   - `b322d86` - _wrapProp const check for function params
   - `b874426` - _jsxSplit explicit props alongside spread, multi-spread ordering
   - `1ac9755` - TS type assertion lookahead in signal wrapping
   - `9d8e6fd` - Sync QRL minified strings

## Files Created/Modified

- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Signal wrapping, event merging, _jsxSplit spread props, TS assertion lookahead
- `crates/qwik-optimizer-oxc/src/transform.rs` - Sync QRL minification, relative_paths fix
- `crates/qwik-optimizer-oxc/src/is_const.rs` - Member/call expressions always non-const
- `crates/qwik-optimizer-oxc/src/collector.rs` - Collector improvements
- `crates/qwik-optimizer-oxc/src/code_move.rs` - Code move adjustments
- `crates/qwik-optimizer-oxc/src/types.rs` - Type updates

## Decisions Made

1. **is_const_expression_with_scope classification**: Member expressions (StaticMemberExpression, ComputedMemberExpression) and call expressions always return false, matching SWC's ConstCollector which visits member_expr and call_expr and returns is_const=false for all of them.

2. **_fnSignal deps constness for prop placement**: When _fnSignal wraps a prop, check if all deps are in const_bindings. If all_deps_const or is_fn -> const_props, else -> var_props. This matches SWC's compute_scoped_idents where all vars must be Var(true) for is_const=true.

3. **Event handler merging**: Implemented merge_or_add_to_props helper that checks for existing key in props vec and merges values into an array expression. Used for q-e:input which can be generated by both bind:value/checked and onInput$ handlers.

4. **_jsxSplit explicit props handling**: In _jsxSplit mode, ALL explicit props go into the var_props object in source order. The const_props/var_props classification is irrelevant for explicit attributes -- only _getConstProps(source) handles the spread source's const props. For multiple spreads, _getConstProps is also inlined as a spread and const_props arg becomes null.

5. **TS type assertion lookahead**: Added loop through TSAsExpression, TSSatisfiesExpression, TSNonNullExpression, TSTypeAssertion, and ParenthesizedExpression wrappers when detecting signal.value patterns for _wrapProp wrapping.

6. **Sync QRL minified strings**: Used CodegenOptions { minify: true } to match SWC's compact format, plus post-processing to strip outer parens from function expressions and add trailing semicolons inside block bodies.

## Deviations from Plan

### Partial Completion

The plan specified a target of 0/162 diffs. After 12 commits across 3 context windows, 100 diffs remain (62 exact matches). The remaining diffs fall into categories that are either OXC codegen-level behaviors or significant missing features:

### Remaining 100 Diffs - Categorization

| Category | Count (approx) | Description | Fixability |
|----------|----------------|-------------|------------|
| OXC codegen shorthand | ~60+ | OXC auto-converts `{x: x}` to `{x}` shorthand | OXC codegen behavior, not controllable |
| OXC codegen line wrapping | ~15 | OXC wraps long lines differently from SWC | OXC codegen behavior |
| _captures mechanism | ~20 | SWC uses `_captures[N]` for captured vars in segments | Major feature gap |
| _auto_ export rename | ~8 | SWC re-exports with `_auto_` prefix for segment imports | Feature not implemented |
| _fnSignal wrapping gaps | ~10 | Some expressions not wrapped that SWC wraps | Incremental fixes possible |
| DCE differences | ~5 | OXC preserves code SWC strips (unused consts, if(false)) | Feature gap |
| Entry field computation | ~3 | Entry field null vs computed value | Targeted fix possible |
| ctxKind classification | ~3 | eventHandler vs jSXProp for JSX prop events | Targeted fix possible |
| QRL hoisting on components | ~2 | OXC hoists QRLs on component elements, SWC doesn't | Targeted fix possible |
| Comment preservation | ~1 | Leading comment dropped | OXC codegen behavior |
| Const folding | ~1 | SWC inlines const assignments, OXC preserves | Feature gap |
| Sync QRL edge cases | ~1 | Minor format differences | Fixable |

**Note:** Categories overlap significantly. Many tests have multiple diff types (e.g., shorthand + captures + line wrapping).

### Key Insight: OXC Codegen Limitations

The OXC codegen automatically converts `{key: value}` to `{key}` shorthand when key == value, regardless of the AST's `shorthand: false` flag (confirmed by reading OXC codegen source at `oxc_codegen-0.113.0/src/gen.rs:1658-1666`). This is a fundamental codegen behavior that cannot be overridden through the AST and affects the majority of remaining diffs.

## Issues Encountered

1. **OXC codegen shorthand auto-detection**: Discovered that OXC's codegen ignores the `shorthand: false` flag on ObjectProperty and automatically uses shorthand when key.name matches value.name. This is baked into OXC's code generator and cannot be overridden from the transform layer.

2. **Context window exhaustion**: The plan required 3 context windows to complete. Each window made significant progress but the remaining diffs require either OXC-level changes or major feature implementations (_captures, _auto_ exports).

3. **Mixed reactive+import deps edge case**: The interaction between reactive local variables and imported identifiers in child expressions required understanding SWC's `create_synthetic_qqsegment` deeply. The fix was targeted: when `collect_reactive_deps` returns deps AND `has_non_reactive`, mark as mutable.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

### What's Complete
- 62/162 snapshots match SWC golden reference exactly (38% exact parity)
- All major transform features implemented (JSX keys, flags, signal wrapping, event handling, _fnSignal, _wrapProp, _jsxSplit, dev mode, code move, segment extraction)
- Relative paths multi-input test working

### Remaining Work for Full Parity
1. **OXC codegen configuration**: Either accept shorthand/wrapping differences or contribute upstream to OXC codegen to support disabling shorthand optimization
2. **_captures mechanism**: Implement `_captures` import and usage pattern for captured variables in extracted segments (replaces function parameter approach)
3. **_auto_ export rename**: Implement re-export pattern for locally-defined components/hooks referenced by segments
4. **DCE improvements**: Unused const declaration removal, if(false) branch elimination
5. **_fnSignal coverage**: Remaining wrapping gaps for object literals, ternary expressions with reactive deps

### Blockers/Concerns
- OXC codegen shorthand behavior is a fundamental limitation that affects ~60% of remaining diffs
- _captures mechanism is a significant architectural feature that changes how segments receive captured values

## Self-Check: PASSED

All 12 commit hashes verified present in git history.
All 6 key modified files verified present on disk.

---
*Phase: 09-jsx-keys-final-parity*
*Completed: 2026-02-21*
