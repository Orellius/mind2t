// Tauri's build step: it reads `tauri.conf.json`, generates the context `generate_context!`
// expands to, and embeds the frontend from `frontendDist`.
//
// THERE IS DELIBERATELY NO `devUrl` IN THAT CONFIG, and it cost four probes to learn why. With a
// `devUrl` present, a debug build points every webview at the dev server whatever else the
// config says - so `cargo run` without a vite server running loads an empty 39-byte document,
// and the chrome looks exactly like a webview that lost a z-order fight. One source of truth
// (the built `chrome/dist`) is worth more here than hot reload: the chrome is small and
// `bun run --cwd chrome build` takes about 50ms.
fn main() {
    tauri_build::build();
}
