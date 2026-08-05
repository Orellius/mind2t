// The typed boundary between the Rust host and this chrome.
//
// Nothing crosses it as `any` and nothing is trusted on arrival: a Tauri event payload is JSON
// that arrived from another process, so it is PARSED into a known shape rather than cast into
// one. A cast tells the compiler to stop checking exactly where the data stopped being ours.
//
// This file is also the whole vocabulary. If a message is not here, the chrome does not know
// how to talk about it - which is the point, because the one law this layer must never break is
// that terminal bytes do not enter the webview. There is deliberately no message carrying grid
// content, pixels, or keystrokes.

/// What the host tells the chrome about the session it is drawing under us.
export type SessionState = {
  readonly cols: number;
  readonly rows: number;
  /// The child's working directory, as the shell reported it (OSC 7). Empty until it does.
  readonly cwd: string;
  readonly exited: boolean;
};

export const SESSION_EVENT = "sadna://session" as const;

/// Sent once, by the chrome, when its script has actually run.
///
/// The host has no other way to distinguish a page that failed to load from a page that loaded
/// and said nothing: both are an empty strip. This is the difference.
export const CHROME_READY = "sadna://chrome-ready" as const;

/// Narrows an unknown payload to `SessionState`, or returns null.
///
/// Returning null rather than throwing is deliberate: a malformed payload is a host bug, and the
/// chrome's correct response is to keep rendering the last good state rather than to blank the
/// window with an error boundary. The bug is reported to the console, not to the operator.
export function parseSessionState(value: unknown): SessionState | null {
  if (typeof value !== "object" || value === null) return null;
  const candidate = value as Record<string, unknown>;
  const { cols, rows, cwd, exited } = candidate;
  if (typeof cols !== "number" || !Number.isFinite(cols)) return null;
  if (typeof rows !== "number" || !Number.isFinite(rows)) return null;
  if (typeof cwd !== "string") return null;
  if (typeof exited !== "boolean") return null;
  return { cols, rows, cwd, exited };
}
