// Import order matters: the bridge installs window.__ruuahReceive at module scope, and
// the host may probe liveness the moment the document finishes loading -- before React
// has mounted anything.
import "./bridge";
import "./styles.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Root } from "./Root";

const root = document.getElementById("root");
if (root === null) {
  // Nothing to render into means the document is not the one we built. Say so where a
  // human will see it rather than failing to paint for no stated reason.
  document.body.textContent = "RUUAH panel: #root missing";
} else {
  createRoot(root).render(
    <StrictMode>
      <Root />
    </StrictMode>,
  );
}
