import { useEffect, useState } from "react";
import { send, subscribe } from "./bridge";
import { DiffPanel } from "./DiffPanel";
import { WorkspacePanel } from "./WorkspacePanel";
import type { PanelKind, Theme } from "./protocol";

function applyTheme(theme: Theme): void {
  const root = document.documentElement.style;
  root.setProperty("--bg", theme.background);
  root.setProperty("--fg", theme.foreground);
  root.setProperty("--accent", theme.accent);
  root.setProperty("--dim", theme.dim);
}

/**
 * Decides which panel this document is, and owns the handshake.
 *
 * The root sends `ready`, not the panel. The host answers with `init` (which names the
 * panel) plus that panel's first data, and the panel only mounts once the kind is known
 * -- so the data provably arrives before its consumer exists. The bridge's
 * latest-per-kind replay is what makes that safe rather than a race.
 */
export function Root() {
  const [panel, setPanel] = useState<PanelKind | null>(null);

  useEffect(() => {
    const unsubscribe = subscribe((message) => {
      if (message.kind !== "init") return;
      applyTheme(message.theme);
      setPanel(message.panel);
    });
    send({ kind: "ready" });
    return unsubscribe;
  }, []);

  if (panel === null) return null;
  return panel === "workspaces" ? <WorkspacePanel /> : <DiffPanel />;
}
