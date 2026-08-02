// Unified-diff parsing, as a pure function over the patch text.
//
// Kept separate from the component because it is the only part of the panel with a
// right answer: given a patch, the line numbers down each gutter are either the ones
// git would print or they are wrong. Rendering is taste; this is arithmetic, and
// arithmetic gets a test (`diff.test.ts`).

export type DiffLineKind = "meta" | "hunk" | "add" | "del" | "context";

export interface DiffLine {
  readonly kind: DiffLineKind;
  readonly text: string;
  /** Line number in the pre-image, or null where the line does not exist there. */
  readonly oldLine: number | null;
  /** Line number in the post-image, or null where the line does not exist there. */
  readonly newLine: number | null;
}

/** `@@ -oldStart,oldCount +newStart,newCount @@ trailing` -- counts are optional. */
const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * Splits a patch into numbered lines.
 *
 * Everything before the first hunk header is `meta` (the `diff --git`, `index`, `---`
 * and `+++` lines) and carries no numbers. Inside a hunk the two counters advance
 * independently, which is the whole reason this is not a map over `split("\n")`.
 *
 * A `\ No newline at end of file` marker belongs to neither side and advances neither
 * counter; git emits it after the line it qualifies.
 */
export function parsePatch(patch: string): DiffLine[] {
  const lines: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;

  // A trailing newline would otherwise produce a phantom final row.
  const source = patch.endsWith("\n") ? patch.slice(0, -1) : patch;
  if (source.length === 0) return lines;

  for (const text of source.split("\n")) {
    const hunk = HUNK.exec(text);
    if (hunk !== null) {
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[3]);
      inHunk = true;
      lines.push({ kind: "hunk", text, oldLine: null, newLine: null });
      continue;
    }
    if (!inHunk) {
      lines.push({ kind: "meta", text, oldLine: null, newLine: null });
      continue;
    }
    if (text.startsWith("\\")) {
      lines.push({ kind: "meta", text, oldLine: null, newLine: null });
      continue;
    }
    if (text.startsWith("+")) {
      lines.push({ kind: "add", text: text.slice(1), oldLine: null, newLine });
      newLine += 1;
      continue;
    }
    if (text.startsWith("-")) {
      lines.push({ kind: "del", text: text.slice(1), oldLine, newLine: null });
      oldLine += 1;
      continue;
    }
    // Context. git prefixes it with a space; a truly empty line arrives empty.
    lines.push({
      kind: "context",
      text: text.startsWith(" ") ? text.slice(1) : text,
      oldLine,
      newLine,
    });
    oldLine += 1;
    newLine += 1;
  }
  return lines;
}
