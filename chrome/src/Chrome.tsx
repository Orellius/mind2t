import { useEffect, useState } from "react";
import { SESSION_EVENT, parseSessionState, type SessionState } from "./protocol";

/// The chrome strip. It describes the session; it never touches it.
///
/// Everything shown here is a FACT THE HOST SENT, not something this component computed. The grid
/// size in particular: the number of columns is decided in Rust from the window and the cell
/// metrics, and a webview that recalculated it from its own width would drift by a pixel of
/// rounding and then quietly disagree with the terminal it is describing.
export function Chrome(): React.JSX.Element {
  const [session, setSession] = useState<SessionState | null>(null);

  useEffect(() => {
    let cancelled = false;
    // Subscribed on mount and torn down on unmount. An event subscription that outlives its
    // component is the leak that survives every reload and is invisible until the handlers pile
    // up enough to matter.
    const unlisten = listen((payload) => {
      const next = parseSessionState(payload);
      if (next === null) {
        console.error("bindary: unparseable session payload", payload);
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
      <span className="dot" data-exited={String(session?.exited ?? false)} />
      <span className="brand">BINDARY</span>
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
      console.warn("bindary: no host events in this context", error);
      return () => {};
    });
}
