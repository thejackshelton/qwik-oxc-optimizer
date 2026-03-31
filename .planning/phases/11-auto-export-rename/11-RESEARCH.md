# Phase 11: Auto Export Rename - Research

**Researched:** 2026-02-23
**Domain:** Segment re-export aliasing (_auto_ prefix) in Qwik optimizer SWC-to-OXC port
**Confidence:** HIGH

## Summary

The `_auto_` prefix mechanism is SWC's approach to re-exporting module-level declarations that are referenced inside segment bodies. When a segment function references a module-level declaration (const, function, class) that is NOT already exported by the user, SWC:

1. Adds `export { X as _auto_X }` to the entry module bottom
2. In segment files, imports as `import { _auto_X as X } from "./self-module"`

The OXC port currently handles the self-import mechanism (via `reclassify_module_level_decl_captures`) but does NOT add the `_auto_` prefix or the entry module re-export. This affects 8 test cases.

**Primary recommendation:** Implement `_auto_` export tracking during segment processing, emit `export { X as _auto_X }` statements at the bottom of the entry module, and update self-import emission in `code_move.rs` to use `_auto_` alias. Also fix `self_import_source()` to respect `explicit_extensions`.

## Architecture Patterns

### SWC Mechanism (Reference)

**Source:** `crates/swc-optimizer/core/src/transform.rs` lines 464-472, 732-753, 965-982

SWC's flow:
1. During segment processing (`handle_qsegment`/`fold_expr`), collect `local_idents` -- all identifiers referenced in the segment body expression
2. For each `local_ident`, check two conditions:
   - NOT already in `global_collect.exports` (not user-exported)
   - IS in `global_collect.root` (is a module-level declaration)
3. If both conditions met, call `ensure_export(id)` which:
   - Computes exported name: `format!("_auto_{}", id.0)` (e.g., `_auto_App`)
   - Adds `(id, Some("_auto_App"))` to `global_collect.exports` (idempotent via `Entry::Vacant`)
   - Creates `export { App as _auto_App }` AST node, stores in `extra_bottom_items` BTreeMap
4. In `fold_module` exit, `extra_bottom_items` values are appended at the bottom of the module body

**Source:** `crates/swc-optimizer/core/src/code_move.rs` lines 58-127

In segment code generation:
1. For each `local_ident`, check if it's in `global.exports`
2. If yes, the export's alias (`Some("_auto_X")`) becomes the import specifier
3. Self-module path: `"./{file_stem}"` or `"./{file_name}"` depending on `explicit_extensions`
4. Result: `import { _auto_X as X } from "./self-module"`

### OXC Current State

**Files involved:**
- `transform.rs`: Main transform, `exit_program()` assembles entry module, `reclassify_module_level_decl_captures()` converts module-level decls to self-imports
- `code_move.rs`: `build_segment_code_with_hoisted()` generates segment module code
- `collector.rs`: `module_level_decls` (HashSet) and `module_exports` (Vec<ExportInfo>)
- `types.rs`: `ImportInfo` struct for needed_imports, `ExportInfo` struct

**Current behavior:**
- Module-level decls referenced in segments are correctly reclassified from captures to `needed_imports` self-imports
- Self-imports use plain names: `import { useFoo } from "./test"` (no `_auto_` prefix)
- No `export { X as _auto_X }` statement in entry module
- `self_import_source()` does NOT respect `explicit_extensions`

### Required Changes (4 areas)

#### Area 1: Track exported local names in collector

**Problem:** The current OXC collector does NOT track `export { X }` or `export { X as Y }` specifier exports (without `from` source) in `module_exports`. Only `export const/function/class` declarations are tracked. This means we can't accurately determine "is X already exported?" for the _auto_ check.

**SWC reference:** SWC's `global_collect.exports` is keyed by the LOCAL Id. For `export { Other as App }`, the key is `Other` (the local binding), not `App`. For `export const Root`, the key is `Root`.

**Solution:** Add an `exported_local_names: HashSet<String>` field to `CollectResult`. Populate it with:
- Binding names from `export const/function/class` declarations (already in `module_exports`)
- LOCAL names from `export { X }` and `export { X as Y }` specifiers (currently missing)
- NOT populated from `export default expr` (matches SWC behavior -- `export default App` does not mark `App` as exported)

**Collector code change** (in `collect_named_export`, after the `export.source.is_some()` block):
```rust
// Track local names from specifier exports (export { X } and export { X as Y })
if export.source.is_none() {
    for spec in &export.specifiers {
        let local_name = match &spec.local {
            ModuleExportName::IdentifierName(id) => id.name.as_str().to_string(),
            ModuleExportName::IdentifierReference(id) => id.name.as_str().to_string(),
            ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
        };
        ctx.exported_local_names.insert(local_name);
    }
}
```
Also populate from declaration exports:
```rust
// In the declaration branch, add to exported_local_names too
ctx.exported_local_names.insert(name.clone());
```

#### Area 2: Track _auto_ exports during transform

Add a `auto_exports: HashSet<String>` field to `QwikTransform` to track which names need `_auto_` re-export.

**Population points:**
- In `reclassify_module_level_decl_captures()`: when a module-level decl is converted to self-import, check if it's NOT in `collected.exported_local_names`. If not, add to `auto_exports`.
- In `finalize_segments()` (the pending_qrl_imports handler at line 558): same check for locally-defined Qrl functions that become self-imports.

#### Area 3: Emit _auto_ exports in entry module

In `exit_program()`, after Phase 7 (assemble body), append `export { X as _auto_X }` statements at the bottom of `new_body`.

**Ordering:** SWC uses `BTreeMap<Id, ModuleItem>` which sorts alphabetically by name. Sort `auto_exports` alphabetically before emitting.

**AST construction:** Build `ExportNamedDeclaration` with a single `ExportSpecifier::Named`:
```rust
// local = IdentifierReference("X"), exported = IdentifierName("_auto_X")
// source = None (re-export from current module, not from external)
```

#### Area 4: _auto_ alias in segment self-imports + explicit_extensions fix

**In `code_move.rs`:** When emitting `needed_imports` that are self-imports, check if the specifier name is in an `auto_exports` set. If so, emit `import { _auto_X as X }` instead of `import { X }`.

**Threading:** Pass `auto_exports: &HashSet<String>` as a new parameter to `build_segment_code_with_hoisted()`.

**`self_import_source()` fix:** Currently always strips extension. For `explicit_extensions=true`, keep the full filename:

```rust
fn self_import_source(&self) -> String {
    let basename = self.filename.rsplit('/').next().unwrap_or(&self.filename);
    if self.options.explicit_extensions {
        format!("./{}", basename)
    } else {
        let stem = basename.rsplit('.').last().unwrap_or(basename);
        format!("./{}", stem)
    }
}
```

## Affected Test Cases

8 tests have `_auto_` diffs:

| Test | Strategy | explicit_ext | _auto_ exports | Segment imports |
|------|----------|-------------|----------------|-----------------|
| `example_export_issue` | Smart | no | `App` | `_auto_App as App` from `"./test"` |
| `example_invalid_references` | Smart | no | `I1`-`I10` (10 names) | `_auto_I1 as I1` etc from `"./test"` |
| `example_qwik_react` | Smart | yes | `filterProps` | `_auto_filterProps` from `"./index.qwik.mjs"` |
| `example_qwik_react_inline` | Inline | yes | `filterProps` | N/A (inline, no segment files) |
| `example_reg_ctx_name_segments_hoisted` | Hoist | no | `STYLES` | N/A (hoisted, no segment files) |
| `impure_template_fns` | Smart | no | `useFoo` | `_auto_useFoo` from `"./test"` |
| `relative_paths` | Smart | yes | `useData` | `_auto_useData` from `"./lib.mjs"` |
| `should_split_spread_props_with_additional_prop5` | Smart | no | `Hola` | `_auto_Hola` from `"./test"` |

**Key observation:** `_auto_` exports are added regardless of entry strategy (Inline, Hoist, Smart/Segment). Even when segments are inlined, the entry module gets `export { X as _auto_X }`. Only the segment-side import alias applies to Smart/Segment strategy (separate file segments).

**Note on `relative_paths`:** This test has additional diffs beyond `_auto_` (the `lib.mjs` additional input produces inline-style output instead of segment-style in OXC). Fixing `_auto_` alone may not fully resolve this test's snapshot.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Export AST node construction | Manual AST building | OXC AstBuilder methods | Arena allocation, correct spans |
| Alphabetical sort | Custom comparison | `Vec.sort()` on `auto_exports` iter | Matches SWC's BTreeMap ordering |

## Common Pitfalls

### Pitfall 1: Duplicate _auto_ exports
**What goes wrong:** Same name gets `_auto_` export added multiple times (referenced in 2+ segment bodies)
**Why it happens:** Multiple segments can reference the same module-level decl
**How to avoid:** Use `HashSet<String>` for `auto_exports` -- naturally deduplicates. SWC uses `Entry::Vacant` check in `add_export()`.

### Pitfall 2: Already-exported names getting _auto_ prefix
**What goes wrong:** User-exported names like `export const Root = component$(...)` get spurious `_auto_Root` exports
**Why it happens:** Not checking existing exports before adding _auto_
**How to avoid:** Check `exported_local_names` before adding to `auto_exports`.

### Pitfall 3: explicit_extensions in self_import_source
**What goes wrong:** `relative_paths` and `example_qwik_react` tests fail because self-import path omits extension
**Why it happens:** `self_import_source()` always strips extension regardless of `explicit_extensions`
**How to avoid:** Branch on `self.options.explicit_extensions` in `self_import_source()`.

### Pitfall 4: _auto_ export ordering
**What goes wrong:** Export statements appear in wrong order, causing snapshot diff
**Why it happens:** Using insertion order instead of alphabetical
**How to avoid:** Sort `auto_exports` alphabetically before emitting. SWC's BTreeMap guarantees alphabetical key order.

### Pitfall 5: Inline/Hoist strategy segments also need _auto_ exports
**What goes wrong:** Missing _auto_ exports for inline/hoist strategy tests
**Why it happens:** Assuming _auto_ only applies to Segment strategy
**How to avoid:** The `_auto_` export generation is independent of entry strategy. Both SWC `handle_qsegment` call sites check `should_emit_segment` which does NOT check entry strategy, only `strip_ctx_name` and `strip_event_handlers`.

### Pitfall 6: export default does NOT prevent _auto_
**What goes wrong:** Assuming `export default App` marks `App` as "already exported"
**Why it happens:** `export default <expr>` is treated differently from `export { X }` in SWC
**How to avoid:** SWC's collector only adds to `exports` from `export default function/class` declarations, NOT from `export default <identifier>`. So `export default App` does NOT prevent `_auto_App`.

### Pitfall 7: Specifier-only exports not tracked
**What goes wrong:** `export { X }` or `export { X as Y }` (without `from` source) not tracked as exports, causing incorrect _auto_ generation
**Why it happens:** Current OXC collector only tracks declaration-based exports, not specifier-based local re-exports
**How to avoid:** Add `exported_local_names` HashSet to collector that also captures specifier-based exports. This is Area 1 of the implementation.

## Code Examples

### Entry module _auto_ export emission (in exit_program)

```rust
// After Phase 7 assembly, before program.body = new_body:

// Build sorted _auto_ export statements
let mut auto_export_names: Vec<&String> = self.auto_exports.iter().collect();
auto_export_names.sort();

for name in auto_export_names {
    let auto_name = format!("_auto_{}", name);
    let local = ctx.ast.module_export_name_identifier_reference(
        SPAN, ctx.ast.atom(name.as_str()));
    let exported = ctx.ast.module_export_name_identifier_name(
        SPAN, ctx.ast.atom(auto_name.as_str()));
    let specifier = ctx.ast.export_specifier(SPAN, local, exported, false);
    let specifiers = ctx.ast.vec1(specifier);
    let export_decl = ctx.ast.module_declaration_export_named_declaration(
        SPAN,
        None,        // no declaration
        specifiers,
        None,        // no source
        ImportOrExportKind::Value,
        None,        // no assertions
    );
    new_body.push(Statement::from(export_decl));
}
```

### Segment self-import with _auto_ alias (in code_move.rs)

```rust
// When emitting needed_imports in build_segment_code_with_hoisted:
for import_info in &segment.needed_imports {
    for (idx, spec_name) in import_info.specifiers.iter().enumerate() {
        let kind = import_info.specifier_kinds.get(idx).unwrap_or(&ImportKind::Named);
        let mut imported_name = if matches!(kind, ImportKind::Named) {
            import_info.specifier_aliases.get(spec_name).cloned()
        } else {
            None
        };

        // For self-imports that need _auto_ prefix
        if auto_exports.contains(spec_name) && !import_info.is_qwik_core {
            imported_name = Some(format!("_auto_{}", spec_name));
        }

        imports.push(SegmentImportEntry {
            local_name: spec_name.clone(),
            source: import_info.source.clone(),
            kind: kind.clone(),
            imported_name,
            assertion: import_info.assertion.clone(),
        });
    }
}
```

### Fixed self_import_source

```rust
fn self_import_source(&self) -> String {
    let basename = self.filename.rsplit('/').next().unwrap_or(&self.filename);
    if self.options.explicit_extensions {
        format!("./{}", basename)
    } else {
        let stem = basename.rsplit('.').last().unwrap_or(basename);
        format!("./{}", stem)
    }
}
```

## State of the Art

| Old Approach (current OXC) | New Approach (matching SWC) | Impact |
|---|---|---|
| Self-imports use plain `{ X }` specifiers | Self-imports use `{ _auto_X as X }` aliased specifiers | Segment module import correctness |
| No re-export in entry module | `export { X as _auto_X }` at entry module bottom | Entry module parity |
| `self_import_source` ignores explicit_extensions | Respects explicit_extensions flag | Correct import paths for lib mode |
| Specifier-only exports not tracked | `exported_local_names` tracks all export forms | Correct "already exported" check |

## Open Questions

1. **relative_paths test complexity**
   - What we know: The `relative_paths` test has additional diffs beyond `_auto_` (the `lib.mjs` additional input produces inline-style output instead of segment-style output in OXC). The lib.mjs file has `inlinedQrl`, `useLexicalScope`, `jsx`/`jsxs` which are inline strategy artifacts.
   - What's unclear: Whether fixing `_auto_` alone will resolve this test's snapshot diff
   - Recommendation: Focus on the `_auto_` changes first; additional `relative_paths` diffs may be Phase 12 territory

## Sources

### Primary (HIGH confidence)
- SWC transform.rs: `ensure_export` (line 972), `extra_bottom_items` (line 104/979/2611), `local_idents` loop (lines 464-472, 732-753)
- SWC code_move.rs: self-import with export alias (lines 100-126), `explicit_extensions` path (line 101-105)
- SWC collector.rs: `add_export` (line 117), `exports` HashMap (line 40), `root` HashMap (line 41), `visit_named_export` (line 203-237)
- OXC transform.rs: `exit_program` (line 3072), `reclassify_module_level_decl_captures` (line 771), `finalize_segments` (line 530+), `self_import_source` (line 753)
- OXC code_move.rs: `build_segment_code_with_hoisted` (line 46), `needed_imports` emission (line 239)
- OXC collector.rs: `module_level_decls` population (lines 408-418), `module_exports` population (lines 518-587), specifier export gap (line 569 only handles `export.source.is_some()`)
- 8 SWC reference snapshots verified against OXC .snap.new files

## Metadata

**Confidence breakdown:**
- Mechanism understanding: HIGH - direct SWC source analysis, verified against 8 snapshot pairs
- Entry module changes: HIGH - clear pattern from SWC extra_bottom_items
- Segment module changes: HIGH - clear pattern from SWC code_move.rs
- Collector enhancement: HIGH - identified specific gap in export tracking
- explicit_extensions fix: HIGH - direct SWC code comparison

**Research date:** 2026-02-23
**Valid until:** Indefinite (SWC reference is static)
