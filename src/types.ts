/**
 * Shared types for ParsedSnapshot structures.
 * All 15 SegmentMetadata fields are camelCase to match the actual JSON in .snap files.
 */

/** All metadata fields from a segment section's JSON block. */
export interface SegmentMetadata {
  /** Relative source path (e.g., "test.tsx") */
  origin: string;
  /** Symbol name: {displayName}_{hash} or s_{hash} in Prod mode */
  name: string;
  /** null or a string bundle key */
  entry: string | null;
  /** Human-readable name prefix */
  displayName: string;
  /** 11-char base64 hash */
  hash: string;
  /** {displayName}_{hash} */
  canonicalFilename: string;
  /** Directory path relative to src_dir, often "" */
  path: string;
  /** File extension: "tsx", "ts", "js", etc. */
  extension: string;
  /** null or parent segment name */
  parent: string | null;
  /** "function" or "eventHandler" */
  ctxKind: string;
  /** The $-suffixed function name */
  ctxName: string;
  /** Whether the segment captures variables */
  captures: boolean;
  /** [startByte, endByte] */
  loc: [number, number];
  /** Array of parameter name strings; absent when no params (148/368 in corpus) */
  paramNames?: string[];
  /** Array of captured variable names; absent when no captures (57/368 in corpus) */
  captureNames?: string[];
}

/** A single parsed content section from a .snap file. */
export interface ParsedSection {
  /** Section header filename/identifier */
  headerName: string;
  /** true if (ENTRY POINT) in header */
  isEntryPoint: boolean;
  /** Code block (source map line excluded) */
  code: string;
  /** Some("...VLQ...") block, null if absent */
  sourceMap: string | null;
  /** null for parent sections (no metadata block); populated for segment sections */
  metadata: SegmentMetadata | null;
}

/** The complete parsed representation of a .snap golden file. */
export interface ParsedSnapshot {
  /** Basename without .snap extension */
  fixtureName: string;
  /** Content of the ==INPUT== section */
  input: string;
  /** All content sections in order (segment + parent sections) */
  sections: ParsedSection[];
  /** Raw parsed JSON from DIAGNOSTICS section */
  diagnostics: unknown[];
}
