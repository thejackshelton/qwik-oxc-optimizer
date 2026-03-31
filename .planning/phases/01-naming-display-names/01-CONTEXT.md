# Phase 1: Naming & Display Names - Context

**Gathered:** 2026-02-19
**Status:** Ready for planning

<domain>
## Phase Boundary

Fix segment naming so that event handler names, display name scope chains, and parent field format in metadata match SWC output exactly. This clears the most pervasive diff noise across 120+ snapshots.

</domain>

<decisions>
## Implementation Decisions

### Approach
- Pure correctness work — SWC snapshots fully define the target behavior
- No user decisions needed; the spec is the snapshot golden reference
- Study SWC source (`collector.rs`, `code_move.rs`) to understand naming algorithms
- Compare snapshot diffs to identify exactly where OXC diverges

### Claude's Discretion
- Investigation order (which naming issue to tackle first)
- Implementation approach (how to restructure OXC naming logic)
- Whether to fix all three naming issues together or in separate passes

</decisions>

<specifics>
## Specific Ideas

No specific requirements — snapshots are the spec.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 01-naming-display-names*
*Context gathered: 2026-02-19*
