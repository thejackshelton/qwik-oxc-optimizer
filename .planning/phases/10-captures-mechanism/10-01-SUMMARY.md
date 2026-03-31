# Phase 10 Plan 01: Captures Mechanism - Iteration Variable Params Summary

**One-liner:** Filter iteration vars from captures and inject them as function params in segment modules, matching SWC's scoped_idents.retain(!param_idents) behavior.

## Task Commits

| Task | Name | Commit | Key Changes |
|------|------|--------|-------------|
| 1 | Filter iteration variable params from captures + fix iteration_var_stack.last() | e71fe74 | transform.rs: current_iteration_vars() uses .last() not flatten; iter_var_param_names filtering after reclassify |
| 2 | Inject iteration variable params into segment function signatures | 13ee5c1 | code_move.rs: inject_iteration_params() with placeholder de-duplication (_->_1) |

## What Changed

### transform.rs
- **current_iteration_vars()**: Changed from `iteration_var_stack.iter().flatten()` (ALL loop scopes) to `iteration_var_stack.last()` (innermost loop scope only), matching SWC's `self.iteration_var_stack.last()`. This ensures outer-loop variables (e.g., `row` in nested `.map().map()`) become captures rather than iteration params for inner handlers.
- **Capture filtering**: After `reclassify_module_level_decl_captures()`, added a step that removes iteration variable param names (positions 2+ in param_names) from capture_names. This mirrors SWC's `scoped_idents.retain(|id| !param_idents.contains(id))`.
- **iter_var_param_names extraction**: Saved iteration variable names before `param_names` is moved into `record_jsx_event_segment()`.

### code_move.rs
- **inject_iteration_params()**: New function that replaces the function parameter list in body_code when param_names has 3+ entries. Handles de-duplication of placeholder names: SWC uses `private_ident!("_")` for both event and element placeholders, and codegen de-duplicates to `_`, `_1`. The function uses `find_arrow_position()` to locate the arrow, then replaces the param list between `(` and `)`.
- **Phase 6 restructured**: Param injection runs before capture injection to avoid arrow position shift issues.

## Decisions Made

| Decision | Context | Rationale |
|----------|---------|-----------|
| `current_iteration_vars()` uses `.last()` only | SWC uses `self.iteration_var_stack.last()` | Outer-loop variables that are used in inner handlers should be captures, not params. SWC only considers the innermost loop's variables for param injection. |
| paramNames metadata keeps duplicate `_` | SWC golden reference stores `["_", "_", "row"]` | De-duplication to `_1` only happens in code generation (segment function signature), not in metadata |
| Param injection before capture injection in Phase 6 | Both use `find_arrow_position()` | Injecting params first changes the arrow position; running it before captures avoids stale position bugs |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed current_iteration_vars() to use .last() instead of flatten()**
- **Found during:** Task 1 investigation of should_transform_nested_loops
- **Issue:** `current_iteration_vars()` flattened ALL iteration_var_stack frames, returning variables from all nested loops. SWC only uses `iteration_var_stack.last()` (innermost loop). This caused outer-loop variables like `row` to be treated as iteration params for inner handlers, where they should be captures.
- **Fix:** Changed from `.iter().flatten()` to `.last().map(|v| v.clone()).unwrap_or_default()`
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Commit:** e71fe74

**2. [Plan deviation] paramNames de-duplication moved to code_move.rs**
- **Found during:** Task 1 analysis
- **Issue:** Plan Part A said to de-duplicate paramNames in transform.rs, but SWC golden reference stores duplicate `_` in paramNames metadata (`["_", "_", "row"]`). De-duplication only happens in codegen.
- **Fix:** Skipped the transform.rs change; implemented de-duplication in code_move.rs inject_iteration_params() instead.
- **Files modified:** crates/qwik-optimizer-oxc/src/code_move.rs
- **Commit:** 13ee5c1

## Known Remaining Issues

1. **Nested component$ capture scope**: In tests like `should_extract_single_qrl_with_nested_components` and `should_transform_component_with_normal_function`, variables declared in `.map()` callbacks (e.g., `item`) are incorrectly captured by the parent `component$` body. This is a pre-existing capture_stack issue (the stack only tracks `$()` body scopes, not regular function/callback scopes), not related to iteration variable handling.

2. **should_transform_nested_loops inner handler**: The inner handler correctly identifies `item` as an iter var param, but `row` (from outer loop) is not captured because the capture_stack filter removes it. SWC has `row` as a capture. Same root cause as issue #1.

## Metrics

- **Duration:** 13 minutes
- **Completed:** 2026-02-23
- **Tests affected:** 10 tests improved (iteration variable function signatures and capture arrays fixed)
- **Snapshot count:** 100 (unchanged -- fixes are within tests that have other unrelated diffs)

## Self-Check: PASSED
