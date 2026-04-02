/**
 * Diagnose all code_diff failures by root cause category.
 * Outputs DIAGNOSIS.md with categorized breakdown and fix priorities.
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { parseSnapFile } from "../src/parser.js";
import { compareFixture, type FailureItem } from "../src/comparator.js";

const SWC_DIR = path.resolve("swc-snapshots");
const OXC_DIR = path.resolve("oxc-snapshots");
const OUTPUT = path.resolve(".planning/phases/28-code-generation/DIAGNOSIS.md");

interface CategorizedDiff {
  fixture: string;
  rootCause: string;
  field?: string; // "parentModule" or undefined (segment)
  expectedSnippet: string;
  actualSnippet: string;
}

function categorize(failure: FailureItem, fixture: string): CategorizedDiff[] {
  if (failure.category !== "code_diff") return [];

  const expected = String(failure.expected ?? "").trim();
  const actual = String(failure.actual ?? "").trim();
  const isParent = failure.field === "parentModule";
  const prefix = isParent ? "PARENT" : "SEGMENT";

  const results: CategorizedDiff[] = [];

  function push(cause: string) {
    results.push({
      fixture,
      rootCause: cause,
      field: failure.field,
      expectedSnippet: expected.slice(0, 300),
      actualSnippet: actual.slice(0, 300),
    });
  }

  if (isParent) {
    // Parent module analysis
    const swcHasJsxSorted = expected.includes("_jsxSorted") || expected.includes("_jsxSplit");
    const oxcHasJsxSorted = actual.includes("_jsxSorted") || actual.includes("_jsxSplit");

    // OXC parent has _jsxSorted imports but SWC does not
    if (!swcHasJsxSorted && oxcHasJsxSorted) {
      push("PARENT_UNWANTED_JSX_IMPORT");
      return results;
    }

    // Check for extra QRL hoists in OXC parent
    const swcQrlConsts = (expected.match(/const \w+\s*=\s*\/\*\s*@__PURE__\s*\*\/\s*qrl\(/g) || []).length;
    const oxcQrlConsts = (actual.match(/const \w+\s*=\s*\/\*\s*@__PURE__\s*\*\/\s*qrl\(/g) || []).length;
    if (oxcQrlConsts > swcQrlConsts) {
      push("PARENT_EXTRA_QRL_HOISTS");
      return results;
    }

    // Check for extra imports in OXC
    const swcImports = (expected.match(/^import\s/gm) || []).length;
    const oxcImports = (actual.match(/^import\s/gm) || []).length;
    if (oxcImports > swcImports) {
      push("PARENT_EXTRA_IMPORTS");
      return results;
    }
    if (swcImports > oxcImports) {
      push("PARENT_MISSING_IMPORTS");
      return results;
    }

    // Check for missing separators
    const swcSeps = (expected.match(/^\/\//gm) || []).length;
    const oxcSeps = (actual.match(/^\/\//gm) || []).length;
    if (swcSeps > oxcSeps + 2) {
      push("PARENT_MISSING_SEPARATOR");
      return results;
    }

    push("PARENT_OTHER");
    return results;
  }

  // Segment module analysis
  const swcHasJsxSorted = expected.includes("_jsxSorted") || expected.includes("_jsxSplit");
  const oxcHasJsxSorted = actual.includes("_jsxSorted") || actual.includes("_jsxSplit");
  const oxcHasRawJsx = /<[A-Za-z]/.test(actual) || actual.includes("<>");

  // SWC has _jsxSorted but OXC has raw JSX
  if (swcHasJsxSorted && !oxcHasJsxSorted && oxcHasRawJsx) {
    push("SEGMENT_JSX_NOT_TRANSFORMED");
    return results;
  }

  // SWC has _jsxSorted but OXC also has _jsxSorted — other difference
  if (swcHasJsxSorted && oxcHasJsxSorted) {
    // Check for missing qrl imports
    const swcHasQrlImport = expected.includes("import") && expected.includes("qrl");
    const oxcHasQrlImport = actual.includes("import") && actual.includes("qrl");
    if (swcHasQrlImport && !oxcHasQrlImport) {
      push("SEGMENT_MISSING_QRL_IMPORT");
      return results;
    }

    // Check for missing imports generally
    const swcImportCount = (expected.match(/^import\s/gm) || []).length;
    const oxcImportCount = (actual.match(/^import\s/gm) || []).length;
    if (swcImportCount > oxcImportCount) {
      push("SEGMENT_MISSING_IMPORTS");
      return results;
    }

    // Extra content in OXC
    if (actual.length > expected.length * 1.1) {
      push("SEGMENT_EXTRA_CONTENT");
      return results;
    }

    push("OTHER");
    return results;
  }

  // SWC has qrl import but OXC does not
  if (expected.includes("import") && expected.includes("qrl") && !actual.includes("qrl")) {
    push("SEGMENT_MISSING_QRL_IMPORT");
    return results;
  }

  // Check import count difference
  const swcImportCount = (expected.match(/^import\s/gm) || []).length;
  const oxcImportCount = (actual.match(/^import\s/gm) || []).length;
  if (swcImportCount > oxcImportCount) {
    push("SEGMENT_MISSING_IMPORTS");
    return results;
  }

  // Extra content
  if (actual.length > expected.length * 1.1) {
    push("SEGMENT_EXTRA_CONTENT");
    return results;
  }

  // Missing content
  if (expected.length > actual.length * 1.1) {
    push("SEGMENT_MISSING_CONTENT");
    return results;
  }

  push("OTHER");
  return results;
}

// Main
const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap")).sort();
const allDiffs: CategorizedDiff[] = [];
let totalCodeDiffs = 0;

for (const file of swcFiles) {
  const swcPath = path.join(SWC_DIR, file);
  const oxcPath = path.join(OXC_DIR, file);
  if (!fs.existsSync(oxcPath)) continue;

  const swcSnap = parseSnapFile(swcPath);
  const oxcSnap = parseSnapFile(oxcPath);
  const failures = compareFixture(swcSnap, oxcSnap);

  for (const f of failures) {
    if (f.category !== "code_diff") continue;
    totalCodeDiffs++;
    const cats = categorize(f, file.replace(".snap", ""));
    allDiffs.push(...cats);
  }
}

// Aggregate by root cause
const counts = new Map<string, number>();
const examples = new Map<string, CategorizedDiff[]>();

for (const d of allDiffs) {
  counts.set(d.rootCause, (counts.get(d.rootCause) || 0) + 1);
  const existing = examples.get(d.rootCause) || [];
  if (existing.length < 3) existing.push(d);
  examples.set(d.rootCause, existing);
}

// Sort by count descending
const sorted = [...counts.entries()].sort((a, b) => b[1] - a[1]);

// Generate DIAGNOSIS.md
let md = `# Phase 28 Code Generation — Diagnosis\n\n`;
md += `**Generated:** ${new Date().toISOString()}\n`;
md += `**Total code_diff failures:** ${totalCodeDiffs}\n\n`;

md += `## Summary Table\n\n`;
md += `| # | Root Cause | Count | % |\n`;
md += `|---|-----------|-------|---|\n`;
for (const [i, [cause, count]] of sorted.entries()) {
  md += `| ${i + 1} | ${cause} | ${count} | ${((count / totalCodeDiffs) * 100).toFixed(1)}% |\n`;
}

md += `\n## Fix Priority (Ranked by Impact)\n\n`;
for (const [i, [cause, count]] of sorted.entries()) {
  md += `${i + 1}. **${cause}** (${count} diffs) — `;
  switch (cause) {
    case "SEGMENT_JSX_NOT_TRANSFORMED":
      md += "Segment modules retain raw JSX instead of _jsxSorted/_jsxSplit calls. Fix: disable suppress_jsx_conversion for segment closure bodies.";
      break;
    case "PARENT_EXTRA_QRL_HOISTS":
      md += "OXC parent modules contain QRL consts for event handlers that belong in segments. Fix: filter QRLs in exit_program to exclude segment-scoped ones.";
      break;
    case "PARENT_UNWANTED_JSX_IMPORT":
      md += "OXC parent imports _jsxSorted/_jsxSplit when SWC preserves raw JSX. Fix: only add JSX imports when JSX transform actually runs.";
      break;
    case "PARENT_EXTRA_IMPORTS":
      md += "OXC parent keeps original $-suffixed imports that SWC strips. Fix: remove converted specifiers from parent imports.";
      break;
    case "SEGMENT_MISSING_IMPORTS":
      md += "Segment modules missing imports (cascades from JSX transform fix). Fix: add _jsxSorted/_jsxSplit imports to segments.";
      break;
    case "SEGMENT_MISSING_QRL_IMPORT":
      md += "Segment modules missing qrl import for hoisted event handler QRLs. Fix: add qrl import when segment contains QRL hoists.";
      break;
    default:
      md += "See examples below.";
  }
  md += "\n";
}

md += `\n## Examples by Category\n\n`;
for (const [cause] of sorted) {
  const exs = examples.get(cause) || [];
  md += `### ${cause}\n\n`;
  for (const ex of exs) {
    md += `**Fixture:** ${ex.fixture}\n`;
    md += `**Type:** ${ex.field === "parentModule" ? "Parent module" : "Segment module"}\n\n`;
    md += `<details><summary>Expected (SWC) — first 300 chars</summary>\n\n\`\`\`\n${ex.expectedSnippet}\n\`\`\`\n</details>\n\n`;
    md += `<details><summary>Actual (OXC) — first 300 chars</summary>\n\n\`\`\`\n${ex.actualSnippet}\n\`\`\`\n</details>\n\n`;
  }
}

fs.mkdirSync(path.dirname(OUTPUT), { recursive: true });
fs.writeFileSync(OUTPUT, md);
console.log(`Wrote DIAGNOSIS.md with ${totalCodeDiffs} code_diff failures across ${sorted.length} categories`);
console.log("Category breakdown:");
for (const [cause, count] of sorted) {
  console.log(`  ${cause}: ${count}`);
}
