import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

// One self-contained index.html: every asset inlined, no sibling files to resolve.
//
// This is not a size preference, it is what makes the WKWebView side safe to reason
// about. A multi-file bundle loaded from a file:// URL resolves its script and style
// siblings against the directory we granted read access to, so the panel's integrity
// depends on that grant staying tight forever. A single document has nothing to
// resolve: the webview is handed one file, loads zero subresources, and a
// navigation policy that refuses everything (WebPanel.swift) has nothing legitimate
// to allow through. It also means build-app.sh copies exactly one artifact.
export default defineConfig({
  plugins: [react(), viteSingleFile()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // Inlining is the whole point; the limit has to clear the bundle.
    assetsInlineLimit: 100_000_000,
    cssCodeSplit: false,
    // No sourcemap: it would be inlined too, tripling the document for no gain
    // inside a webview with no attached debugger in normal use.
    sourcemap: false,
    target: "safari17",
  },
});
