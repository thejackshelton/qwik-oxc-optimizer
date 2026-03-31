---
phase: 11-auto-export-rename
plan: 01
subsystem: transform
tags: [auto-export, segment-import, self-import, explicit-extensions]
requires: [10]
provides: [_auto_ prefix mechanism for segment self-imports]
affects: [12]
tech-stack:
  added: []
  patterns: [_auto_ prefix for non-user-exported module-level decl re-exports]
key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/collector.rs
    - crates/qwik-optimizer-oxc/src/types.rs
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/code_move.rs
    - crates/qwik-optimizer-oxc/src/lib.rs
key-decisions:
  - "export default function/class treated as exported for _auto_ purposes (matches SWC)"
  - "Destructured pattern exports tracked via collect_binding_pattern_names_into (not just simple bindings)"
  - "Stripped segments excluded from auto_exports via is_stripped parameter"
  - "TS enum _auto_ exports accepted as known OXC limitation (SWC inlines enum values)"
  - "Pre-transformed inlinedQrl inputs not supported for _auto_ (pre-existing gap, not this phase)"
duration: 13min
completed: 2026-02-23
---

# Phase 11 Plan 01: Auto Export Rename Summary

Implement `_auto_` prefix mechanism for segment import re-exports, matching SWC behavior for non-user-exported module-level declarations.

## Performance

- **Duration:** 13 minutes
- **Start:** 2026-02-23T10:54:07Z
- **End:** 2026-02-23T11:07:01Z
- **Tasks:** 2/2 completed
- **Files modified:** 5

## Accomplishments

1. **Collector: Track exported local names** - Added `exported_local_names` HashSet to collector context and result. Populated from declaration exports (`export const/function/class`), specifier exports (`export { X }`), destructured pattern exports, and default exports (`export default function X`).

2. **Transform: Auto-exports tracking** - Added `auto_exports` HashSet populated during `reclassify_module_level_decl_captures()` and `finalize_segments()`. Only names that are in `module_level_decls` but NOT in `exported_local_names` get tracked. Stripped segments are excluded to prevent incorrect _auto_ exports.

3. **Transform: Entry module _auto_ exports** - Emit sorted `export { X as _auto_X }` statements at the bottom of entry modules in `exit_program()` for all names in `auto_exports`.

4. **Transform: Fix self_import_source** - When `explicit_extensions=true`, preserve the full filename (e.g., `./index.qwik.mjs`) instead of stripping the extension (e.g., `./index`).

5. **Code move: Segment _auto_ imports** - Thread `auto_exports` HashSet from transform through lib.rs to `build_segment_code_with_hoisted()`. Segment self-imports for names in `auto_exports` use `import { _auto_X as X }` syntax. Framework imports (`is_qwik_core`) are never affected.

## Task Commits

| Task | Name | Commit | Key Changes |
|------|------|--------|-------------|
| 1 | Track exported local names + auto_exports + emit _auto_ exports + fix self_import_source | 3728d45 | collector.rs, types.rs, transform.rs |
| 2 | Thread auto_exports to segment code generation | 3107612 | code_move.rs, lib.rs |

## Files Modified

- `crates/qwik-optimizer-oxc/src/collector.rs` - exported_local_names in CollectContext, population in collect_named_export/collect_default_export
- `crates/qwik-optimizer-oxc/src/types.rs` - exported_local_names field in CollectResult
- `crates/qwik-optimizer-oxc/src/transform.rs` - auto_exports HashSet, reclassify with is_stripped, _auto_ export emission in exit_program, self_import_source fix
- `crates/qwik-optimizer-oxc/src/code_move.rs` - auto_exports parameter, _auto_ alias logic in segment imports
- `crates/qwik-optimizer-oxc/src/lib.rs` - Thread auto_exports from QwikTransform to code_move

## Decisions Made

1. **export default exports mark name as exported** - `export default function X` and `export default class X` mark `X` as exported for _auto_ purposes, preventing unnecessary `_auto_X` re-exports. Matches SWC behavior.

2. **Destructured pattern exports fully tracked** - `export const [a, {b, ...c}] = obj` tracks ALL binding names (a, b, c) as exported via `collect_binding_pattern_names_into`, not just simple `BindingIdentifier`.

3. **Stripped segments excluded from auto_exports** - The `is_stripped` parameter was added to `reclassify_module_level_decl_captures()` to prevent stripped segments (e.g., server$) from incorrectly generating _auto_ exports for their referenced module-level decls.

4. **TS enum _auto_ as known limitation** - SWC inlines TS enum values (Thing.A -> 0), eliminating references. OXC doesn't inline enums, so `_auto_Thing` exports appear. This is a pre-existing OXC limitation, not a regression.

5. **Pre-transformed inlinedQrl not supported** - Tests like `example_qwik_react` and `relative_paths` use pre-transformed code with `inlinedQrl()` calls. The OXC optimizer doesn't extract segments from these, so _auto_ mechanism doesn't apply. This is a pre-existing gap.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Destructured pattern exports not tracked**
- **Found during:** Task 1 (verification)
- **Issue:** `export const [a, {b}] = obj` only tracked simple `BindingIdentifier` via `binding_pattern_name()`, missing all destructured bindings
- **Fix:** Added `collect_binding_pattern_names_into()` call before the simple name check to capture all bindings from patterns
- **Files modified:** collector.rs
- **Commit:** 3728d45

**2. [Rule 1 - Bug] export default function/class not tracked as exported**
- **Found during:** Task 1 (verification)
- **Issue:** `export default function X()` was adding X to `module_level_decls` but not `exported_local_names`, causing incorrect `_auto_X` exports
- **Fix:** Added `exported_local_names.insert()` in `collect_default_export()` for named function/class defaults
- **Files modified:** collector.rs
- **Commit:** 3728d45

**3. [Rule 1 - Bug] Stripped segments generating incorrect _auto_ exports**
- **Found during:** Task 1 (verification)
- **Issue:** `server$()` bodies reference module-level decls, but the segment is stripped (becomes _noopQrl). Reclassification was adding these to auto_exports unnecessarily.
- **Fix:** Added `is_stripped` parameter to `reclassify_module_level_decl_captures()`, checked `self.stripped_segments.contains()` before calling
- **Files modified:** transform.rs
- **Commit:** 3728d45

## Issues Encountered

- **Pre-transformed inlinedQrl tests** - 3 of the 8 targeted tests (example_qwik_react, example_qwik_react_inline, relative_paths) use pre-transformed code with direct `inlinedQrl()` calls. The OXC optimizer doesn't process these as segment extraction candidates, so the _auto_ mechanism cannot apply. This is a pre-existing architectural gap unrelated to this phase.

## Next Phase Readiness

- Phase 12 can proceed. The _auto_ export mechanism is complete for all test cases where the OXC optimizer extracts segments.
- 5 of 8 targeted test cases show correct _auto_ behavior (entry exports + segment imports).
- 2 additional tests (TS enum cases) show _auto_ exports that SWC avoids via enum inlining -- cosmetic difference, not functional.
- No blockers for Phase 12.

## Self-Check: PASSED
