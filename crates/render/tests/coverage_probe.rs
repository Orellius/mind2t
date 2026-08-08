//! Ignored survey tool: which installed fonts cover the Symbols for Legacy
//! Computing block (U+1FBxx) and quadrants that Clawd and TUI mosaics need.
//! Run: cargo test -p mind2t-vt-render --test coverage_probe -- --ignored --nocapture
use mind2t_vt_render::FontStack;

#[test]
#[ignore]
fn survey_legacy_computing_coverage() {
    let dirs = [
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
        &format!("{}/Library/Fonts", std::env::var("HOME").unwrap_or_default()),
    ];
    let probes = ['\u{1FB00}', '\u{1FB40}', '\u{1FBC0}', '\u{2596}', '\u{2580}'];
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else { continue };
            if !matches!(ext, "ttf" | "ttc" | "otf") {
                continue;
            }
            let spec = [(path.to_str().unwrap(), 0usize)];
            let Ok(mut stack) = FontStack::load(&spec, 16.0) else { continue };
            let hits: String = probes
                .iter()
                .map(|&c| if stack.resolve(c).is_some() { 'X' } else { '.' })
                .collect();
            if hits.contains('X') {
                println!("{hits}  {}", path.display());
            }
        }
    }
}
