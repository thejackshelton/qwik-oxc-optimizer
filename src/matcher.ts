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

  // Build identity key from ctxName + loc (SPEC-04: no derived fields)
  const identityKey = (s: ParsedSection): string => {
    const m = s.metadata!;
    return `${m.ctxName}@${m.loc[0]}-${m.loc[1]}`;
  };

  // First pass: match by identity (ctxName + loc) regardless of order
  const matches: SegmentMatch[] = [];
  const matchedSwcIndices = new Set<number>();
  const matchedOxcIndices = new Set<number>();
  let ambiguous = false;

  // Build a map of OXC segments by identity for O(n) lookup
  const oxcByIdentity = new Map<string, number[]>();
  for (let i = 0; i < oxcSegs.length; i++) {
    const key = identityKey(oxcSegs[i]);
    const existing = oxcByIdentity.get(key) ?? [];
    existing.push(i);
    oxcByIdentity.set(key, existing);
  }

  // Match SWC segments to OXC by identity
  for (let si = 0; si < swcSegs.length; si++) {
    const swc = swcSegs[si];
    const key = identityKey(swc);
    const candidates = oxcByIdentity.get(key);
    if (candidates && candidates.length > 0) {
      const oi = candidates.shift()!;
      if (candidates.length === 0) oxcByIdentity.delete(key);
      matchedSwcIndices.add(si);
      matchedOxcIndices.add(oi);
      matches.push({ swcSection: swc, oxcSection: oxcSegs[oi], confidence: "high" });
    }
  }

  // Second pass: pair remaining unmatched segments by order (low confidence)
  const remainingSwc = swcSegs.filter((_, i) => !matchedSwcIndices.has(i));
  const remainingOxc = oxcSegs.filter((_, i) => !matchedOxcIndices.has(i));
  const pairCount = Math.min(remainingSwc.length, remainingOxc.length);

  for (let i = 0; i < pairCount; i++) {
    const swc = remainingSwc[i];
    const oxc = remainingOxc[i];
    const swcMeta = swc.metadata!;
    const oxcMeta = oxc.metadata!;

    const reasons: string[] = [];
    if (swcMeta.ctxName !== oxcMeta.ctxName) {
      reasons.push(`ctxName: "${swcMeta.ctxName}" vs "${oxcMeta.ctxName}"`);
    }
    if (swcMeta.loc[0] !== oxcMeta.loc[0]) {
      reasons.push(`loc[0]: ${swcMeta.loc[0]} vs ${oxcMeta.loc[0]}`);
    }
    if (swcMeta.loc[1] !== oxcMeta.loc[1]) {
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

  // Truly unmatched segments (identity + order both failed)
  const unmatched_swc = remainingSwc.slice(pairCount);
  const unmatched_oxc = remainingOxc.slice(pairCount);

  if (unmatched_swc.length > 0 || unmatched_oxc.length > 0) {
    ambiguous = true;
  }

  return { matches, unmatched_swc, unmatched_oxc, ambiguous };
}
