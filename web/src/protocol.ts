// The wire contract between the Swift host and this panel, and the only place either
// side's message shapes are written down.
//
// Both directions are validated at the boundary, and a validation failure is a REPORTED
// event rather than a thrown one. That is the whole point of the file: a webview bridge
// fails silently by nature (postMessage to a handler that was never registered is a
// no-op, and a malformed payload arriving in JS is just `undefined` a few frames later),
// which is the SCAR-004 shape -- when it works you see nothing, and when it is dead you
// also see nothing. So every decode failure travels back to the host as a message, and
// the host writes it where a human will see it.

/** Terminal colours, so the panel belongs to the window instead of sitting on top of it. */
export interface Theme {
  readonly background: string;
  readonly foreground: string;
  readonly accent: string;
  readonly dim: string;
}

export interface ChangedFile {
  readonly path: string;
  /** git's two-letter porcelain status, e.g. " M", "??", "A ". */
  readonly status: string;
  readonly additions: number;
  readonly deletions: number;
}

/** Which view to render. One bundle serves every panel; the host picks at init. */
export type PanelKind = "diff" | "workspaces";

export interface WorkspaceRow {
  readonly branch: string;
  readonly path: string;
  readonly isPrimary: boolean;
  /** Titles of open sessions living in this worktree. */
  readonly sessions: readonly string[];
  readonly isActive: boolean;
}

export type HostMessage =
  | { readonly kind: "init"; readonly theme: Theme; readonly panel: PanelKind }
  | {
      readonly kind: "workspaces";
      readonly repo: string | null;
      readonly rows: readonly WorkspaceRow[];
      readonly error: string | null;
    }
  | {
      readonly kind: "files";
      readonly repo: string | null;
      readonly files: readonly ChangedFile[];
      readonly error: string | null;
    }
  | {
      readonly kind: "fileDiff";
      readonly path: string;
      readonly patch: string;
      readonly error: string | null;
    }
  /** The liveness probe the headless smoke asserts on. Never used by the UI. */
  | { readonly kind: "ping"; readonly nonce: string };

export type PanelMessage =
  | { readonly kind: "ready" }
  | { readonly kind: "refresh" }
  | { readonly kind: "dismiss" }
  | { readonly kind: "requestDiff"; readonly path: string }
  | { readonly kind: "openWorkspace"; readonly path: string }
  | { readonly kind: "createWorkspace" }
  | { readonly kind: "pong"; readonly nonce: string }
  | { readonly kind: "decodeError"; readonly detail: string };

/** A decode either yields a value or says why it could not, and never throws. */
export type Decoded<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: string };

const fail = (error: string): Decoded<never> => ({ ok: false, error });
const succeed = <T>(value: T): Decoded<T> => ({ ok: true, value });

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(
  source: Record<string, unknown>,
  key: string,
  where: string,
): Decoded<string> {
  const value = source[key];
  return typeof value === "string"
    ? succeed(value)
    : fail(`${where}.${key} must be a string, got ${typeof value}`);
}

function nullableStringField(
  source: Record<string, unknown>,
  key: string,
  where: string,
): Decoded<string | null> {
  const value = source[key];
  if (value === null) return succeed(null);
  return typeof value === "string"
    ? succeed(value)
    : fail(`${where}.${key} must be a string or null`);
}

function numberField(
  source: Record<string, unknown>,
  key: string,
  where: string,
): Decoded<number> {
  const value = source[key];
  return typeof value === "number" && Number.isFinite(value)
    ? succeed(value)
    : fail(`${where}.${key} must be a finite number`);
}

function decodeTheme(value: unknown): Decoded<Theme> {
  if (!isRecord(value)) return fail("init.theme must be an object");
  const background = stringField(value, "background", "theme");
  if (!background.ok) return background;
  const foreground = stringField(value, "foreground", "theme");
  if (!foreground.ok) return foreground;
  const accent = stringField(value, "accent", "theme");
  if (!accent.ok) return accent;
  const dim = stringField(value, "dim", "theme");
  if (!dim.ok) return dim;
  return succeed({
    background: background.value,
    foreground: foreground.value,
    accent: accent.value,
    dim: dim.value,
  });
}

function decodeChangedFile(value: unknown, index: number): Decoded<ChangedFile> {
  const where = `files[${index}]`;
  if (!isRecord(value)) return fail(`${where} must be an object`);
  const path = stringField(value, "path", where);
  if (!path.ok) return path;
  const status = stringField(value, "status", where);
  if (!status.ok) return status;
  const additions = numberField(value, "additions", where);
  if (!additions.ok) return additions;
  const deletions = numberField(value, "deletions", where);
  if (!deletions.ok) return deletions;
  return succeed({
    path: path.value,
    status: status.value,
    additions: additions.value,
    deletions: deletions.value,
  });
}

function decodeWorkspaceRow(value: unknown, index: number): Decoded<WorkspaceRow> {
  const where = `rows[${index}]`;
  if (!isRecord(value)) return fail(`${where} must be an object`);
  const branch = stringField(value, "branch", where);
  if (!branch.ok) return branch;
  const path = stringField(value, "path", where);
  if (!path.ok) return path;
  const isPrimary = value["isPrimary"];
  if (typeof isPrimary !== "boolean") return fail(`${where}.isPrimary must be a boolean`);
  const isActive = value["isActive"];
  if (typeof isActive !== "boolean") return fail(`${where}.isActive must be a boolean`);
  const raw = value["sessions"];
  if (!Array.isArray(raw)) return fail(`${where}.sessions must be an array`);
  const sessions: string[] = [];
  for (const entry of raw) {
    if (typeof entry !== "string") return fail(`${where}.sessions must hold strings`);
    sessions.push(entry);
  }
  return succeed({
    branch: branch.value,
    path: path.value,
    isPrimary,
    isActive,
    sessions,
  });
}

/** The one entry point for anything arriving from Swift. */
export function decodeHostMessage(value: unknown): Decoded<HostMessage> {
  if (!isRecord(value)) return fail("message must be an object");
  const kind = value["kind"];
  if (typeof kind !== "string") return fail("message.kind must be a string");

  switch (kind) {
    case "init": {
      const theme = decodeTheme(value["theme"]);
      if (!theme.ok) return theme;
      const panel = value["panel"];
      if (panel !== "diff" && panel !== "workspaces") {
        return fail(`init.panel must be "diff" or "workspaces", got ${String(panel)}`);
      }
      return succeed({ kind: "init", theme: theme.value, panel });
    }
    case "workspaces": {
      const repo = nullableStringField(value, "repo", "workspaces");
      if (!repo.ok) return repo;
      const error = nullableStringField(value, "error", "workspaces");
      if (!error.ok) return error;
      const raw = value["rows"];
      if (!Array.isArray(raw)) return fail("workspaces.rows must be an array");
      const rows: WorkspaceRow[] = [];
      for (let index = 0; index < raw.length; index += 1) {
        const row = decodeWorkspaceRow(raw[index], index);
        if (!row.ok) return row;
        rows.push(row.value);
      }
      return succeed({ kind: "workspaces", repo: repo.value, rows, error: error.value });
    }
    case "files": {
      const repo = nullableStringField(value, "repo", "files");
      if (!repo.ok) return repo;
      const error = nullableStringField(value, "error", "files");
      if (!error.ok) return error;
      const raw = value["files"];
      if (!Array.isArray(raw)) return fail("files.files must be an array");
      const files: ChangedFile[] = [];
      for (let index = 0; index < raw.length; index += 1) {
        const file = decodeChangedFile(raw[index], index);
        if (!file.ok) return file;
        files.push(file.value);
      }
      return succeed({ kind: "files", repo: repo.value, files, error: error.value });
    }
    case "fileDiff": {
      const path = stringField(value, "path", "fileDiff");
      if (!path.ok) return path;
      const patch = stringField(value, "patch", "fileDiff");
      if (!patch.ok) return patch;
      const error = nullableStringField(value, "error", "fileDiff");
      if (!error.ok) return error;
      return succeed({
        kind: "fileDiff",
        path: path.value,
        patch: patch.value,
        error: error.value,
      });
    }
    case "ping": {
      const nonce = stringField(value, "nonce", "ping");
      return nonce.ok ? succeed({ kind: "ping", nonce: nonce.value }) : nonce;
    }
    default:
      return fail(`unknown message kind "${kind}"`);
  }
}
