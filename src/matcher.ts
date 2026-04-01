/**
 * Order-based segment matcher.
 *
 * Pairs SWC and OXC segments by section order index (primary key).
 * Uses ctxName + full loc tuple as a confidence check — mismatch lowers confidence
 * to "low" and sets ambiguous=true, but does NOT prevent pairing.
 *
 * SPEC-04 constraint: derived fields (displayName, hash, canonicalFilename,
 * name, parent) are NEVER accessed for matching decisions.
 */

import type { ParsedSection } from "./types";

export interface SegmentMatch {
  swcSection: ParsedSection;
  oxcSection: ParsedSection;
  confidence: "high" | "low";
  /** Human-readable explanation when confidence is "low" */
  reason?: string;
}

export interface MatchResult {
  matches: SegmentMatch[];
  unmatched_swc: ParsedSection[];
  unmatched_oxc: ParsedSection[];
  ambiguous: boolean;
}

/**
 * Pair SWC and OXC segment sections by order index.
 *
 * Only sections with non-null metadata are treated as segments.
 * Parent sections (metadata === null) are ignored.
 */
export function matchSegments(
  swcSections: ParsedSection[],
  oxcSections: ParsedSection[]
): MatchResult {
  // Filter to segment sections only (SPEC-04: parent sections excluded)
  const swcSegs = swcSections.filter((s) => s.metadata !== null);
  const oxcSegs = oxcSections.filter((s) => s.metadata !== null);

  const pairCount = Math.min(swcSegs.length, oxcSegs.length);
  const matches: SegmentMatch[] = [];
  let ambiguous = false;

  for (let i = 0; i < pairCount; i++) {
    const swc = swcSegs[i];
    const oxc = oxcSegs[i];

    // Confidence check: ctxName match AND full loc tuple match → high
    // CRITICAL: only ctxName and loc are accessed here — no derived fields
    const swcMeta = swc.metadata!;
    const oxcMeta = oxc.metadata!;

    const ctxNameMatch = swcMeta.ctxName === oxcMeta.ctxName;
    const locStartMatch = swcMeta.loc[0] === oxcMeta.loc[0];
    const locEndMatch = swcMeta.loc[1] === oxcMeta.loc[1];
    const locMatch = locStartMatch && locEndMatch;

    if (ctxNameMatch && locMatch) {
      matches.push({ swcSection: swc, oxcSection: oxc, confidence: "high" });
    } else {
      const reasons: string[] = [];
      if (!ctxNameMatch) {
        reasons.push(`ctxName: "${swcMeta.ctxName}" vs "${oxcMeta.ctxName}"`);
      }
      if (!locStartMatch) {
        reasons.push(`loc[0]: ${swcMeta.loc[0]} vs ${oxcMeta.loc[0]}`);
      }
      if (!locEndMatch) {
        reasons.push(`loc[1]: ${swcMeta.loc[1]} vs ${oxcMeta.loc[1]}`);
      }
      matches.push({
        swcSection: swc,
        oxcSection: oxc,
        confidence: "low",
        reason: reasons.join("; "),
      });
      ambiguous = true;
    }
  }

  // Leftover segments
  const unmatched_swc = swcSegs.slice(pairCount);
  const unmatched_oxc = oxcSegs.slice(pairCount);

  if (unmatched_swc.length > 0 || unmatched_oxc.length > 0) {
    ambiguous = true;
  }

  return { matches, unmatched_swc, unmatched_oxc, ambiguous };
}
