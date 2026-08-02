import { useEffect, useState } from "react";
import { send, subscribe } from "./bridge";
import type { WorkspaceRow } from "./protocol";

/// One worktree: its branch, the sessions open in it, and whether it is the active one.
function Row({ row }: { row: WorkspaceRow }) {
  return (
    <button
      className="workspace"
      aria-selected={row.isActive}
      type="button"
      onClick={() => send({ kind: "openWorkspace", path: row.path })}
      title={row.path}
    >
      <span className="wsHead">
        <span className="wsMark">{row.isPrimary ? "◆" : "⎇"}</span>
        <span className="wsName">{row.branch}</span>
        {row.sessions.length > 0 && <span className="wsCount">{row.sessions.length}</span>}
      </span>
      {row.sessions.length > 0 ? (
        <span className="wsSessions">{row.sessions.join(", ")}</span>
      ) : (
        // Naming the consequence rather than leaving the row silent: clicking a
        // workspace with no session opens one, and that is not obvious from a list.
        <span className="wsSessions wsIdle">no session - click to open</span>
      )}
    </button>
  );
}

export function WorkspacePanel() {
  const [repo, setRepo] = useState<string | null>(null);
  const [rows, setRows] = useState<readonly WorkspaceRow[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(
    () =>
      subscribe((message) => {
        if (message.kind !== "workspaces") return;
        setRepo(message.repo);
        setRows(message.rows);
        setError(message.error);
      }),
    [],
  );

  return (
    <div className="panel docked">
      <div className="head">
        <span className="title">Workspaces</span>
        <button type="button" onClick={() => send({ kind: "refresh" })}>
          &#x21BB;
        </button>
      </div>
      <p className="repo docked">
        &#x2068;{repo ?? "not a git repository"}&#x2069;
      </p>
      {error !== null && <p className="error">{error}</p>}
      <div className="workspaces">
        {rows.map((row) => (
          <Row key={row.path} row={row} />
        ))}
        {rows.length === 0 && error === null && (
          <div className="empty">no worktrees here</div>
        )}
      </div>
      <button className="wsNew" type="button" onClick={() => send({ kind: "createWorkspace" })}>
        + New workspace
      </button>
    </div>
  );
}
