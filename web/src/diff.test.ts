import { expect, test } from "bun:test";
import { parsePatch } from "./diff";

// A real `git diff` payload: two hunks, so the second one's numbering can only be
// right if the header reset it rather than the counters running on from the first.
const PATCH = `diff --git a/src/lib.rs b/src/lib.rs
index 1234567..89abcde 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,5 @@
 use std::fmt;
-fn old() {}
+fn new() {}
+fn extra() {}

@@ -40,3 +41,3 @@ impl Thing {
     fn keep(&self) {}
-    fn was(&self) {}
+    fn is(&self) {}
`;

test("meta lines carry no numbers", () => {
  const lines = parsePatch(PATCH);
  const meta = lines.filter((line) => line.kind === "meta");
  expect(meta).toHaveLength(4);
  expect(meta.every((line) => line.oldLine === null && line.newLine === null)).toBe(true);
});

test("an added line numbers only the new side, a removed line only the old", () => {
  const lines = parsePatch(PATCH);
  const added = lines.filter((line) => line.kind === "add");
  const removed = lines.filter((line) => line.kind === "del");

  expect(added.map((line) => [line.text, line.oldLine, line.newLine])).toEqual([
    ["fn new() {}", null, 2],
    ["fn extra() {}", null, 3],
    ["    fn is(&self) {}", null, 42],
  ]);
  expect(removed.map((line) => [line.text, line.oldLine, line.newLine])).toEqual([
    ["fn old() {}", 2, null],
    ["    fn was(&self) {}", 41, null],
  ]);
});

// The discriminating one. A parser that ignores the second @@ header and just keeps
// counting produces old 6 / new 7 here instead of 40 / 41 -- and every assertion above
// still passes, because the first hunk is untouched by that bug.
test("the second hunk header resets both counters", () => {
  const lines = parsePatch(PATCH);
  const secondHunk = lines.filter((line) => line.kind === "hunk")[1];
  expect(secondHunk?.text).toContain("-40,3 +41,3");

  const contextAfter = lines
    .slice(lines.indexOf(secondHunk!) + 1)
    .find((line) => line.kind === "context");
  expect([contextAfter?.oldLine, contextAfter?.newLine]).toEqual([40, 41]);
});

test("context advances both counters together", () => {
  const lines = parsePatch(PATCH);
  const firstContext = lines.find((line) => line.kind === "context");
  expect([firstContext?.text, firstContext?.oldLine, firstContext?.newLine]).toEqual([
    "use std::fmt;",
    1,
    1,
  ]);
});

test("a no-newline marker belongs to neither side and advances neither counter", () => {
  const lines = parsePatch(`@@ -1,1 +1,1 @@
-a
\\ No newline at end of file
+b
`);
  const marker = lines.find((line) => line.text.startsWith("\\"));
  expect(marker?.kind).toBe("meta");
  const added = lines.find((line) => line.kind === "add");
  expect(added?.newLine).toBe(1);
});

test("an empty patch yields no lines rather than one phantom row", () => {
  expect(parsePatch("")).toEqual([]);
  expect(parsePatch("\n")).toEqual([]);
});
