import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { send, subscribe } from "./bridge";
import { parsePatch, type DiffLine } from "./diff";
import type { ChangedFile, Theme } from "./protocol";

function applyTheme(theme: Theme): void {
  const root = document.documentElement.style;
  root.setProperty("--bg", theme.background);
  root.setProperty("--fg", theme.foreground);
  root.setProperty("--accent", theme.accent);
  root.setProperty("--dim", theme.dim);
}

/** `crates/host/src` + `lib.rs`, so the leaf reads at full contrast and the path dims. */
function splitPath(path: string): { dir: string; name: string } {
  const cut = path.lastIndexOf("/");
  return cut < 0
    ? { dir: "", name: path }
    : { dir: path.slice(0, cut + 1), name: path.slice(cut + 1) };
}

function FileRow({
  file,
  selected,
  onSelect,
}: {
  file: ChangedFile;
  selected: boolean;
  onSelect: () => void;
}) {
  const { dir, name } = splitPath(file.path);
  return (
    <button className="file" aria-selected={selected} onClick={onSelect} type="button">
      <span className="status">{file.status}</span>
      {/* The bidi isolate keeps an RTL filename from dragging the separator around
          inside a row that is itself reversed for left-truncation. */}
      <span className="path" title={file.path}>
        &#x2068;
        <span className="dir">{dir}</span>
        {name}&#x2069;
      </span>
      <span className="stat">
        <span className="plus">+{file.additions}</span> <span className="minus">-{file.deletions}</span>
      </span>
    </button>
  );
}

function DiffRow({ line }: { line: DiffLine }) {
  const sign = line.kind === "add" ? "+" : line.kind === "del" ? "-" : " ";
  return (
    <div className={`row ${line.kind}`}>
      <span className="num">{line.oldLine ?? ""}</span>
      <span className="num">{line.newLine ?? ""}</span>
      <span className="sign">{line.kind === "hunk" || line.kind === "meta" ? "" : sign}</span>
      <span className="text">{line.text || " "}</span>
    </div>
  );
}

export function DiffPanel() {
  const [repo, setRepo] = useState<string | null>(null);
  const [files, setFiles] = useState<readonly ChangedFile[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [patch, setPatch] = useState("");
  const [error, setError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const select = useCallback((path: string) => {
    setSelected(path);
    setPatch("");
    send({ kind: "requestDiff", path });
  }, []);

  useEffect(() => {
    const unsubscribe = subscribe((message) => {
      switch (message.kind) {
        case "init":
          applyTheme(message.theme);
          break;
        case "files": {
          setRepo(message.repo);
          setFiles(message.files);
          setError(message.error);
          // Keep the open file open across a refresh; otherwise show the first.
          setSelected((current) => {
            const keep = current !== null && message.files.some((f) => f.path === current);
            const next = keep ? current : (message.files[0]?.path ?? null);
            if (next !== null && next !== current) send({ kind: "requestDiff", path: next });
            if (next === null) setPatch("");
            return next;
          });
          break;
        }
        case "fileDiff":
          // A late reply for a file the user already navigated away from must not
          // overwrite the current one.
          setSelected((current) => {
            if (message.path === current) {
              setPatch(message.patch);
              setError(message.error);
            }
            return current;
          });
          break;
      }
    });
    send({ kind: "ready" });
    return unsubscribe;
  }, []);

  const move = useCallback(
    (delta: number) => {
      if (files.length === 0) return;
      const at = files.findIndex((file) => file.path === selected);
      const next = files[Math.min(files.length - 1, Math.max(0, (at < 0 ? 0 : at) + delta))];
      if (next !== undefined && next.path !== selected) select(next.path);
    },
    [files, selected, select],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") return send({ kind: "dismiss" }), undefined;
      if (event.key === "r" && !event.metaKey) return send({ kind: "refresh" }), undefined;
      if (event.key === "ArrowDown" || event.key === "j") {
        event.preventDefault();
        move(1);
      }
      if (event.key === "ArrowUp" || event.key === "k") {
        event.preventDefault();
        move(-1);
      }
      return undefined;
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [move]);

  const lines = useMemo(() => parsePatch(patch), [patch]);

  return (
    <div className="panel">
      <div className="head">
        <span className="title">Changes</span>
        {/* The isolate is load-bearing, not decoration. `.repo` is `direction: rtl` so
            that a long path truncates from the LEFT, and under RTL the leading "/" of an
            absolute path is a neutral that resolves to the paragraph direction and gets
            reordered to the far end: `/Users/orel/x` renders as `Users/orel/x/`. Seen in
            the first live capture, 2026-08-02. Isolating the text keeps the container's
            truncation behaviour while resolving the path's own direction as LTR. */}
        <span className="repo">
          &#x2068;{repo ?? "not a git repository"}&#x2069;
        </span>
        <button type="button" onClick={() => send({ kind: "refresh" })}>
          Refresh
        </button>
        <button type="button" onClick={() => send({ kind: "dismiss" })}>
          Close
        </button>
      </div>
      {error !== null && <p className="error">{error}</p>}
      <div className="body">
        <div className="files" ref={listRef}>
          {files.map((file) => (
            <FileRow
              key={file.path}
              file={file}
              selected={file.path === selected}
              onSelect={() => select(file.path)}
            />
          ))}
          {files.length === 0 && error === null && (
            <div className="empty">{repo === null ? "no repository here" : "working tree clean"}</div>
          )}
        </div>
        <div className="diff">
          {lines.length === 0 ? (
            <div className="empty">{selected === null ? "nothing selected" : "no textual diff"}</div>
          ) : (
            lines.map((line, index) => <DiffRow key={index} line={line} />)
          )}
        </div>
      </div>
    </div>
  );
}
