# Qwik Optimizer — OXC Port

## What This Is

A Rust-based Qwik optimizer that replaces the SWC-based implementation with OXC. It transforms JS/TS/JSX/TSX source files by extracting lazy-loadable segments at `$()` boundaries, producing multiple output modules with QRL wrappers, capture arrays, and segment metadata. The port must produce identical output to the SWC optimizer, validated against 162 snapshot tests.

## Core Value

Snapshot parity with the SWC optimizer — every transformation must produce identical output across all 162 test cases.

## Current Milestone: v1.0 Full Snapshot Parity

**Goal:** Make all 162 snapshot tests pass, matching the SWC optimizer's output exactly.

**Target features:**
- Correct segment naming (event handlers, display names, parent fields)
- Complete metadata (paramNames, path fields)
- All bugs fixed (TS stripping, missing segments, captures, component options, comments)
- Signal reactivity transforms (_fnSignal, _wrapProp)
- Props destructuring transforms
- QRL hoisting
- Correct JSX keys and immutability flags
- Import ordering and merging matching SWC output

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] All 162 snapshot tests passing against SWC golden reference
- [ ] Naming: event handler names, display name context, parent field format
- [ ] Metadata: paramNames, path field handling
- [ ] Bugs: TS type stripping, missing segments, capture analysis, component options, comments
- [ ] Features: signal wrapping, props destructuring, QRL hoisting
- [ ] JSX: key generation, immutability flags
- [ ] Imports: ordering, specifier merging, path handling

### Out of Scope

- TS/JSX transpilation via OXC transformer — defer to v2, focus on Qwik-specific transforms
- Diagnostic emission (QWIK error messages) — implement once core transforms stable
- Performance optimization (rayon parallelism) — correctness first
- Entry strategy variations beyond what snapshots test — match SWC behavior only

## Context

- Two-phase architecture (analyze → emit) is already in place
- Public API (`transform_modules()`) matches the SWC optimizer's interface
- 162 snapshot tests exist with SWC golden reference output
- 160/162 snapshots currently have diffs (~3,900 insertions, ~7,200 deletions)
- ISSUES.md contains detailed analysis of all diff categories with priority ordering
- OXC 0.113 is the foundation (parser, semantic analysis, codegen, traverse)
- Research completed 2026-02-10 covering stack, architecture, features, and pitfalls

## Constraints

- **Snapshot integrity**: NEVER commit changes to `.snap` files — they are the SWC golden reference and source of truth for TDD
- **Tech stack**: OXC 0.113 with Rust edition 2024, MSRV 1.91.0
- **Two-phase constraint**: All semantic analysis must happen before AST mutation (OXC limitation)
- **Arena lifetimes**: Every AST-constructing function needs `'a` lifetime parameter
- **Multi-output**: Segment Program ASTs built after traversal, not during

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| OXC over SWC | 3x faster parsing, better maintained, comprehensive semantic analysis | — Pending |
| Two-phase (analyze → emit) | OXC semantic data goes stale after mutation | — Pending |
| Snapshot-driven TDD | 162 existing snapshots provide concrete correctness baseline | — Pending |
| ISSUES.md priority ordering | Naming first (cascading), then metadata, bugs, features, JSX, imports last | — Pending |

---
*Last updated: 2026-02-19 after milestone v1.0 initialization*
