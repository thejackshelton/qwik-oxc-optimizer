/**
 * Normalizer module: formats code through oxfmt 0.32.0 for symmetric comparison.
 *
 * Purpose: Eliminate cosmetic whitespace/formatting differences between SWC and OXC
 * code output so that only semantic differences surface during comparison (Phase 4).
 *
 * Uses oxfmt's JS API directly (no process spawning). warmNormalizationCache()
 * pre-normalizes all code blocks via Promise.all. After warming, normalizeCode()
 * is a synchronous cache lookup.
 *
 * NOTE: Source maps are already stripped by the parser (parser.ts extracts sourceMap
 * into ParsedSection.sourceMap and excludes it from ParsedSection.code). This module
 * does NOT need to strip source maps.
 *
 * NOTE: loc metadata fields live in SegmentMetadata, not in code strings. Normalization
 * cannot and does not affect loc values.
 */

import { format } from "oxfmt";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { OXFMT_VERSION } from "./contract.js";

// Resolve path to the oxfmt binary for version assertion only
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OXFMT_BIN = path.join(__dirname, "..", "node_modules", ".bin", "oxfmt");

// In-memory normalization cache: "filepath\0code" -> normalized code
const cache = new Map<string, string>();

function cacheKey(code: string, filepath: string): string {
  return `${filepath}\0${code}`;
}

/**
 * Assert that the installed oxfmt binary matches the pinned OXFMT_VERSION from contract.ts.
 * Throws if version does not match or oxfmt cannot be invoked.
 *
 * @throws {Error} If oxfmt is missing, cannot be run, or its version does not match.
 */
export function assertOxfmtVersion(): void {
  const res = spawnSync(OXFMT_BIN, ["--version"], { encoding: "utf8" });
  if (res.error) {
    throw new Error(`oxfmt binary not found or could not be executed: ${res.error.message}`);
  }
  if (res.status !== 0) {
    throw new Error(`oxfmt --version exited with status ${res.status}: ${(res.stderr || "").trim()}`);
  }
  const output = (res.stdout + res.stderr).trim();
  const match = output.match(/(\d+\.\d+\.\d+)/);
  if (!match) {
    throw new Error(`Could not parse oxfmt version from output: ${output}`);
  }
  const installedVersion = match[1];
  if (installedVersion !== OXFMT_VERSION) {
    throw new Error(
      `oxfmt version mismatch: expected ${OXFMT_VERSION}, got ${installedVersion}. ` +
        `Update devDependencies.oxfmt in package.json and re-run npm install.`
    );
  }
}

/**
 * Format code through oxfmt (synchronous cache lookup).
 *
 * Empty or whitespace-only code is returned unchanged.
 * After warmNormalizationCache() has been called, this is a pure cache lookup.
 * Throws on cache miss — call warmNormalizationCache() first in the CLI pipeline.
 *
 * @param code - Source code string.
 * @param stdinFilepath - Fake file path whose extension tells oxfmt how to parse the code.
 * @returns Formatted code string (trailing newline stripped).
 * @throws {Error} If the code is not in the cache and cannot be formatted synchronously.
 */
export function normalizeCode(code: string, stdinFilepath: string): string {
  if (code.trim().length === 0) {
    return code;
  }

  const key = cacheKey(code, stdinFilepath);
  const cached = cache.get(key);
  if (cached !== undefined) return cached;

  // Fallback: synchronous spawn for non-warmed paths (tests, idempotency check)
  const res = spawnSync(OXFMT_BIN, ["--stdin-filepath", stdinFilepath], {
    input: code + "\n",
    encoding: "utf8",
    maxBuffer: 50 * 1024 * 1024,
  });

  if (res.error) {
    throw new Error(
      `oxfmt process error for ${stdinFilepath}: ${res.error.message}`
    );
  }

  if (res.status !== 0) {
    const stderr = (res.stderr ?? "").trim();
    throw new Error(
      `oxfmt exited with status ${res.status} for ${stdinFilepath}` +
        (stderr ? `\nstderr: ${stderr}` : "")
    );
  }

  const result = res.stdout.replace(/\n$/, "");
  cache.set(key, result);
  return result;
}

/**
 * Format a single code block via the oxfmt JS API (async, no process spawn).
 */
async function normalizeCodeViaApi(code: string, filepath: string): Promise<string> {
  const result = await format(filepath, code + "\n");
  if (result.errors.length > 0) {
    throw new Error(
      `oxfmt format error for ${filepath}: ${result.errors.map((e) => e.message).join("; ")}`
    );
  }
  return result.code.replace(/\n$/, "");
}

/**
 * Attempt async normalization with .tsx fallback (mirrors tryNormalizeCode logic).
 */
async function tryNormalizeViaApi(code: string, filepath: string): Promise<string> {
  try {
    return await normalizeCodeViaApi(code, filepath);
  } catch {
    try {
      const fallbackPath = filepath.replace(/\.[^.]+$/, ".tsx");
      if (fallbackPath !== filepath) {
        return await normalizeCodeViaApi(code, fallbackPath);
      }
    } catch {
      // Both attempts failed
    }
    return code.trim();
  }
}

/**
 * Pre-warm the normalization cache using the oxfmt JS API.
 * All code blocks are normalized via Promise.all (no process spawning).
 * After this returns, normalizeCode() is a synchronous cache lookup.
 *
 * @param entries - Array of {code, filepath} pairs to normalize
 */
export async function warmNormalizationCache(
  entries: Array<{ code: string; filepath: string }>
): Promise<void> {
  // Deduplicate and filter empties
  const toProcess: Array<{ code: string; filepath: string; key: string }> = [];
  const seen = new Set<string>();
  for (const entry of entries) {
    if (entry.code.trim().length === 0) continue;
    const key = cacheKey(entry.code, entry.filepath);
    if (cache.has(key) || seen.has(key)) continue;
    seen.add(key);
    toProcess.push({ ...entry, key });
  }

  // All at once — no process spawning, just in-memory formatting
  const results = await Promise.all(
    toProcess.map(async (entry) => {
      const result = await tryNormalizeViaApi(entry.code, entry.filepath);
      return { key: entry.key, result };
    })
  );
  for (const { key, result } of results) {
    cache.set(key, result);
  }
}

/**
 * Assert that normalizeCode is idempotent: formatting already-formatted code
 * produces identical output.
 *
 * Uses a small representative TypeScript snippet for verification.
 * Throws if the double-format output differs from the single-format output.
 *
 * @throws {Error} If double-formatting produces a different result.
 */
export function assertNormalizerIdempotent(): void {
  const testCode = 'const greeting = "hello world";\nexport const add = (a: number, b: number): number => a + b;\n';
  const once = normalizeCode(testCode, "idempotency-check.ts");
  const twice = normalizeCode(once, "idempotency-check.ts");
  if (once !== twice) {
    throw new Error(
      `normalizeCode is not idempotent!\n` +
        `First pass:\n${once}\n\nSecond pass:\n${twice}`
    );
  }
}
