# Concerns & Risks

## Arena Lifetime Infection

Every function that creates AST nodes needs the `'a` lifetime parameter from the allocator. This cascades through helper functions. Design around this from the start — don't fight it.

See `.planning/research/PITFALLS.md` #1 for details.

## Semantic Invalidation

`oxc_semantic` data (ScopeId, SymbolId, ReferenceId) goes stale after AST mutation. All analysis MUST happen before any mutation. Use `ctx.generate_binding()` and `ctx.create_bound_reference()` for new nodes created during emit.

See `.planning/research/PITFALLS.md` #3 for details.

## Multi-Output Allocator Strategy

Building segment `Program` ASTs during input traversal causes borrow conflicts. Collect plain data (spans, strings, metadata) during traversal, then build segment programs after traversal completes.

See `.planning/research/PITFALLS.md` #4 for details.

## Capture Analysis Edge Cases

8+ distinct edge cases that each cause runtime failures if wrong:
1. Imports are NOT captured (segment gets its own import)
2. Exports at module root are NOT captured
3. Loop iteration variables ARE captured with special handling
4. Shadowed variables use the INNER binding
5. Destructured component props need transformation
6. `const` vs `let`/`var` affects optimization flags
7. Function declarations are hoisted
8. TypeScript type-only imports must NOT be captured

See `.planning/research/PITFALLS.md` #5 for details.

## Hash Compatibility

Segment hashes must match between old and new optimizers for drop-in replacement. The hash depends on display name, file path, and scope context. Any string difference changes the hash entirely.

In our snapshots, hashes are replaced with `XXXXXXXXXXXX` placeholders, so hash differences don't cause test failures. But hash correctness matters for production use.

See `.planning/research/PITFALLS.md` #11 for details.

## Statement Insertion

OXC's `Traverse` visits individual nodes — you can't insert siblings during `enter_statement`/`exit_statement`. Use deferred insertion: collect pending statements, then apply them in `exit_program` or `exit_statements`.

See `.planning/research/PITFALLS.md` #7 for details.

## `#__PURE__` Annotations

OXC doesn't auto-attach `#__PURE__` comments to new nodes. You must explicitly add comments to `program.comments` at the correct span position for `qrl()` and `componentQrl()` calls. Missing these = broken tree-shaking.

See `.planning/research/PITFALLS.md` #8 for details.
