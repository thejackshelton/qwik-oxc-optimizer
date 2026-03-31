# Testing: Snapshot-Driven TDD

## How It Works

The 162 snapshot files in `crates/qwik-optimizer-oxc/tests/snapshots/` contain the **old (SWC) optimizer's output** as expected values. Running `cargo test` on the new OXC optimizer compares its output against these snapshots. Any diff means the new optimizer is producing different output than the old one.

**This is the sole compliance mechanism.** There are no separate spec files, no unit tests for individual modules, no integration test suite. The snapshots ARE the spec.

## Running Tests

```bash
# Run all 162 snapshot tests
cargo test -p qwik-optimizer-oxc

# Run and see diffs for failing tests
cargo test -p qwik-optimizer-oxc -- --nocapture

# Review snapshot diffs interactively
cargo insta review -p qwik-optimizer-oxc
```

## Current State

~160 of 162 tests are currently failing. This is expected — the new optimizer is being built from scratch. Each passing test represents a feature that works correctly.

## Test Harness Details

The test harness is in `crates/qwik-optimizer-oxc/tests/test.rs`. It:

1. Loads input source from `tests/input/<name>.tsx`
2. Applies per-case config overrides via `apply_case_overrides()`
3. Calls `transform_modules()` with the configured options
4. Formats all code blocks with `oxfmt` (if available)
5. Replaces segment hashes with `XXXXXXXXXXXX` placeholders
6. Renders the output in the snapshot format (`=== SECTION ===` headers)
7. Compares against the accepted snapshot via `insta::assert_snapshot!`

## Working on a Feature

1. Pick a failing snapshot test that covers the feature
2. Read the snapshot to understand expected input → output
3. Read the old optimizer's source to understand the transformation logic
4. Implement the feature in the new optimizer
5. Run `cargo test -p qwik-optimizer-oxc` to check progress
6. When the snapshot matches, the feature is done

## Snapshot Sections

Each snapshot has these sections:

- **`=== INPUT ===`** — The source code being transformed
- **`=== <path> ===`** — A transformed output module (main module first, then segments)
- **`=== <path> === (ENTRY)`** — An entry point module
- **`/* ... */`** — JSON metadata block after each segment module
- **`=== DIAGNOSTICS ===`** — JSON array of diagnostics (errors/warnings)

## Hash Placeholders

Segment hashes are replaced with `XXXXXXXXXXXX` in snapshots to make comparisons stable across hash algorithm changes. The test harness does this replacement automatically.
