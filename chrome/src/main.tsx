import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Chrome } from "./Chrome";
import "./styles.css";

const host = document.getElementById("root");
if (host === null) {
  // Loud rather than silent. A missing root in a webview shows an empty transparent strip, which
  // is indistinguishable from a chrome that rendered correctly and had nothing to say.
  throw new Error("mind2t: #root is missing from the document");
}

createRoot(host).render(
  <StrictMode>
    <Chrome />
  </StrictMode>,
);
