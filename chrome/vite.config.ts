import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// NOT single-file, and that is the difference from `web/`.
//
// The panels bundle inlines everything because a WKWebView loads it from a file:// URL, where
// every subresource it resolves is a hole in a read grant. This chrome is served by Tauri's own
// asset protocol instead - the webview never touches the filesystem, so the reason for inlining
// does not apply and an ordinary multi-file build keeps the dev server usable.
export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    // The webview is WKWebView on macOS; there is no older engine to support here.
    target: "safari17",
  },
});
