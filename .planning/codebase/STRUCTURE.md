# Project Structure

## Directory Layout

```
qwik-oxc-optimizer/
├── crates/
│   ├── qwik-optimizer-oxc/          # NEW optimizer (OXC-based) — this is what we're building
│   │   ├── src/
│   │   │   ├── lib.rs               # Public API: transform_modules()
│   │   │   └── ...                  # Implementation modules (to be built)
│   │   ├── tests/
│   │   │   ├── test.rs              # Snapshot test harness (162 cases)
│   │   │   ├── input/               # Test input .tsx files (one per case)
│   │   │   └── snapshots/           # Expected snapshot outputs (from old optimizer)
│   │   └── Cargo.toml
│   └── swc-optimizer/               # OLD optimizer (SWC-based) — reference implementation
│       └── core/
│           ├── src/
│           │   ├── lib.rs            # transform_modules() entry point
│           │   ├── parse.rs          # AST parsing + code transformation
│           │   ├── collector.rs      # Import/export/declaration collection
│           │   ├── code_move.rs      # Segment extraction and code movement
│           │   ├── entry_strategy.rs # Entry point strategy logic
│           │   ├── props_destructuring.rs
│           │   ├── filter_exports.rs
│           │   ├── const_replace.rs
│           │   └── words.rs          # String constants
│           └── src/snapshots/        # Old optimizer's accepted snapshots
├── scripts/
│   ├── format-snapshot-code-blocks.mjs  # Format code blocks in snapshots with oxfmt
│   ├── fix-old-snapshot-blanks.mjs      # Normalize blank lines in old snapshots
│   └── copy-old-to-new-snapshots.mjs    # Copy old snapshot content to new snapshots
├── .planning/
│   ├── codebase/                    # THIS directory — project structure docs
│   └── research/                    # OXC research (architecture, features, pitfalls, stack)
├── Cargo.toml                       # Workspace root
└── AGENTS.md                        # Agent instructions for automated work
```

## Key Files

### New Optimizer (what we're building)

- **`crates/qwik-optimizer-oxc/src/lib.rs`** — Public API. Exports `transform_modules()` and all public types (`TransformModulesOptions`, `TransformOutput`, `TransformModule`, `SegmentAnalysis`, etc.).
- **`crates/qwik-optimizer-oxc/tests/test.rs`** — The snapshot test harness. Runs all 162 test cases, formats code with oxfmt, replaces hashes with placeholders, and compares against `insta` snapshots.
- **`crates/qwik-optimizer-oxc/tests/input/*.tsx`** — Input source files for each test case (one file per case).
- **`crates/qwik-optimizer-oxc/tests/snapshots/*.snap`** — Expected snapshot outputs. These contain the OLD optimizer's output and serve as the TDD baseline.

### Old Optimizer (reference only)

- **`crates/swc-optimizer/core/src/`** — The complete SWC-based implementation. Read this to understand what the new optimizer must do.
- **`crates/swc-optimizer/core/src/snapshots/`** — The old optimizer's own snapshots (used to validate the old optimizer, not the new one).

### Test Configuration

Per-case config overrides are defined in `test.rs:apply_case_overrides()`. This function sets `transpile_ts`, `transpile_jsx`, `entry_strategy`, `mode`, `filename`, `strip_exports`, etc. for each test case. There are no external config files.

## Snapshot Format

Each snapshot (`.snap` file) has this structure:

```
---
source: crates/qwik-optimizer-oxc/tests/test.rs
expression: output
snapshot_kind: text
---
=== INPUT ===
<source code>

=== <module_path> ===
<transformed code>

/*
{
  "origin": "...",
  "name": "...",
  ...segment metadata JSON...
}
*/

=== DIAGNOSTICS ===

<diagnostics JSON array>
```

Hashes in the snapshot are replaced with `XXXXXXXXXXXX` placeholders for stability.
