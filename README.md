# qwik-optimizer-oxc

OXC-based Qwik optimizer achieving parity with the SWC-based reference implementation (qwik-core). The project is driven by a 4,082-line behavioral specification (`SPEC.md`) covering a 13-stage compilation pipeline that transforms Qwik components into lazy-loadable segments with automatic closure capture, QRL generation, and manifest metadata.

## Project Structure

```
crates/qwik-optimizer-oxc/   -- Rust optimizer crate (OXC-based)
src/                         -- TypeScript test harness (comparator, parser, reporter, normalizer)
inputs/                      -- 201 .tsx test fixtures
swc-snapshots/               -- Golden reference snapshots from SWC
oxc-snapshots/               -- OXC-generated snapshots for comparison
SPEC.md                      -- Behavioral specification (sole source of truth)
.planning/                   -- GSD planning docs (gitignored)
```

## Current Status

**Milestone v3.1** -- Phase 28 (Code Generation) complete. Segment identity fully resolved (`missing_segment=0`, `extra_segment=0`).

**Parity: 5 / 201 fixtures passing, 196 failing.** 909 total failures across 13 categories.

| Category | Count | Notes |
|----------|-------|-------|
| code_diff | 465 | Primary focus -- architectural issues remain |
| wrong_capture_names | 71 | |
| hash_mismatch | 70 | Cascades from code_diff |
| display_name_mismatch | 69 | Cascades from code_diff |
| canonical_filename_mismatch | 69 | Cascades from code_diff |
| wrong_captures | 45 | |
| wrong_param_names | 42 | |
| wrong_parent | 38 | |
| wrong_loc | 15 | |
| wrong_ctx_kind | 9 | |
| wrong_ctx_name | 9 | |
| wrong_entry | 4 | |
| diagnostics_mismatch | 3 | |

Hash, display_name, and canonical failures (~208 total) largely cascade from code_diff and are expected to resolve as code generation improves.

## Testing

```bash
# Rust unit tests (380 tests, all passing)
cargo test -p qwik-optimizer-oxc

# Build optimizer
cargo build -p qwik-optimizer-oxc

# TypeScript harness tests
bun test

# Full SWC-vs-OXC comparison
bun run harness --swc-snapshots swc-snapshots/ --oxc-snapshots oxc-snapshots/
```

### Reliability Notes

- **Unit tests (380)** are deterministic pure-Rust tests covering individual optimizer stages.
- **Harness comparison** is the real validation -- compares OXC output against SWC golden snapshots across all 201 fixtures.
- **oxfmt normalization** eliminates cosmetic diffs (whitespace, semicolons, formatting) so only semantic differences remain.
- **Source maps** are stripped from comparison (architectural difference, not behavioral).

## Architectural Challenges

Three major remaining challenges:

1. **transpile_jsx=false preservation** -- SWC can preserve JSX as-is when `transpile_jsx=false`, but OXC's `handle_jsx_props` destructively drains JSX attributes during processing. Requires a JSX attribute reconstruction refactor.

2. **Component body retention** -- SWC retains component body inline in the parent module; OXC extracts it to a segment. This causes `PARENT_MISSING_IMPORTS` failures.

3. **Event handler param naming** -- SWC produces `q_e_keydown` but OXC produces `q_e_key_down` for event handler segments (camelCase word-boundary splitting difference).

## Dependencies

**Rust:**
- `oxc = "0.113"` -- pinned; later versions introduce breaking renames (`Atom` to `Str`, `node_id` additions) with no value for this project
- `siphasher` -- stable cross-platform hashing (not `DefaultHasher` which varies across Rust versions)

**Node:**
- `oxfmt = "0.32.0"` -- code normalization for snapshot comparison
- `siphash` -- JS-side hash verification
- `bun` runtime

## Specification

`SPEC.md` is the sole source of truth: 4,082 lines, 6 chapters, 13-stage pipeline covering the full Qwik compilation model including segment extraction, QRL synthesis, capture analysis, and manifest generation.

## License

MIT
