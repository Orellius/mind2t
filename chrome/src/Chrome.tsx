import { useEffect, useState } from "react";
import { CHROME_READY, SESSION_EVENT, parseSessionState, type SessionState } from "./protocol";

/// The chrome strip. It describes the session; it never touches it.
///
/// Everything shown here is a FACT THE HOST SENT, not something this component computed. The grid
/// size in particular: the number of columns is decided in Rust from the window and the cell
/// metrics, and a webview that recalculated it from its own width would drift by a pixel of
/// rounding and then quietly disagree with the terminal it is describing.
export function Chrome(): React.JSX.Element {
  const [session, setSession] = useState<SessionState | null>(null);

  useEffect(() => {
    // A POSITIVE signal that this document ran. The host cannot otherwise tell a page that
    // failed to load from a page that loaded and had nothing to say - both are a blank strip,
    // and one of them is a bug (SCAR-004: a silence proves nothing).
    // The title is a channel that cannot fail for IPC reasons: WebKit exposes it to the host
    // directly, so it separates "the script never ran" from "the script ran and the IPC did not
    // carry its message". Two suspects, one line.
    document.title = `mind2t-chrome-ran ${window.location.href}`;
    void import("@tauri-apps/api/event")
      .then((api) => api.emit(CHROME_READY, { href: window.location.href }))
      .catch(() => {});
  }, []);

  useEffect(() => {
    let cancelled = false;
    // Subscribed on mount and torn down on unmount. An event subscription that outlives its
    // component is the leak that survives every reload and is invisible until the handlers pile
    // up enough to matter.
    const unlisten = listen((payload) => {
      const next = parseSessionState(payload);
      if (next === null) {
        console.error("mind2t: unparseable session payload", payload);
        return;
      }
      if (!cancelled) setSession(next);
    });
    return () => {
      cancelled = true;
      void unlisten.then((stop) => stop());
    };
  }, []);

  return (
    <div className="bar">
      {/* A folder, not a status light and a wordmark. The strip already prints the path
          immediately to the right, so a dot that meant "alive" and a brand that never changed
          were both spending width to say nothing the operator could not already see. The
          exited signal is not lost: it colours this icon, which is the thing the path belongs
          to, so one glyph carries both facts. */}
      <svg
        className="folder"
        data-exited={String(session?.exited ?? false)}
        viewBox="0 0 16 16"
        width="14"
        height="14"
        aria-hidden="true"
        focusable="false"
      >
        <path
          fill="currentColor"
          d="M1.5 3.25c0-.69.56-1.25 1.25-1.25h3.09c.4 0 .78.19 1.02.51l.79 1.05c.05.06.12.1.2.1h5.4c.69 0 1.25.56 1.25 1.25v7.34c0 .69-.56 1.25-1.25 1.25H2.75c-.69 0-1.25-.56-1.25-1.25V3.25Z"
        />
      </svg>
      <span className="cwd">{session?.cwd || "-"}</span>
      <span className="grid">{session ? `${session.cols}x${session.rows}` : "-"}</span>
    </div>
  );
}

/// Subscribes to the host's session events.
///
/// Imported dynamically, and the failure path returns a no-op unsubscribe rather than throwing:
/// opened in a plain browser by `vite dev` there is no Tauri global, and the chrome should still
/// render with no session so its look can be worked on without launching the app. A static
/// import would make that a blank page and a console full of module errors.
function listen(handler: (payload: unknown) => void): Promise<() => void> {
  return import("@tauri-apps/api/event")
    .then((api) => api.listen(SESSION_EVENT, (event) => handler(event.payload)))
    .catch((error: unknown) => {
      console.warn("mind2t: no host events in this context", error);
      return () => {};
    });
}
