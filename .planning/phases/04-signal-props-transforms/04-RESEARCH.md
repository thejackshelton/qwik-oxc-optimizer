# Phase 4: Signal & Props Transforms - Research

**Researched:** 2026-02-20
**Domain:** Qwik optimizer signal reactivity transforms (SWC -> OXC port)
**Confidence:** HIGH

## Summary

Phase 4 addresses four interrelated transforms that the SWC Qwik optimizer applies to JSX output but the OXC port currently does not fully implement. Together these account for approximately 121+ snapshot diffs (some overlapping): `_fnSignal` wrapping (~57), `_wrapProp` generation (~42), `q:p` iteration variable injection (~22), and QRL hoisting (embedded in the other counts). The OXC codebase already has partial infrastructure for `_fnSignal` and `_wrapProp` in JSX **attributes**, but is missing these transforms for JSX **children**, lacks loop/iteration tracking entirely (no `loop_depth`, no `iteration_var_stack`), and does not hoist QRL calls to variable declarations.

The SWC reference implementation is available at `crates/swc-optimizer/core/src/` and has been deeply analyzed. The key SWC files are `transform.rs` (3805 lines, main orchestration), `inlined_fn.rs` (294 lines, `_fnSignal` generation via `convert_inlined_fn`), and `props_destructuring.rs` (520 lines, `_restProps` + prop reference rewriting).

**Primary recommendation:** Implement in three plans: (1) `_fnSignal` wrapping for JSX children + `_wrapProp` for children, (2) loop tracking + `q:p` injection + QRL hoisting, (3) props destructuring completeness + `_restProps` correctness. The existing OXC code for JSX attribute signal wrapping is correct and can serve as the template for children wrapping.

## Standard Stack

This phase operates entirely within the existing Rust/OXC toolchain. No new libraries are needed.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| oxc | (current) | AST manipulation, traversal, codegen | Project's AST framework |
| oxc_traverse | (current) | Visitor pattern for AST walking | Used throughout transform.rs |

### Supporting
No additional libraries needed. All transforms are implemented as custom AST transformations within the existing crate.

## Architecture Patterns

### Current OXC Architecture for Signal Transforms

The existing code has three layers of signal/prop handling:

1. **JSX Attribute Level** (`jsx_transform.rs:1031-1092`): For each non-event JSX attribute, the code runs `detect_signal_wrap()` to check for `X.value` (WrapPropSignal) or `_rawProps.propName` (WrapPropNamed), then falls through to `_fnSignal` wrapping via `collect_reactive_deps()` + `build_fn_signal_wrapping()`. This layer works correctly.

2. **JSX Children Level** (`jsx_transform.rs:1453-1470`): For JSX expression children, only `_wrapProp(signal)` for simple `.value` access is handled. Missing: `_wrapProp(_rawProps, "propName")` for named prop access, and `_fnSignal` wrapping for complex reactive expressions in children.

3. **Module/Segment Level** (`transform.rs`, `code_move.rs`): Hoisted function declarations (`_hf0`, `_hf0_str`) are tracked in `hoisted_function_stmts` and emitted at segment module top level by `code_move.rs`. This infrastructure works but is only fed from the attribute-level transform.

### SWC Architecture for Comparison

SWC handles signal transforms through a different approach:

1. **`convert_to_getter()`** (`transform.rs:2191`): Called for JSX attribute values. Delegates to `create_synthetic_qqsegment()` which calls `convert_inlined_fn()`. Returns `(expr, is_const)`. If the result is a `_fnSignal` call, it then calls `hoist_fn_signal_call()` to extract the arrow function to a top-level `const _hfN = ...`.

2. **`convert_to_signal_item()`** (`transform.rs:2206`): Called for JSX children array elements. Same logic as `convert_to_getter()` but also marks `jsx_mutable` when calls are detected.

3. **`convert_children()`** (`transform.rs:2028`): Called for the `children` prop. Handles arrays by mapping each element through `convert_to_signal_item()`. Handles single expressions the same way.

4. **`_wrapProp` generation** (`transform.rs:620-636`): Inside `create_synthetic_qqsegment()`, when the expression is `obj.prop` where `obj` is an identifier, it generates `_wrapProp(obj, "prop")` or `_wrapProp(obj)` (for `.value`). This is the same pattern OXC implements for attributes.

5. **Loop/Iteration Tracking** (`transform.rs:127-128, 2786-2977`): SWC tracks `loop_depth` and `iteration_var_stack` by intercepting `fold_for_stmt`, `fold_for_in_stmt`, `fold_for_of_stmt`, `fold_while_stmt`, and array method calls (`.map()`, `.filter()`, etc.). When `loop_depth > 0`, it:
   - Hoists QRL calls to variable declarations before the loop
   - Adds `q:p` (single) or `q:ps` (multiple) props to JSX elements that use iteration variables
   - Transforms event handler lambdas to accept iteration variables as extra parameters

6. **QRL Hoisting** (`transform.rs:985-1051, 2478-2506`): `hoist_qrl_if_needed()` creates `const SegmentName = /* @__PURE__ */ qrl(...)` declarations that are injected after the last variable declaration in the enclosing block via `inject_hoisted_qrls_into_block()`.

### Recommended Implementation Structure

```
src/
├── transform.rs         # Add loop_depth, iteration_var_stack tracking
│                        # Add QRL hoisting state (hoisted_qrls stack)
│                        # Add enter/exit for ForStatement, ForInStatement, etc.
│                        # Add enter/exit for CallExpression (.map/.filter)
├── jsx_transform.rs     # Extend transform_jsx_children for _fnSignal + _wrapProp
│                        # Add q:p/q:ps injection in JSX element transform
│                        # Add QRL variable reference (instead of inline) for event handlers
├── code_move.rs         # Already handles hoisted functions (no changes expected)
├── props_destructuring.rs  # May need fixes for excluded_keys completeness
└── import_rewrite.rs    # Already has build_wrap_prop_call/build_wrap_prop_call_named
```

### Pattern 1: _fnSignal Wrapping for JSX Children
**What:** When a JSX child expression contains reactive deps (signal.value, store.prop, etc.) and no function calls, wrap it with `_fnSignal(_hfN, [deps], _hfN_str)`.
**When to use:** JSX children that are expressions (not text literals, not JSX elements, not function calls).
**Example:**
```rust
// In transform_jsx_children, after getting the expression from JSXExpressionContainer:
// Current code only handles _wrapProp for .value:
//   if detect_signal_wrap == WrapPropSignal -> build_wrap_prop_call
//
// Need to add:
//   1. _wrapProp(_rawProps, "propName") for WrapPropNamed in children
//   2. collect_reactive_deps() -> if deps && !has_non_reactive -> build_fn_signal_wrapping
//
// The existing attribute-level code (jsx_transform.rs:1068-1092) is the exact template.
```

### Pattern 2: Loop/Iteration Tracking
**What:** Track when AST traversal enters loop constructs (for, for-in, for-of, while) or iteration method callbacks (.map, .filter, etc.). Record iteration variables for each scope.
**When to use:** Any JSX that contains event handlers inside loops.
**SWC Reference:**
```rust
// SWC state fields (transform.rs:127-128):
// loop_depth: u32,
// iteration_var_stack: Vec<Vec<ast::Ident>>,
//
// For each loop type:
// 1. Increment loop_depth
// 2. Extract iteration variable(s) from the loop header
// 3. Push to iteration_var_stack
// 4. Process children
// 5. Pop iteration_var_stack
// 6. Decrement loop_depth
//
// For .map()/.filter()/etc:
// 1. Detect method name from callee member expression
// 2. Extract callback parameters as iteration variables
// 3. Same push/pop pattern
```

### Pattern 3: QRL Hoisting
**What:** When inside a loop (loop_depth > 0), QRL calls (`qrl(i_HASH, "name", [captures])`) are hoisted to `const name = qrl(...)` variable declarations above the loop, and the JSX prop value becomes just the variable reference.
**When to use:** Event handler QRLs inside loop bodies.
**SWC Reference:**
```rust
// SWC: hoist_qrl_if_needed() (transform.rs:985)
// - Only hoists when loop_depth > 0 && !is_fn && hoisted_qrls not empty
// - Uses the symbol_name as the variable name
// - Deduplicates: if same QRL already hoisted at current depth, reuses the variable
// - inject_hoisted_qrls_into_block() inserts declarations after last VarDecl in block
```

### Pattern 4: q:p Injection
**What:** When a JSX event handler inside a loop uses iteration variables, the optimizer adds `"q:p": iterVar` (single) or `"q:ps": [var1, var2]` (multiple) to the element's var_props. The handler lambda also gets extra parameters `(_, _1, iterVar)`.
**When to use:** JSX event handlers ($-suffixed attributes) inside loops that reference iteration variables.
**SWC Reference:**
```rust
// SWC: transform.rs:1748-1796
// 1. Check if handler body uses any iteration variables (expr_uses_ident)
// 2. Transform handler: add placeholder params (_, _1) then iteration var params
// 3. Add q:p/q:ps prop to element's var_props
// 4. paramNames of the segment must reflect the added parameters
```

### Anti-Patterns to Avoid
- **Wrapping function calls with _fnSignal:** SWC explicitly skips expressions containing function calls (the `used_as_call` check in `is_used_as_object_or_call`). OXC already has `contains_function_call()` for this.
- **Wrapping arrow/function expressions:** SWC aborts `convert_inlined_fn` if the expression is an arrow. OXC must not wrap these.
- **Wrapping too-long expressions:** SWC has a 150-char limit on the rendered expression string (`rendered_expr.len() > 150`). OXC should match this.
- **Double-wrapping:** If an expression is already a `_wrapProp` or `_fnSignal` call, don't wrap it again.
- **Hoisting non-loop QRLs:** Only hoist when `loop_depth > 0`. Top-level QRLs remain inline.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Signal detection in children | New analysis code | Reuse existing `detect_signal_wrap()` + `collect_reactive_deps()` from jsx_transform.rs | Already works correctly for attributes; same logic applies to children |
| _fnSignal AST construction | New builder | Reuse existing `build_fn_signal_wrapping()` from jsx_transform.rs | Handles param naming, code serialization, minification |
| _wrapProp AST construction | New builder | Reuse existing `build_wrap_prop_call()`/`build_wrap_prop_call_named()` from import_rewrite.rs | Already handles both 1-arg and 2-arg forms |
| _restProps declaration | New builder | Reuse existing `build_rest_props_declaration()` from props_destructuring.rs | Handles excluded_keys array construction |
| Expression codegen for hoisted fns | Custom serializer | Reuse existing `Codegen::new().print_expression()` pattern from build_fn_signal_wrapping | OXC's codegen handles all expression types |

## Common Pitfalls

### Pitfall 1: _fnSignal only for children in expression containers, not all children
**What goes wrong:** Wrapping text children or JSX element children with _fnSignal.
**Why it happens:** Not filtering child types before applying signal analysis.
**How to avoid:** Only apply reactive analysis to `JSXChild::ExpressionContainer` children that resolve to non-JSX, non-literal expressions. Text nodes and nested JSX elements are never wrapped.
**Warning signs:** `_fnSignal` appearing around string literals or nested `_jsxSorted` calls.

### Pitfall 2: _wrapProp for children uses 1-arg form for `.value` and 2-arg form for named props
**What goes wrong:** Using `_wrapProp(obj, "value")` instead of `_wrapProp(obj)` for signal.value access.
**Why it happens:** Treating `.value` access the same as other member accesses.
**How to avoid:** The existing `detect_signal_wrap()` already distinguishes `WrapPropSignal` (1-arg) from `WrapPropNamed` (2-arg). Use the same function for children.
**Warning signs:** `_wrapProp(signal, "value")` instead of `_wrapProp(signal)`.

### Pitfall 3: q:p goes into var_props, not const_props
**What goes wrong:** Putting `q:p` into the wrong JSX argument position.
**Why it happens:** `q:p` is added as a JSX prop, but since the iteration variable is runtime-varying, it must go into var_props (2nd argument of `_jsxSorted`), not const_props (3rd argument).
**How to avoid:** Always add `q:p`/`q:ps` to `var_props` in the JSX transform, never to `const_props`.
**Warning signs:** `q:p` appearing in the const_props object.

### Pitfall 4: QRL hoisting position in block
**What goes wrong:** Inserting hoisted QRL declarations at the wrong position in the block.
**Why it happens:** SWC inserts after the LAST variable declaration in the block. Inserting at the top or after the first declaration changes semantics.
**How to avoid:** Use `rposition(|stmt| matches!(stmt, VarDecl))` to find insertion point, matching SWC.
**Warning signs:** ReferenceError because hoisted QRL references a variable declared after it.

### Pitfall 5: Iteration variable detection for .map() callbacks
**What goes wrong:** Not extracting iteration variables from `.map()` / `.filter()` / etc. callbacks.
**Why it happens:** Only tracking `for`/`while` loops but forgetting array method callbacks.
**How to avoid:** SWC detects iteration methods by checking the member expression property name against `["map", "filter", "forEach", "flatMap", "some", "every", "find", "findIndex", "reduce", "reduceRight"]`. The OXC implementation must match this list.
**Warning signs:** Missing `q:p` injection and missing QRL hoisting inside `.map()` callbacks.

### Pitfall 6: Event handler parameter transformation for q:p
**What goes wrong:** Not adding `(_, _1, iterVar)` parameters to event handler lambdas.
**Why it happens:** Adding `q:p` to the element but forgetting to transform the handler's parameter list.
**How to avoid:** When iteration variables are used by a handler:
1. Ensure first 2 params exist (add `_` placeholders if missing)
2. Append used iteration variables as additional params
3. Update segment's `paramNames` metadata to reflect all parameters
**Warning signs:** Segment's `paramNames` don't include `_`, `_1`, and the iteration variable name.

### Pitfall 7: Props destructuring excluded_keys completeness
**What goes wrong:** `_restProps(_rawProps, ["key1", "key2"])` missing keys that SWC includes.
**Why it happens:** The OXC `analyze_props_destructuring` extracts prop keys from the ObjectPattern, but SWC also includes destructured nested props and props with default values that have const defaults.
**How to avoid:** Compare OXC's `_restProps` excluded_keys lists with SWC's output for the `example_props_optimization` test case. The SWC version includes all destructured prop names: `["count", "some", "hello", "stuff", "stuffDefault"]`.
**Warning signs:** Diff showing different arrays in `_restProps()` calls.

### Pitfall 8: Hoisted function deduplication
**What goes wrong:** Creating duplicate `_hfN` declarations for the same function body.
**Why it happens:** SWC deduplicates hoisted functions by their rendered body string in `hoisted_fn_signals: HashMap<String, Id>`. If the same expression appears multiple times, it reuses the first hoisted function.
**How to avoid:** Before creating a new hoisted function, check if one with the same rendered body already exists. OXC currently creates unique `_hfN` names per call but doesn't deduplicate.
**Warning signs:** Multiple `_hf0`, `_hf1` declarations with identical function bodies.

## Code Examples

### Example 1: _fnSignal wrapping for JSX children (to add)
Source: SWC `convert_to_signal_item()` (transform.rs:2206) + golden snapshot analysis.
```rust
// In jsx_transform.rs, transform_jsx_children(), after the existing _wrapProp check:
// (this is pseudocode showing the pattern to implement)
JSXChild::ExpressionContainer(container) => {
    let expr = jsx_expression_to_expression(container.expression, ctx);
    match expr {
        // ... existing JSXElement/JSXFragment handling ...
        other => {
            // 1. Check for _wrapProp (signal.value) -- ALREADY EXISTS
            if !is_call_on_value(&other) {
                match detect_signal_wrap(&other, destructured_props) {
                    SignalWrapResult::WrapPropSignal => { /* existing code */ }
                    // 2. NEW: _wrapProp(_rawProps, "propName") for children
                    SignalWrapResult::WrapPropNamed(prop_name) => {
                        let source = ctx.ast.expression_identifier(SPAN, "_rawProps");
                        let wrapped = import_rewrite::build_wrap_prop_call_named(source, &prop_name, ctx);
                        tracker.needs_wrap_prop = true;
                        child_exprs.push(wrapped);
                        continue;
                    }
                    SignalWrapResult::None => {}
                }
            }
            // 3. NEW: _fnSignal for reactive expressions in children
            if !is_call_on_value(&other) && !contains_function_call(&other) {
                let (deps, has_non_reactive) = collect_reactive_deps(
                    &other, destructured_props, module_imports
                );
                if !deps.is_empty() && !has_non_reactive {
                    let (wrapped, fn_code, str_code) = build_fn_signal_wrapping(
                        other, &deps, destructured_props, tracker, ctx,
                    );
                    tracker.needs_fn_signal = true;
                    hoisted_stmts.push((fn_code, str_code));
                    child_exprs.push(wrapped);
                    continue;
                }
            }
            child_exprs.push(other);
        }
    }
}
```

### Example 2: Loop tracking state (to add)
Source: SWC transform.rs:127-128, 2786-2977.
```rust
// In transform.rs QwikTransform struct, add:
/// Depth counter for nested loops. When > 0, QRL calls are hoisted.
loop_depth: u32,

/// Stack of iteration variables for each loop scope.
/// Each entry contains the iteration variables for that loop level.
iteration_var_stack: Vec<Vec<String>>,

/// Stack of hoisted QRL declarations for each block scope.
/// Each Vec contains (symbol_name, qrl_call_code) pairs.
hoisted_qrls: Vec<Vec<(String, String)>>,
```

### Example 3: QRL hoisting (to add)
Source: SWC transform.rs:985-1051.
```rust
// When creating a QRL call for a JSX event handler inside a loop:
fn hoist_qrl_if_needed(&mut self, qrl_call: Expression, symbol_name: &str) -> Expression {
    if self.loop_depth == 0 || self.hoisted_qrls.is_empty() {
        return qrl_call; // Not in a loop, use inline
    }
    let depth = self.hoisted_qrls.len() - 1;
    // Check if already hoisted at this depth
    if let Some(existing) = self.hoisted_qrls[depth].iter().find(|(name, _)| name == symbol_name) {
        // Return reference to existing hoisted variable
        return ctx.ast.expression_identifier(SPAN, symbol_name);
    }
    // Hoist: store for later injection
    self.hoisted_qrls[depth].push((symbol_name.to_string(), /*serialized call*/));
    ctx.ast.expression_identifier(SPAN, symbol_name)
}
```

### Example 4: q:p injection (to add)
Source: SWC transform.rs:1748-1796.
```rust
// In JSX element transform, after processing event handlers:
// Check which iteration variables the handler uses
if self.loop_depth > 0 && !added_iter_var_prop {
    let used_iter_vars: Vec<String> = self.iteration_var_stack.last()
        .map(|vars| vars.iter()
            .filter(|var| handler_uses_variable(handler_expr, var))
            .cloned()
            .collect())
        .unwrap_or_default();

    if !used_iter_vars.is_empty() {
        let (prop_name, value) = if used_iter_vars.len() == 1 {
            ("q:p", identifier_expr(used_iter_vars[0]))
        } else {
            ("q:ps", array_of_identifiers(used_iter_vars))
        };
        var_props.push((prop_name.to_string(), value));
        added_iter_var_prop = true;
    }
}
```

## State of the Art

| Old Approach (OXC current) | Current Approach (SWC reference) | Impact |
|---------------------------|----------------------------------|--------|
| _fnSignal only for JSX attributes | _fnSignal for both attributes AND children | ~57 snapshot diffs |
| _wrapProp only for .value in children | _wrapProp for both .value AND named props in children | ~42 snapshot diffs |
| No loop tracking | loop_depth + iteration_var_stack | ~22 snapshot diffs (q:p) |
| QRL calls inlined at usage site | QRL calls hoisted to variables in loop contexts | ~40 snapshot diffs |
| _restProps with only destructured keys | _restProps with all prop names including defaults | Some diffs in props_optimization |

## Detailed Diff Analysis

### Breakdown by Issue Type
From the snapshot diff analysis (160 files, 3734 insertions, 7172 deletions):

**_fnSignal (154 diff lines mentioning it, ~57 snapshots):**
- Missing in JSX children expressions
- Missing for complex reactive expressions (ternary, binary, object literals)
- Hoisted function declarations (_hfN, _hfN_str) not generated in segment modules
- Example: `panelStore.active ? "yes" : "no"` should become `_fnSignal(_hf0, [panelStore], _hf0_str)`

**_wrapProp (115 diff lines mentioning it, ~42 snapshots):**
- Missing for children: `signal.value` in children not wrapped with `_wrapProp(signal)`
- Missing for named props in children: `_rawProps.fromProps` not wrapped
- Missing for inline component props: `props.id` not wrapped with `_wrapProp(props, "id")`
- Example: `props.id` in children should become `_wrapProp(props, "id")`

**q:p injection (20 diff lines, ~22 snapshots via paramNames):**
- No loop/iteration tracking in OXC
- No `q:p` prop added to elements with event handlers in loops
- No parameter injection into event handler lambdas (_, _1, iterVar)
- Segment paramNames missing the injected parameters

**QRL hoisting (embedded in above counts):**
- QRL calls inlined in JSX attribute values instead of hoisted to variables
- Missing `const SegmentName = /* @__PURE__ */ qrl(...)` declarations
- Hoisted QRLs should be inserted after last VarDecl in enclosing block

### Overlap with Phase 5/6
Some of the 160 diff files have Phase 4 AND Phase 5/6 issues. After Phase 4 is complete:
- Phase 5 (JSX keys/flags) diffs should be isolated to key values and flag numbers
- Phase 6 (import ordering) diffs should be limited to import statement ordering
- Phase 4 completing the signal transforms will also fix many "missing import" diffs (I2) as a side effect, since `_fnSignal`, `_wrapProp`, `_restProps` imports will be correctly added

## Open Questions

1. **_fnSignal expression length limit**: SWC has a 150-char limit on the rendered expression string before it gives up on wrapping. The OXC code doesn't currently enforce this limit. Need to verify if any test cases exercise this edge case.
   - What we know: SWC checks `rendered_expr.len() > 150` in `convert_inlined_fn`
   - What's unclear: Whether any of the 162 test fixtures produce expressions exceeding 150 chars
   - Recommendation: Implement the limit to match SWC, test against snapshots

2. **Props destructuring completeness**: The SWC `props_destructuring.rs` handles additional patterns the OXC version may not:
   - Default values with const expressions: `{ count, some = 3 }` -> includes `some` in excluded_keys with `_rawProps.some ?? 3`
   - Nested destructuring: `{ stuff: { hey } }` -> SWC falls back to non-transform (skip = true)
   - `use*()` return value destructuring: `const { value } = useSignal(0)` -> SWC optimizes this
   - What we know: OXC's `analyze_props_destructuring` handles basic ObjectPattern and rest params
   - What's unclear: How many test cases exercise the advanced patterns
   - Recommendation: Compare OXC's output for `example_props_optimization` fixture to identify specific gaps

3. **should_extract_single_qrl_2 dedup naming**: This was deferred from Phase 3 -- the dedup suffix `_1` is assigned to the wrong segment due to bottom-up traverse order. This may or may not be addressed by Phase 4 changes.
   - What we know: It's a single snapshot diff caused by traverse order
   - What's unclear: Whether loop tracking changes affect the traverse order
   - Recommendation: Defer to Phase 6 cleanup unless it naturally resolves

## Implementation Plan Recommendation

### Plan 1: _fnSignal + _wrapProp for JSX Children (~40% of Phase 4 diffs)
**Scope:** Extend `transform_jsx_children()` in `jsx_transform.rs` to apply the same signal detection and wrapping that already works for JSX attributes.
**Changes:**
- Add `_wrapProp(_rawProps, "propName")` for `WrapPropNamed` in children (currently only handles `WrapPropSignal`)
- Add `_fnSignal` wrapping for complex reactive expressions in children (copy attribute-level logic)
- Ensure hoisted function stmts flow through correctly to segment modules
**Estimated diffs fixed:** ~50-60 snapshots

### Plan 2: Loop Tracking + q:p Injection + QRL Hoisting (~35% of Phase 4 diffs)
**Scope:** Add loop/iteration infrastructure to `transform.rs` and integrate with JSX event handler processing.
**Changes:**
- Add `loop_depth`, `iteration_var_stack`, `hoisted_qrls` state to `QwikTransform`
- Implement enter/exit for `ForStatement`, `ForInStatement`, `ForOfStatement`, `WhileStatement`
- Detect `.map()` / `.filter()` / etc. in `enter_call_expression` and extract callback params
- Implement `hoist_qrl_if_needed()` and `inject_hoisted_qrls_into_block()`
- Add `q:p`/`q:ps` injection in JSX element transform
- Transform event handler lambdas to add iteration variable parameters
- Update segment `paramNames` metadata
**Estimated diffs fixed:** ~30-40 snapshots

### Plan 3: Props Destructuring Completeness (~25% of Phase 4 diffs)
**Scope:** Audit and fix `props_destructuring.rs` to handle all SWC patterns.
**Changes:**
- Ensure `_restProps` excluded_keys includes ALL destructured prop names (including those with defaults)
- Handle default value expressions: `{ some = 3 }` -> `_rawProps.some ?? 3`
- Handle skip cases matching SWC (nested destructuring, non-const defaults)
- Verify `_rawProps` reference rewriting handles all expression types in component bodies
**Estimated diffs fixed:** ~20-30 snapshots

## Sources

### Primary (HIGH confidence)
- SWC optimizer source code: `crates/swc-optimizer/core/src/transform.rs` (3805 lines) - main transform logic, loop tracking, QRL hoisting, q:p injection
- SWC optimizer source code: `crates/swc-optimizer/core/src/inlined_fn.rs` (294 lines) - `convert_inlined_fn` (_fnSignal generation)
- SWC optimizer source code: `crates/swc-optimizer/core/src/props_destructuring.rs` (520 lines) - props transform, _restProps
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/jsx_transform.rs` (1643 lines) - current JSX transform with partial signal wrapping
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/transform.rs` (2828 lines) - current transform state
- OXC optimizer source code: `crates/qwik-optimizer-oxc/src/props_destructuring.rs` (533 lines) - current props handling
- Snapshot diff analysis: `git diff -- crates/qwik-optimizer-oxc/tests/snapshots/` (160 files, 3734+/7172- lines)

### Secondary (MEDIUM confidence)
- STATE.md Phase 4 triage data: 42x _wrapProp, 57x _fnSignal, 22x q:p injection
- ISSUES.md M1-M4 issue descriptions

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All code is in the same Rust crate, no external dependencies
- Architecture: HIGH - SWC reference code is directly accessible and thoroughly analyzed
- Pitfalls: HIGH - Derived from actual SWC code review and snapshot diff analysis
- _fnSignal wrapping: HIGH - OXC already has working attribute-level implementation to copy
- Loop tracking: HIGH - SWC implementation is clear and well-structured
- Props destructuring: MEDIUM - Some edge cases need snapshot-level verification

**Research date:** 2026-02-20
**Valid until:** Indefinite (SWC reference and OXC source are version-controlled)
