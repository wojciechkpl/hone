// Tiny in-house markdown renderer.
//
// The full Phase 2 design pulls in Shiki (lazy WASM) for syntax
// highlighting. Phase 1 ships a much smaller subset with no
// dependencies — fenced code blocks, inline code, bold, italic. That's
// enough to render most assistant responses readably without paying
// the bundle cost of a full markdown parser.

export type Node =
  | { kind: "text"; value: string }
  | { kind: "bold"; value: string }
  | { kind: "italic"; value: string }
  | { kind: "code"; value: string }
  | { kind: "block"; lang: string; value: string };

/**
 * Lex `text` into a flat node list. Block-level (fenced code) is
 * separated first; inline spans are scanned within each text run.
 *
 * The lexer is non-strict: malformed runs (an unclosed `**`, a stray
 * `\``) just stay as plain text rather than rejecting the whole turn.
 */
export function parseMarkdown(text: string): Node[] {
  const out: Node[] = [];
  // Split off fenced code blocks first.  ```lang\n...\n```
  const blockRe = /```([a-zA-Z0-9_+\-]*)\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = blockRe.exec(text)) !== null) {
    const before = text.slice(lastIndex, match.index);
    if (before) out.push(...parseInline(before));
    const lang = match[1] ?? "";
    const value = match[2] ?? "";
    out.push({ kind: "block", lang, value });
    lastIndex = match.index + match[0].length;
  }
  const tail = text.slice(lastIndex);
  if (tail) out.push(...parseInline(tail));
  return out;
}

function parseInline(text: string): Node[] {
  const out: Node[] = [];
  // Tokenise inline runs: code (`x`), bold (**x**), italic (*x* or _x_), text.
  // Greedy left-to-right; longest-match priority.
  const re = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\*[^*\n]+\*|_[^_\n]+_)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    if (match.index > lastIndex) {
      out.push({ kind: "text", value: text.slice(lastIndex, match.index) });
    }
    const tok = match[0];
    if (tok.startsWith("`")) {
      out.push({ kind: "code", value: tok.slice(1, -1) });
    } else if (tok.startsWith("**")) {
      out.push({ kind: "bold", value: tok.slice(2, -2) });
    } else {
      // *x* or _x_
      out.push({ kind: "italic", value: tok.slice(1, -1) });
    }
    lastIndex = match.index + tok.length;
  }
  if (lastIndex < text.length) {
    out.push({ kind: "text", value: text.slice(lastIndex) });
  }
  return out;
}
