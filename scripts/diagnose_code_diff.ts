/**
 * Diagnostic script: quantify code_diff failure categories across 200 failing fixtures.
 *
 * Reads all .snap files from oxc-snapshots/ and swc-snapshots/, matches segments by
 * identity (ctxName@loc[0]-loc[1]), normalizes code with oxfmt, then classifies diffs.
 *
 * Usage: bun scripts/diagnose_code_diff.ts
 * Output: .planning/phases/25-code-generation/DIAGNOSIS.md
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { parseSnapFile } from "../src/parser.ts";
import { normalizeCode } from "../src/normalizer.ts";
import type { ParsedSection } from "../src/types.ts";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");

const SWC_DIR = path.join(ROOT, "swc-snapshots");
const OXC_DIR = path.join(ROOT, "oxc-snapshots");
const PHASE_DIR = path.join(ROOT, ".planning", "phases", "25-code-generation");

// ──────────────────────────────────────────────
// Category classification
// ──────────────────────────────────────────────

type Category =
  | "JSX_IN_SEGMENT"
  | "MISSING_QRL_IMPORT"
  | "MISSING_PURE"
  | "MISSING_COMMENT_SEPARATOR"
  | "WRONG_IMPORTS_PARENT"
  | "OTHER";

interface DiffEntry {
  fixture: string;
  sectionType: "segment" | "parent";
  sectionName: string;
  category: Category;
  swcCode: string;
  oxcCode: string;
}

function classifyDiff(
  swcNorm: string,
  oxcNorm: string,
  isParent: boolean
): Category {
  // WRONG_IMPORTS_PARENT: OXC has _jsxSorted/_jsxSplit import that SWC doesn't (parent module)
  if (
    isParent &&
    (oxcNorm.includes("_jsxSorted") || oxcNorm.includes("_jsxSplit")) &&
    !swcNorm.includes("_jsxSorted") &&
    !swcNorm.includes("_jsxSplit")
  ) {
    return "WRONG_IMPORTS_PARENT";
  }

  // JSX_IN_SEGMENT: OXC segment has _jsxSorted or _jsxSplit calls
  if (
    !isParent &&
    (oxcNorm.includes("_jsxSorted(") || oxcNorm.includes("_jsxSplit(")) &&
    !swcNorm.includes("_jsxSorted(") &&
    !swcNorm.includes("_jsxSplit(")
  ) {
    return "JSX_IN_SEGMENT";
  }

  // MISSING_QRL_IMPORT: SWC has `import { qrl }` but OXC doesn't
  if (
    swcNorm.includes('import { qrl }') ||
    swcNorm.match(/import\s*\{[^}]*\bqrl\b[^}]*\}/)
  ) {
    const oxcHasQrl = oxcNorm.includes('import { qrl }') || oxcNorm.match(/import\s*\{[^}]*\bqrl\b[^}]*\}/);
    if (!oxcHasQrl) {
      return "MISSING_QRL_IMPORT";
    }
  }

  // MISSING_PURE: SWC has /*#__PURE__*/ but OXC doesn't
  if (swcNorm.includes("/*#__PURE__*/") && !oxcNorm.includes("/*#__PURE__*/")) {
    return "MISSING_PURE";
  }

  // MISSING_COMMENT_SEPARATOR: SWC has standalone // comment line but OXC doesn't
  const SWC_HAS_COMMENT_SEP = /^\/\/\s*$/m.test(swcNorm);
  const OXC_HAS_COMMENT_SEP = /^\/\/\s*$/m.test(oxcNorm);
  if (SWC_HAS_COMMENT_SEP && !OXC_HAS_COMMENT_SEP) {
    return "MISSING_COMMENT_SEPARATOR";
  }

  return "OTHER";
}

// ──────────────────────────────────────────────
// Segment identity key (matches matcher.ts)
// ──────────────────────────────────────────────

function identityKey(section: ParsedSection): string {
  const m = section.metadata!;
  return `${m.ctxName}@${m.loc[0]}-${m.loc[1]}`;
}

// ──────────────────────────────────────────────
// Main
// ──────────────────────────────────────────────

const snapFiles = fs
  .readdirSync(OXC_DIR)
  .filter((f) => f.endsWith(".snap"))
  .sort();

const diffs: DiffEntry[] = [];
const skipped: string[] = [];
let fixturesWithDiff = 0;
let fixturesClean = 0;
let totalComparisons = 0;

for (const snapFile of snapFiles) {
  const fixtureName = snapFile.replace(/\.snap$/, "");
  const oxcPath = path.join(OXC_DIR, snapFile);
  const swcPath = path.join(SWC_DIR, snapFile);

  if (!fs.existsSync(swcPath)) {
    skipped.push(`${fixtureName}: missing SWC snapshot`);
    continue;
  }

  let swcSnap, oxcSnap;
  try {
    swcSnap = parseSnapFile(swcPath);
    oxcSnap = parseSnapFile(oxcPath);
  } catch (err) {
    skipped.push(`${fixtureName}: parse error — ${err}`);
    continue;
  }

  let fixtureDiffed = false;

  // ── Match segment sections by identity ──
  const swcSegs = swcSnap.sections.filter((s) => s.metadata !== null);
  const oxcSegs = oxcSnap.sections.filter((s) => s.metadata !== null);

  // Build OXC map by identity
  const oxcByKey = new Map<string, ParsedSection[]>();
  for (const s of oxcSegs) {
    const k = identityKey(s);
    const arr = oxcByKey.get(k) ?? [];
    arr.push(s);
    oxcByKey.set(k, arr);
  }

  for (const swcSeg of swcSegs) {
    const key = identityKey(swcSeg);
    const candidates = oxcByKey.get(key);
    if (!candidates || candidates.length === 0) continue;
    const oxcSeg = candidates.shift()!;
    if (candidates.length === 0) oxcByKey.delete(key);

    // Use the fixture's origin as the file extension hint
    const ext = swcSeg.metadata?.extension ?? "tsx";
    const fakeFile = `segment.${ext}`;

    let swcNorm: string;
    let oxcNorm: string;
    try {
      swcNorm = normalizeCode(swcSeg.code, fakeFile);
      oxcNorm = normalizeCode(oxcSeg.code, fakeFile);
    } catch {
      // If normalization fails, try raw comparison
      swcNorm = swcSeg.code;
      oxcNorm = oxcSeg.code;
    }

    totalComparisons++;
    if (swcNorm !== oxcNorm) {
      const category = classifyDiff(swcNorm, oxcNorm, false);
      diffs.push({
        fixture: fixtureName,
        sectionType: "segment",
        sectionName: swcSeg.headerName,
        category,
        swcCode: swcNorm,
        oxcCode: oxcNorm,
      });
      fixtureDiffed = true;
    }
  }

  // ── Parent sections (metadata === null) ──
  const swcParents = swcSnap.sections.filter((s) => s.metadata === null);
  const oxcParents = oxcSnap.sections.filter((s) => s.metadata === null);

  // Match parent sections by header name
  const oxcParentByName = new Map<string, ParsedSection[]>();
  for (const s of oxcParents) {
    const arr = oxcParentByName.get(s.headerName) ?? [];
    arr.push(s);
    oxcParentByName.set(s.headerName, arr);
  }

  for (const swcParent of swcParents) {
    const candidates = oxcParentByName.get(swcParent.headerName);
    if (!candidates || candidates.length === 0) continue;
    const oxcParent = candidates.shift()!;
    if (candidates.length === 0) oxcParentByName.delete(swcParent.headerName);

    // Determine extension from header name
    const ext = swcParent.headerName.endsWith(".tsx")
      ? "tsx"
      : swcParent.headerName.endsWith(".ts")
      ? "ts"
      : swcParent.headerName.endsWith(".jsx")
      ? "jsx"
      : "js";
    const fakeFile = `parent.${ext}`;

    let swcNorm: string;
    let oxcNorm: string;
    try {
      swcNorm = normalizeCode(swcParent.code, fakeFile);
      oxcNorm = normalizeCode(oxcParent.code, fakeFile);
    } catch {
      swcNorm = swcParent.code;
      oxcNorm = oxcParent.code;
    }

    totalComparisons++;
    if (swcNorm !== oxcNorm) {
      const category = classifyDiff(swcNorm, oxcNorm, true);
      diffs.push({
        fixture: fixtureName,
        sectionType: "parent",
        sectionName: swcParent.headerName,
        category,
        swcCode: swcNorm,
        oxcCode: oxcNorm,
      });
      fixtureDiffed = true;
    }
  }

  if (fixtureDiffed) {
    fixturesWithDiff++;
  } else {
    fixturesClean++;
  }
}

// ──────────────────────────────────────────────
// Tally categories
// ──────────────────────────────────────────────

const CATEGORIES: Category[] = [
  "JSX_IN_SEGMENT",
  "MISSING_QRL_IMPORT",
  "MISSING_PURE",
  "MISSING_COMMENT_SEPARATOR",
  "WRONG_IMPORTS_PARENT",
  "OTHER",
];

const counts: Record<Category, number> = {
  JSX_IN_SEGMENT: 0,
  MISSING_QRL_IMPORT: 0,
  MISSING_PURE: 0,
  MISSING_COMMENT_SEPARATOR: 0,
  WRONG_IMPORTS_PARENT: 0,
  OTHER: 0,
};

for (const d of diffs) {
  counts[d.category]++;
}

// ──────────────────────────────────────────────
// Gather examples (up to 3 per category)
// ──────────────────────────────────────────────

const examples: Record<Category, DiffEntry[]> = {
  JSX_IN_SEGMENT: [],
  MISSING_QRL_IMPORT: [],
  MISSING_PURE: [],
  MISSING_COMMENT_SEPARATOR: [],
  WRONG_IMPORTS_PARENT: [],
  OTHER: [],
};

for (const d of diffs) {
  if (examples[d.category].length < 3) {
    examples[d.category].push(d);
  }
}

// ──────────────────────────────────────────────
// Render DIAGNOSIS.md
// ──────────────────────────────────────────────

function codeBlock(code: string, lang = "tsx"): string {
  // Truncate long code blocks for readability
  const lines = code.split("\n");
  const truncated = lines.length > 25 ? lines.slice(0, 25).join("\n") + "\n... (truncated)" : code;
  return "```" + lang + "\n" + truncated + "\n```";
}

let md = `# Phase 25 Code-Diff Diagnosis

**Generated:** ${new Date().toISOString()}

## Summary

| Metric | Value |
|--------|-------|
| Total fixtures analyzed | ${snapFiles.length - skipped.length} |
| Fixtures with at least one diff | ${fixturesWithDiff} |
| Fixtures fully clean | ${fixturesClean} |
| Total section comparisons | ${totalComparisons} |
| Total diffs found | ${diffs.length} |

## Category Breakdown

| Category | Count | % of diffs |
|----------|-------|-----------|
`;

for (const cat of CATEGORIES) {
  const pct = diffs.length === 0 ? 0 : ((counts[cat] / diffs.length) * 100).toFixed(1);
  md += `| \`${cat}\` | ${counts[cat]} | ${pct}% |\n`;
}

md += `\n## Root-Cause Classes\n\n`;

for (const cat of CATEGORIES) {
  if (counts[cat] === 0) continue;

  md += `### \`${cat}\` (${counts[cat]} diffs)\n\n`;

  switch (cat) {
    case "JSX_IN_SEGMENT":
      md += `**Root cause:** OXC emits \`_jsxSorted(\` / \`_jsxSplit(\` calls inside segment code. SWC emits plain JSX (\`<div ...>\`). The segment should contain JSX syntax, not runtime helper calls.\n\n`;
      break;
    case "MISSING_QRL_IMPORT":
      md += `**Root cause:** SWC emits \`import { qrl } from "@qwik.dev/core"\` in the segment/parent file. OXC does not emit this import, causing undefined reference to \`qrl\`.\n\n`;
      break;
    case "MISSING_PURE":
      md += `**Root cause:** SWC wraps \`qrl()\` calls with \`/*#__PURE__*/\` annotation for tree-shaking. OXC omits these annotations.\n\n`;
      break;
    case "MISSING_COMMENT_SEPARATOR":
      md += `**Root cause:** SWC emits standalone \`//\` comment separator lines between declaration groups. OXC omits these separators.\n\n`;
      break;
    case "WRONG_IMPORTS_PARENT":
      md += `**Root cause:** OXC parent module imports \`_jsxSorted\` / \`_jsxSplit\` from the runtime. SWC parent module does not import these — JSX is handled differently (segments retain JSX, parent uses plain qrl calls).\n\n`;
      break;
    case "OTHER":
      md += `**Root cause:** Miscellaneous differences not covered by the 4 primary categories. Includes extra trailing semicolons, whitespace edge cases, or import ordering differences.\n\n`;
      break;
  }

  const exList = examples[cat];
  if (exList.length === 0) {
    md += `*No examples available.*\n\n`;
    continue;
  }

  for (let i = 0; i < exList.length; i++) {
    const ex = exList[i];
    md += `#### Example ${i + 1}: \`${ex.fixture}\` / ${ex.sectionType} \`${ex.sectionName}\`\n\n`;
    md += `**SWC (expected):**\n\n`;
    md += codeBlock(ex.swcCode) + "\n\n";
    md += `**OXC (actual):**\n\n`;
    md += codeBlock(ex.oxcCode) + "\n\n";
  }
}

if (skipped.length > 0) {
  md += `## Skipped Fixtures\n\n`;
  for (const s of skipped) {
    md += `- ${s}\n`;
  }
  md += "\n";
}

md += `## Implications for Phase 25 Plans\n\n`;
md += `- **Plan 25-02**: Fix \`JSX_IN_SEGMENT\` (${counts.JSX_IN_SEGMENT} diffs) — segments must emit JSX syntax, not \`_jsxSorted\` calls\n`;
md += `- **Plan 25-02**: Fix \`WRONG_IMPORTS_PARENT\` (${counts.WRONG_IMPORTS_PARENT} diffs) — parent module must not import \`_jsxSorted\`/\`_jsxSplit\`\n`;
md += `- **Plan 25-03**: Fix \`MISSING_QRL_IMPORT\` (${counts.MISSING_QRL_IMPORT} diffs) — add \`import { qrl }\` to files that use it\n`;
md += `- **Plan 25-03**: Fix \`MISSING_PURE\` (${counts.MISSING_PURE} diffs) — annotate \`qrl()\` calls with \`/*#__PURE__*/\`\n`;
md += `- **Plan 25-03**: Fix \`MISSING_COMMENT_SEPARATOR\` (${counts.MISSING_COMMENT_SEPARATOR} diffs) — emit standalone \`//\` separator lines\n`;
if (counts.OTHER > 0) {
  md += `- **Plan 25-04 (TBD)**: Investigate \`OTHER\` (${counts.OTHER} diffs) — needs further analysis\n`;
}
md += "\n";

// Write output
fs.mkdirSync(PHASE_DIR, { recursive: true });
const outPath = path.join(PHASE_DIR, "DIAGNOSIS.md");
fs.writeFileSync(outPath, md, "utf8");

console.log("=== Code-Diff Diagnosis Complete ===");
console.log(`Fixtures analyzed: ${snapFiles.length - skipped.length}`);
console.log(`Fixtures with diffs: ${fixturesWithDiff}`);
console.log(`Total comparisons: ${totalComparisons}`);
console.log(`Total diffs: ${diffs.length}`);
console.log("\nCategory breakdown:");
for (const cat of CATEGORIES) {
  const pct = diffs.length === 0 ? 0 : ((counts[cat] / diffs.length) * 100).toFixed(1);
  console.log(`  ${cat.padEnd(30)} ${String(counts[cat]).padStart(4)}  (${pct}%)`);
}
if (skipped.length > 0) {
  console.log(`\nSkipped: ${skipped.length} fixture(s)`);
  for (const s of skipped) console.log(`  - ${s}`);
}
console.log(`\nDIAGNOSIS.md written to: ${outPath}`);
