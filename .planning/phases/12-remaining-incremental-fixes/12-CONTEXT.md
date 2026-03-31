# Phase 12: Signal Wrapping Gaps - Context

**Gathered:** 2026-02-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Fix remaining _fnSignal and _wrapProp wrapping gaps across ~25 affected tests. This is the single largest category of remaining semantic diffs. Phase 12 ONLY covers signal/prop wrapping — captures, DCE, and small categories are phases 13-14.

**Scope change:** Original phase 12 ("Remaining Incremental Fixes") was split into 3 focused phases after a fresh audit showed the scope was ~2x bigger than estimated:
- Phase 12: Signal wrapping (_fnSignal + _wrapProp) — ~25 tests
- Phase 13: Captures edge cases + DCE — ~20 tests
- Phase 14: Small categories (ctxKind, entry field, dev mode, file ext, etc.) — ~15 tests

</domain>

<decisions>
## Implementation Decisions

### Acceptance criteria
- Fix ALL semantic diffs — only OXC codegen aesthetic differences (shorthand, whitespace) are accepted
- No "won't fix" categories — every semantic diff must be addressed across the 3 phases
- Tests with multiple overlapping categories: fixing wrapping in phase 12 may cascade-resolve some captures/DCE diffs too

### Signal wrapping scope
- _fnSignal: Missing or incorrect _fnSignal() wrapping in JSX expressions (~23 tests)
- _wrapProp: Missing or incorrect _wrapProp() wrapping in component props (~14 tests)
- Many tests have both — these are interrelated transforms
- Fixing wrapping also changes capture content (side effect — benefits phase 13)

### Claude's Discretion
- Technical approach to fixing wrapping gaps (per-test vs systematic analysis)
- Research methodology (SWC reference comparison depth)
- Plan structure and task breakdown

</decisions>

<specifics>
## Specific Ideas

No specific requirements — research should do a systematic SWC comparison of all wrapping-related diff lines to find root causes. Many tests may share the same few underlying bugs.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. Captures/DCE and small categories are already scoped as phases 13-14.

</deferred>

---

*Phase: 12-remaining-incremental-fixes*
*Context gathered: 2026-02-23*
