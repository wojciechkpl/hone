// Tiny unified-diff parser.
//
// Just enough to render `edit_file` tool results as side-by-side
// hunks. Not strict — partial diffs (e.g. a single hunk without file
// headers) parse fine; the renderer ignores anything it doesn't
// understand. No external dep.

export type DiffLine =
  | { kind: "add"; text: string }
  | { kind: "remove"; text: string }
  | { kind: "context"; text: string };

export type DiffHunk = {
  /** Header line as it appeared (`@@ -1,3 +1,4 @@ ...`), informational. */
  header: string;
  /** The raw line offsets from the `@@ -<old>,<oldN> +<new>,<newN> @@` token. */
  oldStart: number;
  oldLen: number;
  newStart: number;
  newLen: number;
  lines: DiffLine[];
};

export type DiffFile = {
  /** Path on the "before" side, parsed from `--- a/...`. */
  oldPath: string | null;
  /** Path on the "after" side, parsed from `+++ b/...`. */
  newPath: string | null;
  hunks: DiffHunk[];
};

/** Best-effort: returns true if `text` looks like a unified diff. */
export function looksLikeDiff(text: string): boolean {
  return /^@@\s/.test(text) || /^---\s/.test(text) || /^\+\+\+\s/.test(text);
}

const HUNK_HEADER_RE = /^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@/;

export function parseUnifiedDiff(text: string): DiffFile[] {
  const lines = text.split(/\r?\n/);
  const files: DiffFile[] = [];
  let cur: DiffFile | null = null;
  let curHunk: DiffHunk | null = null;

  function pushHunk() {
    if (curHunk && cur) cur.hunks.push(curHunk);
    curHunk = null;
  }

  for (const raw of lines) {
    if (raw.startsWith("--- ")) {
      pushHunk();
      cur = { oldPath: stripDiffHeader(raw.slice(4)), newPath: null, hunks: [] };
      files.push(cur);
      continue;
    }
    if (raw.startsWith("+++ ")) {
      if (!cur) {
        cur = { oldPath: null, newPath: null, hunks: [] };
        files.push(cur);
      }
      cur.newPath = stripDiffHeader(raw.slice(4));
      continue;
    }
    const hunkMatch = raw.match(HUNK_HEADER_RE);
    if (hunkMatch) {
      pushHunk();
      if (!cur) {
        cur = { oldPath: null, newPath: null, hunks: [] };
        files.push(cur);
      }
      curHunk = {
        header: raw,
        oldStart: parseInt(hunkMatch[1] ?? "0", 10),
        oldLen: parseInt(hunkMatch[2] ?? "1", 10),
        newStart: parseInt(hunkMatch[3] ?? "0", 10),
        newLen: parseInt(hunkMatch[4] ?? "1", 10),
        lines: [],
      };
      continue;
    }
    if (!curHunk) continue;
    if (raw.startsWith("+")) curHunk.lines.push({ kind: "add", text: raw.slice(1) });
    else if (raw.startsWith("-")) curHunk.lines.push({ kind: "remove", text: raw.slice(1) });
    else if (raw.startsWith(" ")) curHunk.lines.push({ kind: "context", text: raw.slice(1) });
    // Anything else (the `\ No newline at end of file` marker, blank
    // lines between hunks) is silently dropped.
  }
  pushHunk();
  return files;
}

function stripDiffHeader(field: string): string {
  // Common forms: `a/path/to/file`, `b/path/to/file`, or `path/to/file`.
  // Drop the `a/` or `b/` prefix when present; the user cares about
  // the path, not git's left/right convention.
  const trimmed = field.trim();
  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) {
    return trimmed.slice(2);
  }
  return trimmed;
}

/** Pair add/remove lines positionally so a side-by-side renderer can
 * walk them. The pairing is greedy: consecutive remove+add runs are
 * zipped together; surplus lines on either side fill in as one-sided
 * rows. */
export type SideBySideRow =
  | { kind: "context"; left: string; right: string }
  | { kind: "removed"; left: string }
  | { kind: "added"; right: string }
  | { kind: "modified"; left: string; right: string };

export function pairLines(hunk: DiffHunk): SideBySideRow[] {
  const rows: SideBySideRow[] = [];
  const lines = hunk.lines;
  let i = 0;
  while (i < lines.length) {
    const cur = lines[i];
    if (!cur) {
      i++;
      continue;
    }
    if (cur.kind === "context") {
      rows.push({ kind: "context", left: cur.text, right: cur.text });
      i++;
      continue;
    }
    // Collect a run of removes followed (optionally) by adds.
    const removes: string[] = [];
    while (i < lines.length && lines[i]?.kind === "remove") {
      removes.push(lines[i]!.text);
      i++;
    }
    const adds: string[] = [];
    while (i < lines.length && lines[i]?.kind === "add") {
      adds.push(lines[i]!.text);
      i++;
    }
    // Zip the run.
    const max = Math.max(removes.length, adds.length);
    for (let j = 0; j < max; j++) {
      const l = removes[j];
      const r = adds[j];
      if (l !== undefined && r !== undefined) {
        rows.push({ kind: "modified", left: l, right: r });
      } else if (l !== undefined) {
        rows.push({ kind: "removed", left: l });
      } else if (r !== undefined) {
        rows.push({ kind: "added", right: r });
      }
    }
  }
  return rows;
}
