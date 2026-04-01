/**
 * Snap file parser: converts .snap golden files into typed ParsedSnapshot structures.
 *
 * Uses a line-by-line state machine scanner. Section boundaries are detected via
 * regex matches on individual lines — no full-document regex.
 *
 * Verified against all 201 .snap files in swc-snapshots/.
 */

import * as fs from "node:fs";
import * as path from "node:path";
import type { ParsedSection, ParsedSnapshot, SegmentMetadata } from "./types.js";

// Section boundary patterns (verified against all 201 snap files)
const SEGMENT_HEADER_RE = /^={10,} (.+?) \(ENTRY POINT\)==$/;
const CONTENT_HEADER_RE = /^={10,} (.+?) ==$/;
const INPUT_RE = /^==INPUT==$/;
const DIAGNOSTICS_RE = /^== DIAGNOSTICS ==$/;
const FRONTMATTER_RE = /^---$/;
const SOURCE_MAP_RE = /^Some\("/;

type ScanState = "frontmatter" | "frontmatter_done" | "input" | "section" | "diagnostics";

/**
 * Find the metadata block (/* { ... } *\/) in a section's body lines.
 * Returns { start: index of the "/*" line, json: JSON text } or null if not found.
 */
function extractMetadataBlock(lines: string[]): { start: number; json: string } | null {
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].trim() !== "/*") continue;
    const next = (lines[i + 1] ?? "").trimStart();
    if (!next.startsWith("{")) continue;
    // Find the closing */
    for (let j = i + 1; j < lines.length; j++) {
      if (lines[j].trim() === "*/") {
        return { start: i, json: lines.slice(i + 1, j).join("\n") };
      }
    }
  }
  return null;
}

/**
 * Parse a metadata JSON string into a typed SegmentMetadata object.
 * paramNames and captureNames are left undefined when absent (NOT defaulted to []).
 */
function parseMetadata(jsonText: string): SegmentMetadata {
  const raw = JSON.parse(jsonText) as Record<string, unknown>;
  return {
    origin: raw["origin"] as string,
    name: raw["name"] as string,
    entry: raw["entry"] as string | null,
    displayName: raw["displayName"] as string,
    hash: raw["hash"] as string,
    canonicalFilename: raw["canonicalFilename"] as string,
    path: raw["path"] as string,
    extension: raw["extension"] as string,
    parent: raw["parent"] as string | null,
    ctxKind: raw["ctxKind"] as string,
    ctxName: raw["ctxName"] as string,
    captures: raw["captures"] as boolean,
    loc: raw["loc"] as [number, number],
    paramNames: raw["paramNames"] as string[] | undefined,
    captureNames: raw["captureNames"] as string[] | undefined,
  };
}

/**
 * Extract the source map value from a section's body lines.
 * Source map is a single line matching Some("..."). Returns null if absent.
 */
function extractSourceMap(lines: string[]): string | null {
  const line = lines.find(l => SOURCE_MAP_RE.test(l));
  return line ?? null;
}

/**
 * Finalize a content section from accumulated body lines.
 * Sections with a metadata block are segment sections; those without are parent sections.
 */
function buildSection(
  headerName: string,
  isEntryPoint: boolean,
  bodyLines: string[]
): ParsedSection {
  const metaBlock = extractMetadataBlock(bodyLines);
  // Code = lines before the metadata block start, with source map line excluded
  const codeLines = metaBlock
    ? bodyLines.slice(0, metaBlock.start)
    : bodyLines;
  const code = codeLines
    .filter(l => !SOURCE_MAP_RE.test(l))
    .join("\n")
    .trim();

  return {
    headerName,
    isEntryPoint,
    code,
    sourceMap: extractSourceMap(bodyLines),
    metadata: metaBlock ? parseMetadata(metaBlock.json) : null,
  };
}

/**
 * Parse a single .snap file into a ParsedSnapshot.
 *
 * @param filePath - Absolute or relative path to the .snap file.
 * @returns Typed ParsedSnapshot with all sections and metadata.
 */
export function parseSnapFile(filePath: string): ParsedSnapshot {
  const content = fs.readFileSync(filePath, "utf8");
  const lines = content.split("\n");
  const fixtureName = path.basename(filePath, ".snap");

  let state: ScanState = "frontmatter";
  let frontmatterCount = 0;

  let input = "";
  const inputLines: string[] = [];

  let currentSectionName = "";
  let currentIsEntryPoint = false;
  let currentBodyLines: string[] = [];

  const sections: ParsedSection[] = [];
  const diagnosticsLines: string[] = [];

  function flushSection() {
    if (!currentSectionName) return;
    sections.push(buildSection(currentSectionName, currentIsEntryPoint, currentBodyLines));
    currentSectionName = "";
    currentBodyLines = [];
  }

  for (const line of lines) {
    // --- Frontmatter skip ---
    if (state === "frontmatter") {
      if (FRONTMATTER_RE.test(line)) {
        frontmatterCount++;
        if (frontmatterCount === 2) state = "frontmatter_done";
      }
      continue;
    }

    // --- Section header detection (any state after frontmatter) ---
    if (INPUT_RE.test(line)) {
      flushSection();
      state = "input";
      continue;
    }

    if (DIAGNOSTICS_RE.test(line)) {
      flushSection();
      state = "diagnostics";
      continue;
    }

    const segMatch = line.match(SEGMENT_HEADER_RE);
    if (segMatch) {
      flushSection();
      currentSectionName = segMatch[1]!;
      currentIsEntryPoint = true;
      currentBodyLines = [];
      state = "section";
      continue;
    }

    const contMatch = line.match(CONTENT_HEADER_RE);
    if (contMatch) {
      flushSection();
      currentSectionName = contMatch[1]!;
      currentIsEntryPoint = false;
      currentBodyLines = [];
      state = "section";
      continue;
    }

    // --- Accumulate lines for current section ---
    if (state === "input") {
      inputLines.push(line);
    } else if (state === "section") {
      currentBodyLines.push(line);
    } else if (state === "diagnostics") {
      diagnosticsLines.push(line);
    }
  }

  // Flush any remaining section at EOF
  flushSection();

  input = inputLines.join("\n").trim();

  const diagText = diagnosticsLines.join("\n").trim();
  let diagnostics: unknown[] = [];
  if (diagText) {
    try {
      diagnostics = JSON.parse(diagText) as unknown[];
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      throw new Error(
        `Malformed diagnostics JSON in ${fixtureName}: ${msg}\nRaw text: ${diagText.slice(0, 200)}`
      );
    }
  }

  return { fixtureName, input, sections, diagnostics };
}

