# Known Issues — OXC Optimizer vs SWC Snapshots

> **Caution:** This is a preliminary analysis based on snapshot diff review (2026-02-19).
> Each issue listed here reflects our *current understanding* and should be treated with care.
> Before working on any issue, always do a deep research pass first:
>
> 1. Re-examine the specific snapshot diffs to confirm the issue still exists and is correctly described
> 2. Read the SWC optimizer source to understand how it handles the case
> 3. Read the OXC optimizer source to understand the current implementation
> 4. Identify whether the fix is localized or reveals an architectural flaw that needs broader changes
> 5. Check if fixing one issue might resolve or affect others in the list
>
> Some issues may turn out to be non-issues, others may be more complex than described,
> and new issues may be discovered during deeper investigation.

---

## Current Status

**160/162 snapshots have diffs** (~3,900 insertions, ~7,200 deletions vs golden SWC output).

```bash
# 1. Run tests with --accept to overwrite .snap files with current OXC output
cargo insta test -p qwik-optimizer-oxc --accept --test test -- snapshot_all_transforms

# 2. See exactly what differs between SWC (golden) and OXC (current)
git diff -- crates/qwik-optimizer-oxc/tests/snapshots/

# 3. After analyzing diffs, ALWAYS restore golden snapshots before committing
git checkout -- crates/qwik-optimizer-oxc/tests/snapshots/
```

> **⚠️ CRITICAL: NEVER COMMIT OR PUSH SNAPSHOT CHANGES**
> The `.snap` files contain the SWC golden reference output. Always `git checkout` them back before any commit.
> Only commit changes to source code in `crates/qwik-optimizer-oxc/src/`.

> Note: the golden SWC output snapshots were copy/pasted to the OXC `crates/qwik-optimizer-oxc/tests/snapshots/*.snap` snapshots after an oxfmt applied to all snapshots. Re-running the oxc `cargo insta test -p qwik-optimizer-oxc --accept --test test -- snapshot_all_transforms` did not produce new snapshot diffs, so the snapshots formatting is correct.

---

## Verification Sub-Step (apply after every fix)

After each fix, run the snapshot workflow and check for unexpected formatting-only diffs. Both SWC golden snapshots and OXC output go through the same `oxfmt` post-processing, so there should be **zero formatting-only differences**. If you see diffs that look purely like formatting (indentation, line breaks, compact vs expanded objects), it means the underlying AST output changed — investigate the code diff, not the formatting. If formatting diffs persist after restoring the golden snapshots, inform the user.

---

## Priority Plan

> **Rationale for ordering:** Naming first because segment names cascade into filenames,
> import paths, metadata, and QRL strings — fixing them clears the most diff noise.
> Metadata next because it's simple data plumbing with low risk. Bugs before features
> because missing segments (B4) block testing signal wrapping inside them.
> Features (M1-M4) last among code changes because each one naturally fixes
> its corresponding "missing import" (I2) as a side effect.
> Import ordering goes at the very end — once all the right imports exist,
> sorting them is a clean final pass.

### Phase 1 — Naming & Display Names (~120+ files)

Segment names cascade into filenames, import paths, metadata, and QRL strings. Fixing them first clears the most diff noise and makes all other issues easier to diagnose.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 1.1 | N1 | Event handler naming (`onClick` vs `q_e_click`) — ~80+ snapshots |
| 1.2 | N2 | Display name context gaps (missing intermediate scope) — ~120+ snapshots |
| 1.3 | N3 | Parent field format (segment ID vs display name string) — ~100+ snapshots |

### Phase 2 — Metadata (~90+ files)

Simple data plumbing, low risk. Eliminates noise from metadata blocks in ~90+ snapshots.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 2.1 | D1 | `paramNames` missing from segment metadata — ~90+ snapshots |
| 2.2 | D2 | Path field handling (`path` vs `canonicalFilename` prefix) — ~40+ snapshots |

### Phase 3 — Bugs & Correctness

Fix correctness issues before layering new features. Missing segments (B4) block testing other transforms inside those segments.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 3.1 | B1 | TypeScript type annotations not stripped — ~20+ snapshots (**needs research**, see details below) |
| 3.2 | B4 | Missing segments entirely — ~30+ snapshots (prerequisite for testing features inside segments) |
| 3.3 | B3 | Capture names differences (wrong vars, wrong order) — ~50+ snapshots |
| 3.4 | B2 | Component options object omitted (e.g. `tagName`) — ~10+ snapshots |
| 3.5 | B5 | `qwik_router_inline` massive output reduction — 1 snapshot |
| 3.6 | B6 | Comment lines deleted — ~5 snapshots |

### Phase 4 — Missing Features: Signals, Props & QRL Hoisting (runtime correctness)

The meaty transforms. Each one naturally fixes its corresponding "missing import" (I2) as a side effect.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 4.1 | M1, M2 | `_fnSignal` / `_wrapProp` signal reactivity — ~50+ snapshots |
| 4.2 | M3 | Props destructuring transforms — ~30+ snapshots |
| 4.3 | M4 | QRL hoisting (variable vs inline) — ~40+ snapshots |

### Phase 5 — JSX Keys & Flags

Localized to JSX transform logic.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 5.1 | J1 | JSX key generation differences — ~60+ snapshots |
| 5.2 | J2 | JSX immutability flags (wrong values) — ~80+ snapshots |

### Phase 6 — Import Ordering (final cleanup)

Do this last. Once all the right imports exist (from fixing M1-M4 and B1-B6), sorting them to match SWC's order is a clean final pass. I2 (missing/extra imports) is largely a symptom of M1-M4 and B4 — most will resolve naturally.

| Priority | Issues | Rationale |
|----------|--------|-----------|
| 6.1 | I1 | Import statement ordering — ~140+ snapshots |
| 6.2 | I2, I3 | Missing/extra imports and specifier merging — mostly resolved by earlier phases, mop up remainder |
| 6.3 | I4 | Import path differences (relative path handling) — ~20+ snapshots |

---

## Issue Tables

### Naming Issues

These affect segment display names, filenames, and metadata references.

| # | Issue | Affects |
|---|-------|---------|
| N1 | **Event handler naming** — SWC uses original attr name in display name (`div_onClick`) while OXC uses transformed name (`div_q_e_click`) | ~80+ snapshots |
| N2 | **Display name context gaps** — Missing intermediate scope in display names (e.g. `App_component_div_button_q_e_click` → `App_component_button_q_e_click`, drops `div`) | ~120+ snapshots |
| N3 | **Parent field format** — SWC uses segment name with hash (`renderHeader_XXXXXXXXXXXX`), OXC uses display name string (`test.tsx_renderHeader`) | ~100+ snapshots |

### Metadata Issues

| # | Issue | Affects |
|---|-------|---------|
| D1 | **`paramNames` missing** — SWC includes `paramNames` array in segment metadata, OXC omits it | ~90+ snapshots |
| D2 | **Path field handling** — OXC puts path prefix in `canonicalFilename` with empty `path`; SWC puts it in `path` field | ~40+ snapshots |

### Bugs

| # | Issue | Affects |
|---|-------|---------|
| B1 | **TypeScript types not stripped** — TS type annotations preserved in JS output when `transpile_ts=true` (e.g. `(props: Stuff)`). Currently `transpile_ts` only changes the output file extension but doesn't actually remove type annotations from the AST. The `transformer` feature is intentionally excluded from `Cargo.toml`. **Needs research** to decide the approach: (a) enable the `transformer` feature and run `oxc_transformer` with TS-only config, (b) use `oxc_isolated_declarations`, or (c) strip types manually during the custom traversal. Each has trade-offs in binary size, complexity, and correctness. | ~20+ snapshots |
| B2 | **Component options object omitted** — `componentQrl()` second arg (e.g. `{ tagName: "my-foo" }`) dropped | ~10+ snapshots |
| B3 | **Capture names differences** — Wrong captured variables or different capture order in metadata | ~50+ snapshots |
| B4 | **Missing segments** — OXC creates fewer segment files than SWC for some inputs | ~30+ snapshots |
| B5 | **`qwik_router_inline` drastically simplified** — Entire complex transformation skipped (~2100 lines diff) | 1 snapshot |
| B6 | **Comment lines deleted** — Some source comments stripped from output | ~5 snapshots |

### Missing Features

| # | Issue | Affects |
|---|-------|---------|
| M1 | **`_fnSignal` not emitted** — Derived signals in JSX children use direct expressions instead of `_fnSignal` wrapper with hoisted function + string | ~50+ snapshots |
| M2 | **`_wrapProp` 2-argument form missing** — `_wrapProp(obj, "prop")` not generated, OXC destructures instead | ~40+ snapshots |
| M3 | **Props destructuring** — Inline component props destructuring and `_restProps` not transformed correctly | ~30+ snapshots |
| M4 | **QRL hoisting** — SWC hoists QRL calls to variables then references them; OXC inlines QRL calls at usage site | ~40+ snapshots |

### JSX Issues

| # | Issue | Affects |
|---|-------|---------|
| J1 | **JSX key generation** — Different key values (`null` vs specific keys like `"u6_0"`) | ~60+ snapshots |
| J2 | **JSX immutability flags** — Wrong flag values (e.g. `1` or `2` in SWC → `3` in OXC) | ~80+ snapshots |

### Import Issues

These are mostly symptoms of other issues. I2 (missing/extra imports) largely resolves when M1-M4 and B4 are fixed. I1 (ordering) is pure cosmetic sorting best done as a final pass.

| # | Issue | Affects |
|---|-------|---------|
| I1 | **Import statement ordering** — OXC emits imports in different order than SWC | ~140+ snapshots |
| I2 | **Missing/extra imports** — OXC includes imports that SWC doesn't (e.g. `onRender`, `useStore`, `useSignal` appear in main module when they should only be in segments) or is missing imports SWC has (e.g. `_fnSignal`, `_wrapProp`). Largely a symptom of M1-M4 and B4. | ~100+ snapshots |
| I3 | **Import specifier splitting** — SWC merges specifiers from same module into one import (`import { useStore, mutable } from "..."`) while OXC splits them into separate imports | ~30+ snapshots |
| I4 | **Import path differences** — Relative paths differ (e.g. `./test.tsx_Header_...` vs `./project/test.tsx_Header_...`) | ~20+ snapshots |
