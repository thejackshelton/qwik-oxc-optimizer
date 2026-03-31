# Phase 1: Naming & Display Names - Research

**Researched:** 2026-02-19
**Domain:** Rust AST transformer naming logic (SWC parity)
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- Pure correctness work -- SWC snapshots fully define the target behavior
- No user decisions needed; the spec is the snapshot golden reference
- Study SWC source (`collector.rs`, `code_move.rs`) to understand naming algorithms
- Compare snapshot diffs to identify exactly where OXC diverges

### Claude's Discretion
- Investigation order (which naming issue to tackle first)
- Implementation approach (how to restructure OXC naming logic)
- Whether to fix all three naming issues together or in separate passes

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope.
</user_constraints>

## Summary

This phase addresses the three most pervasive naming mismatches between OXC and SWC optimizer output, affecting 120+ snapshots. The root cause is an **architectural difference** in how the two implementations track scope context during AST traversal:

- **SWC** uses a `stack_ctxt: Vec<String>` that naturally accumulates ALL scope elements (variable names, function names, JSX element names, attribute names) as the visitor descends the AST. Display names are built by `self.stack_ctxt.join("_")` followed by `escape_sym()` which normalizes non-alphanumeric chars. This approach captures full scope context automatically.

- **OXC** uses a two-pass architecture where the **collector** pre-computes display names using explicit functions (`derive_display_name()`, `derive_jsx_event_display_name()`) that track only limited context (`current_var_name`, `scope_prefix`, `wrapper_callee_name`). The **transform** then looks up these pre-computed names. This loses intermediate scope context that SWC's stack naturally captures.

The fix requires either (a) introducing a `stack_ctxt`-equivalent in OXC's transform pass, or (b) enriching the collector with full scope tracking. Option (a) is recommended because the transform already walks the AST and can accumulate context naturally.

**Primary recommendation:** Introduce a `stack_ctxt: Vec<String>` into OXC's transform pass that mirrors SWC's scope accumulation, then build display names from `stack_ctxt.join("_")` + `escape_sym()` at segment creation time. Fix the parent field to use `segment_name` (with hash) instead of `display_name`.

## Architecture Patterns

### SWC's stack_ctxt Approach (the Reference)

SWC's `QwikTransform` maintains a `stack_ctxt: Vec<String>` that is pushed/popped at each scope boundary:

```
Location                    | What's pushed to stack_ctxt
----------------------------|--------------------------------------------
fold_var_declarator         | Variable name (e.g., "renderHeader")
fold_fn_decl                | Function name (e.g., "loopArrowFn")
fold_class_decl             | Class name
fold_export_default_expr    | File name or folder name
fold_jsx_element            | JSX element tag name (e.g., "div", "button")
fold_jsx_attr (native elem) | Transformed event name (e.g., "q-e:click")
fold_jsx_attr (component)   | Original attr name (e.g., "onClick$")
fold_jsx_attr (non-event)   | Original attr name (e.g., "custom$")
fold_call_expr (marker fn)  | Callee name (e.g., "component$")
fold_call_expr (other)      | Callee name for context
handle_jsx (post-transform) | Element name from jsx() first arg
handle_jsx_props_obj        | Property key (e.g., "q-e:click" or "host:onClick$")
```

At segment creation time: `display_name = escape_sym(self.stack_ctxt.join("_"))`

The `escape_sym` function:
1. Replaces all non-alphanumeric chars with `_`
2. Squashes consecutive underscores
3. Trims leading/trailing underscores

So `["Foo", "component$", "div", "q-e:click"]` becomes `Foo_component_div_q_e_click`.

### SWC's segment_stack for Parent Field

SWC maintains a separate `segment_stack: Vec<Atom>` that tracks the **symbol_name** (segment name WITH hash, e.g., `renderHeader_XXXXXXXXXXXX`) of each enclosing segment:

```rust
// Before processing child AST of a segment:
self.segment_stack.push(symbol_name.clone());  // e.g., "Foo_component_XXXXXXXXXXXX"
let folded = first_arg.fold_with(self);
self.segment_stack.pop();

// When creating a new segment:
parent_segment: self.segment_stack.last().cloned(),  // Contains hash!
```

### SWC's Deduplication Counter

When the same display name occurs multiple times (e.g., two `$()` calls both named `Foo_component`), SWC appends `_1`, `_2`, etc.:

```rust
let index = match self.segment_names.get_mut(&display_name) {
    Some(count) => { *count += 1; *count }
    None => 0,
};
if index == 0 {
    self.segment_names.insert(display_name.clone(), 0);
} else {
    write!(display_name, "_{}", index).unwrap();
}
```

This is how SWC produces names like `Foo_component_1` for a second `$()` call inside the same component scope.

### OXC's Current Architecture (What Needs to Change)

OXC currently:
1. **Collector** pre-computes display names using `derive_display_name()` and `derive_jsx_event_display_name()`
2. **Transform** looks up pre-computed names by matching span positions
3. Uses `dollar_call_stack: Vec<String>` storing **display_name** (not segment name)
4. No deduplication counter for display names
5. No `escape_sym()` equivalent

### Recommended Architecture Change

Add to OXC's `QwikTransformer`:
```rust
struct QwikTransformer {
    // NEW: scope context stack (mirrors SWC's stack_ctxt)
    stack_ctxt: Vec<String>,
    // NEW: segment name stack for parent field (mirrors SWC's segment_stack)
    segment_stack: Vec<String>,
    // NEW: deduplication counter (mirrors SWC's segment_names)
    segment_names: HashMap<String, u32>,
    // KEEP existing fields...
}
```

Push/pop `stack_ctxt` in OXC's traverse callbacks:
- `enter_variable_declarator` -> push variable name
- `exit_variable_declarator` -> pop
- `enter_function` (named) -> push function name
- `exit_function` -> pop
- When entering JSX element -> push element tag name
- When entering JSX attribute -> push attribute name (transformed for native event handlers)
- When entering call expression (named function) -> push callee name

Build display name at segment creation: `escape_sym(stack_ctxt.join("_"))`.

## Issue-by-Issue Analysis

### N1: Event Handler Naming (`onClick` vs `q_e_click`)

**Root cause:** OXC's `transform_attr_name_for_display()` in the collector converts ALL event-like attributes to `q_e_*` format, including when a bare `$()` is used inside a regular `onClick` attribute. SWC only transforms to `q_e_*` when the attribute name passes through `jsx_event_to_html_attribute()` which requires the attribute to end with `$`.

**What SWC does for `onClick={$(...)}` (bare `$()` in non-$ attr):**
- `fold_jsx_attr` for `onClick`: `convert_qrl_word("onClick")` returns `None` (no `$` suffix)
- Pushes `"onClick"` to stack_ctxt (original name)
- `$()` inside creates segment with display name containing `onClick`

**What SWC does for `onClick$={...}` ($-suffixed attr on native element):**
- `fold_jsx_attr`: `convert_qrl_word("onClick$")` returns `Some("onClickQrl")`
- `jsx_event_to_html_attribute("onClick$")` returns `Some("q-e:click")`
- Pushes `"q-e:click"` to stack_ctxt (transformed name)
- After `escape_sym`: `q_e_click`

**What SWC does for `host:onClick$={...}` (host-prefixed):**
- Falls into the `JSXNamespacedName` branch
- Pushes `"host-onClick$"` (with `ns-name` format) to stack_ctxt
- After `escape_sym`: `host_onClick` (dollar gets squeezed)

**Fix:** Stop pre-computing event name transformations in the collector. Instead, use the `stack_ctxt` approach in the transform where the correct transformation is applied contextually.

**Snapshot examples:**
- SWC: `renderHeader_div_onClick` vs OXC: `renderHeader_div_q_e_click` (example_1)
- SWC: `Header_component_Header_onClick` vs OXC: `Header_component___q_e_click` (example_3)

### N2: Display Name Context Gaps

**Root cause:** OXC's collector-based display name derivation tracks only `current_var_name`, `scope_prefix`, `wrapper_callee_name`, and `parent_display_name`. It does NOT track:
- JSX element names (no `<div>` context)
- Intermediate function names inside `$()` bodies (e.g., `loopArrowFn`)
- The `component` suffix for nested `$()` inside `component$()` body
- Proper deduplication counters for repeated names

**Missing scope elements observed:**
1. **JSX elements dropped**: `App_component_div_button_q_e_click` -> `App_component_button_q_e_click` (missing `div`)
2. **Function names dropped**: `App_component_loopArrowFn_span_q_e_click` -> `App_component_span_q_e_click` (missing `loopArrowFn`)
3. **Component context dropped for nested $()**: `App_component_useStyles` -> `App_useStyles` (missing `component`)
4. **Dedup counter missing**: `Foo_component_1` -> `Foo` (missing `component_1` suffix)
5. **Parent scope dropped**: `App_Header_component` -> `Header_component` (missing `App`)
6. **Default export naming**: `test_component` -> `default_component` (should use file stem, not "default")

**Fix:** The `stack_ctxt` approach in the transform naturally solves all of these because:
- Each JSX element pushes its tag name
- Each function declaration pushes its name
- Each variable declaration pushes its name
- The full chain is preserved through `join("_")`
- Deduplication counter handles repeated names

### N3: Parent Field Format

**Root cause:** OXC pushes `segment.display_name` (e.g., `"test.tsx_renderHeader"`) to `dollar_call_stack`, and uses this as the parent field. SWC pushes `symbol_name` (e.g., `"renderHeader_XXXXXXXXXXXX"`) to `segment_stack`, and uses `segment_stack.last()` as the parent field.

**What SWC uses as parent:**
```
"parent": "renderHeader_XXXXXXXXXXXX"     // segment name WITH hash
"parent": "Foo_component_XXXXXXXXXXXX"    // segment name WITH hash
"parent": "Foo_component_1_XXXXXXXXXXXX"  // segment name WITH hash (deduped)
```

**What OXC uses as parent:**
```
"parent": "test.tsx_renderHeader"          // DISPLAY name (includes filename)
"parent": "test.tsx_Foo_component"         // DISPLAY name (includes filename)
"parent": "test.tsx_Foo"                   // DISPLAY name (missing component, no dedup)
```

**Fix:** Change `dollar_call_stack` to store `segment_name` (the `{display_name}_{hash}` format, matching SWC's `segment_stack`). When creating a new segment, set `parent = self.segment_stack.last()`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Display name construction | Custom derivation functions per context type | `stack_ctxt.join("_")` + `escape_sym()` | SWC's approach handles all cases uniformly |
| Scope tracking | Separate tracking for var, fn, JSX, attr contexts | Single `stack_ctxt` push/pop in each traverse callback | Avoids maintaining multiple context types |
| Name deduplication | Ad-hoc naming with counters | `HashMap<String, u32>` exactly like SWC | Handles all edge cases (multiple `$()` in same scope) |

**Key insight:** SWC's `stack_ctxt` design is elegant because it doesn't need to understand naming rules -- it just accumulates ALL scope names. The display name emerges from joining them. Trying to replicate this with explicit logic per context type (what OXC currently does) is fragile and always misses edge cases.

## Common Pitfalls

### Pitfall 1: Two-Pass Name Computation
**What goes wrong:** Computing display names in the collector (pass 1) and using them in the transform (pass 2) creates a mismatch because the collector doesn't have the full AST traversal context available to the transform.
**Why it happens:** The collector was designed for simpler analysis before the naming requirements were fully understood.
**How to avoid:** Move display name computation entirely to the transform pass where full AST context is available. The collector should only identify dollar call SITES (spans), not compute their names.
**Warning signs:** Display names missing intermediate scope elements.

### Pitfall 2: Event Name Transformation in Wrong Place
**What goes wrong:** Applying `onClick` -> `q_e_click` transformation unconditionally to all event-like attributes, even when the attribute doesn't end with `$`.
**Why it happens:** The collector doesn't distinguish between `onClick={$(...)}` (bare dollar) and `onClick$={...}` (dollar-suffixed attribute).
**How to avoid:** Only apply event name transformation when the attribute name ends with `$` AND is on a native element. Use `stack_ctxt` which naturally pushes the correct name based on context.
**Warning signs:** `q_e_click` appearing in names where SWC uses `onClick`.

### Pitfall 3: Parent Field Using Wrong Identifier
**What goes wrong:** Using display_name (e.g., `test.tsx_renderHeader`) as parent instead of segment name (e.g., `renderHeader_XXXXXXXXXXXX`).
**Why it happens:** Conflating `dollar_call_stack` (display names) with `segment_stack` (segment names with hash).
**How to avoid:** Maintain a separate `segment_stack` that stores segment names (display_name + hash format) and use it for the parent field.
**Warning signs:** Parent field containing filename prefix or missing hash.

### Pitfall 4: Missing escape_sym
**What goes wrong:** Display names contain characters like `-`, `:`, `$` which are not valid in JavaScript identifiers.
**Why it happens:** OXC doesn't have an equivalent of SWC's `escape_sym()`.
**How to avoid:** Port `escape_sym` from SWC -- replace non-alphanumeric chars with `_`, squash consecutive underscores, trim leading/trailing underscores.
**Warning signs:** Names containing hyphens, colons, or dollar signs.

### Pitfall 5: Missing Deduplication Counter
**What goes wrong:** Two `$()` calls in the same scope get the same display name, causing hash collisions.
**Why it happens:** OXC doesn't track display name usage counts.
**How to avoid:** Port SWC's `segment_names: HashMap<String, u32>` approach. On first use, insert with count 0. On subsequent uses, increment count and append `_N`.
**Warning signs:** Multiple segments with identical display names (before hash), or missing `_1`, `_2` suffixes.

### Pitfall 6: OXC Traverse vs SWC Fold Ordering
**What goes wrong:** OXC's `oxc_traverse` visits nodes in a different order than SWC's `fold_*` methods, potentially affecting when stack_ctxt is pushed/popped relative to child processing.
**Why it happens:** OXC traverse uses explicit `enter_*/exit_*` callbacks while SWC uses `fold_children_with(self)` which gives precise control over when the fold occurs relative to child processing.
**How to avoid:** Carefully ensure that `stack_ctxt.push()` happens in `enter_*` and `stack_ctxt.pop()` happens in `exit_*` for each scope-introducing AST node. Test with complex nesting patterns.
**Warning signs:** Display names with elements in wrong order or at wrong nesting level.

## Code Examples

### SWC's escape_sym (port this to OXC)
```rust
// Source: crates/swc-optimizer/core/src/transform.rs:3320
fn escape_sym(str: &str) -> String {
    str.chars()
        .flat_map(|x| match x {
            'A'..='Z' | 'a'..='z' | '0'..='9' => Some(x),
            _ => Some('_'),
        })
        .fold((String::new(), None), |(mut acc, prev), x| {
            if x == '_' {
                if prev.is_none() {
                    (acc, None)
                } else {
                    (acc, Some('_'))
                }
            } else {
                if prev == Some('_') {
                    acc.push('_');
                }
                acc.push(x);
                (acc, Some(x))
            }
        })
        .0
}
```

### SWC's register_context_name (core naming logic)
```rust
// Source: crates/swc-optimizer/core/src/transform.rs:323
fn register_context_name(&mut self, custom_symbol: Option<Atom>) -> (Atom, Atom, Atom, u64) {
    // 1. Join stack context
    let mut display_name = self.stack_ctxt.join("_");
    if self.stack_ctxt.is_empty() {
        display_name += "s_";
    }
    // 2. Escape non-alphanumeric chars
    display_name = escape_sym(&display_name);
    // 3. Ensure valid identifier start
    let first_char = display_name.chars().next();
    if first_char.is_some_and(|c| c.is_ascii_digit()) {
        display_name = format!("_{}", display_name);
    }
    // 4. Deduplicate
    let index = match self.segment_names.get_mut(&display_name) {
        Some(count) => { *count += 1; *count }
        None => 0,
    };
    if index == 0 {
        self.segment_names.insert(display_name.clone(), 0);
    } else {
        write!(display_name, "_{}", index).unwrap();
    }
    // 5. Hash
    let mut hasher = DefaultHasher::new();
    // ... hash computation ...
    // 6. Build symbol_name and full display_name
    let symbol_name = format!("{}_{}", display_name, hash64);
    display_name = format!("{}_{}", &self.options.path_data.file_name, display_name);
    // Note: file_name is prepended AFTER dedup counter and AFTER hash computation
    (Atom::from(symbol_name), Atom::from(display_name), Atom::from(hash64), hash)
}
```

Key ordering observation: The hash is computed on `display_name` BEFORE the filename prefix is prepended. The filename prefix is added to display_name AFTER hashing. The symbol_name also does NOT include the filename prefix.

### SWC's fold_jsx_element (scope push for elements)
```rust
// Source: crates/swc-optimizer/core/src/transform.rs:2996
fn fold_jsx_element(&mut self, node: ast::JSXElement) -> ast::JSXElement {
    let (stacked, is_native) = if let ast::JSXElementName::Ident(ref ident) = node.opening.name {
        let is_native_element = ident.sym.chars().next().is_some_and(|c| c.is_lowercase());
        self.stack_ctxt.push(ident.sym.to_string());
        self.jsx_element_is_native.push(is_native_element);
        (true, true)
    } else {
        (false, false)
    };
    let o = node.fold_children_with(self);
    if stacked { self.stack_ctxt.pop(); }
    if is_native { self.jsx_element_is_native.pop(); }
    o
}
```

### SWC's fold_jsx_attr (scope push for attributes)
```rust
// Source: crates/swc-optimizer/core/src/transform.rs:3037
fn fold_jsx_attr(&mut self, node: ast::JSXAttr) -> ast::JSXAttr {
    match node.name {
        ast::JSXAttrName::Ident(ref ident) => {
            let is_native = self.jsx_element_is_native.last().copied().unwrap_or(false);
            if is_native {
                if let Some(html_attr) = jsx_event_to_html_attribute(&ident.sym) {
                    // NATIVE + EVENT: push TRANSFORMED name (e.g., "q-e:click")
                    self.stack_ctxt.push(html_attr.to_string());
                } else {
                    // NATIVE + NON-EVENT: push ORIGINAL name
                    self.stack_ctxt.push(ident.sym.to_string());
                }
            } else {
                // COMPONENT: push ORIGINAL name (never transform)
                self.stack_ctxt.push(ident.sym.to_string());
            }
        }
        ast::JSXAttrName::JSXNamespacedName(ref ns) => {
            // NAMESPACED: push "ns-name" format (e.g., "host-onClick$")
            self.stack_ctxt.push(format!("{}-{}", ns.ns.sym, ns.name.sym));
        }
    }
    // ... process attribute value (may create segment) ...
    self.stack_ctxt.pop();
}
```

## Implementation Strategy Recommendation

### Recommended Order: Fix All Three Together

The three issues share a root cause (inadequate scope tracking) and the fix is a unified architectural change. Fixing them separately would require intermediate refactors that get thrown away. Therefore:

**Phase 1 implementation should:**

1. **Add `stack_ctxt`, `segment_stack`, `segment_names` to OXC's QwikTransformer**
2. **Add push/pop calls in traverse callbacks:**
   - `enter_variable_declarator` / `exit_variable_declarator`
   - `enter_function` (for named functions) / `exit_function`
   - When entering/exiting JSX elements during JSX event handler processing
   - When entering/exiting JSX attributes during JSX event handler processing
   - When entering/exiting call expressions for `$`-suffixed callees
3. **Port `escape_sym` from SWC**
4. **Replace `derive_display_name_for_call()` with `stack_ctxt.join("_")` + `escape_sym()`**
5. **Replace `derive_jsx_event_display_name()` with the same stack-based approach**
6. **Add deduplication counter logic**
7. **Change parent field to use segment name (with hash) from `segment_stack`**

### Critical Implementation Detail: JSX Processing Path

OXC handles JSX event handlers differently from SWC. In OXC:
- JSX event handler `$`-suffixed attributes are processed during the transform's JSX element traversal
- The transform walks JSX elements looking for `$`-suffixed attributes and creates segments for them
- This happens in `create_jsx_event_segments_in_children()` and related methods

The `stack_ctxt` must be pushed/popped correctly during this JSX processing. Specifically:
- When entering a JSX element: push element tag name
- When processing a JSX attribute: push the appropriate attribute name
- When the segment is created: `stack_ctxt.join("_")` gives the correct display name
- After processing: pop the attribute name, then pop the element name

### Critical Implementation Detail: Collector's Role After Changes

After moving display name construction to the transform, the collector's `DollarCallSite.display_name` field becomes unnecessary for naming purposes. However, the collector is still needed for:
- Identifying which imports are `$`-suffixed (for recognizing dollar calls)
- Recording call site spans (for matching during transform)
- Tracking nesting relationships (for parent detection)
- Module-level declaration tracking (for capture analysis)

The collector's `derive_display_name()` and `derive_jsx_event_display_name()` can be removed or simplified once the transform handles naming.

## Open Questions

1. **OXC Traverse Callback Granularity**
   - What we know: OXC's `oxc_traverse` provides `enter_*/exit_*` for many AST node types (expressions, statements, declarations).
   - What's unclear: Does it provide callbacks for JSX element enter/exit and JSX attribute enter/exit that fire at the right granularity? The JSX event handler processing currently happens in custom walk methods, not traverse callbacks.
   - Recommendation: Check available traverse callbacks for JSX. If not available, the `stack_ctxt` push/pop for JSX context may need to happen inside the existing custom JSX walk methods rather than via traverse callbacks.

2. **Default Export File Stem Naming**
   - What we know: SWC uses the file stem (or folder name for index files) as the display name for default exports. OXC uses `"default"`.
   - What's unclear: Exact conditions for when folder name is used vs file stem.
   - Recommendation: Port SWC's `fold_export_default_expr` logic which checks if `file_stem == "index"` and uses folder name in that case.

3. **Hash Computation Input**
   - What we know: SWC hashes `scope + rel_path + display_name` where `display_name` does NOT include the filename prefix.
   - What's unclear: Whether OXC currently includes the filename prefix in its hash input.
   - Recommendation: Verify that OXC's `compute_segment_hash` receives the display_name WITHOUT filename prefix, matching SWC's algorithm.

## Sources

### Primary (HIGH confidence)
- `crates/swc-optimizer/core/src/transform.rs` -- Full SWC naming logic: `stack_ctxt`, `register_context_name`, `escape_sym`, `fold_jsx_element`, `fold_jsx_attr`, `fold_var_declarator`, `fold_fn_decl`, `segment_stack`
- `crates/qwik-optimizer-oxc/src/collector.rs` -- OXC collector: `derive_display_name`, `derive_jsx_event_display_name`, `transform_attr_name_for_display`
- `crates/qwik-optimizer-oxc/src/transform.rs` -- OXC transform: `derive_display_name_for_call`, `derive_jsx_event_display_name`, `dollar_call_stack`, `record_segment`
- `crates/qwik-optimizer-oxc/src/hash.rs` -- OXC hash computation
- Git-committed snapshot files -- SWC golden reference output
- Git working tree snapshot diffs -- OXC current output deviations

## Metadata

**Confidence breakdown:**
- Architecture analysis: HIGH -- Direct source code comparison between SWC and OXC
- N1 root cause: HIGH -- Verified through SWC code trace and snapshot diffs
- N2 root cause: HIGH -- Verified through SWC stack_ctxt accumulation and snapshot diffs
- N3 root cause: HIGH -- Verified through SWC segment_stack vs OXC dollar_call_stack
- Implementation strategy: MEDIUM -- OXC traverse callback availability for JSX needs verification
- Pitfalls: HIGH -- Derived from actual snapshot failures

**Research date:** 2026-02-19
**Valid until:** Indefinite (SWC reference is stable, codebase is local)
