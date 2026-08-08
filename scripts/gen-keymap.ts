// Generates the macOS virtual-keycode tables from the oracle's keycode table
// (src/input/keycodes.zig, Chromium dom_code_data) and the C key enum order
// (vendor/.../vt/key/event.h). Run: bun gen-keymap.ts <ruuah-src> <mind2t-vt-root>
import { readFileSync, writeFileSync } from "fs";

const oracleSrc = process.argv[2];
const repo = process.argv[3];
if (!oracleSrc || !repo) throw new Error("usage: gen-keymap.ts <oracle-src> <repo>");

// 1. C enum order -> value.
const eventH = readFileSync(`${repo}/vendor/libghostty-vt/include/ghostty/vt/key/event.h`, "utf8");
const enumBody = eventH.match(/GHOSTTY_KEY_UNIDENTIFIED = 0,([\s\S]*?)GHOSTTY_KEY_MAX_VALUE/);
if (!enumBody) throw new Error("enum body not found");
const cValues = new Map<string, number>();
cValues.set("GHOSTTY_KEY_UNIDENTIFIED", 0);
let next = 1;
for (const m of enumBody[1].matchAll(/GHOSTTY_KEY_([A-Z0-9_]+),/g)) {
  cValues.set(`GHOSTTY_KEY_${m[1]}`, next++);
}

// 2. DomCode -> zig key name.
const keycodes = readFileSync(`${oracleSrc}/src/input/keycodes.zig`, "utf8");
const codeToKey = new Map<string, string>();
for (const m of keycodes.matchAll(/\.\{ "([A-Za-z0-9]+)", \.([a-z0-9_]+) \}/g)) {
  codeToKey.set(m[1], m[2]);
}

// 3. raw_entries rows: { usb, evdev, xkb, win, mac, "DomCode" }.
const rows = [...keycodes.matchAll(
  /\.\{\s*0x([0-9a-fA-F]+),\s*0x([0-9a-fA-F]+),\s*0x([0-9a-fA-F]+),\s*0x([0-9a-fA-F]+),\s*0x([0-9a-fA-F]+),\s*"([A-Za-z0-9]*)"\s*\}/g,
)];
if (rows.length < 100) throw new Error(`only ${rows.length} raw entries parsed`);

const zigToC = (zig: string): string => {
  let name = zig.toUpperCase();
  if (name.startsWith("KEY_")) name = name.slice(4); // key_a -> A
  return `GHOSTTY_KEY_${name}`;
};

// One column of the Chromium table -> `(native keycode, GhosttyKey value, DomCode)`.
//
// Extracted from the macOS-only loop on 2026-08-08 (T2) rather than copied beside it. Orel's
// call was a native key source PER PLATFORM, and the named risk of that route is two tables
// that must never disagree. They cannot disagree if there is one generator, one source table
// and one row loop, which is what this function makes true - a platform is a column index and
// a sentinel, nothing more.
//
// `absent` is the value the column uses for "this key does not exist here", and it differs by
// column: the mac column parks unmapped keys at 0xffff, the xkb column at 0x0000. Both are
// refused, because a key mapped to a sentinel is a key that silently does the wrong thing.
const tableFor = (
  column: number,
  absent: number[],
): Array<[number, number, string]> => {
  const pairs: Array<[number, number, string]> = [];
  const seen = new Set<number>();
  const unmappedKeys: string[] = [];
  for (const row of rows) {
    const native = parseInt(row[column], 16);
    const dom = row[6];
    if (absent.includes(native) || dom === "") continue;
    const zig = codeToKey.get(dom);
    if (!zig) continue; // dom codes the oracle maps to .unidentified
    const cName = zigToC(zig);
    const value = cValues.get(cName);
    if (value === undefined) {
      unmappedKeys.push(`${dom} -> ${zig} -> ${cName}`);
      continue;
    }
    if (seen.has(native)) continue; // first entry wins, matching the table order
    seen.add(native);
    pairs.push([native, value, dom]);
  }
  if (unmappedKeys.length) {
    console.error("zig->C name misses (fix zigToC):\n" + unmappedKeys.join("\n"));
    process.exit(1);
  }
  pairs.sort((a, b) => a[0] - b[0]);
  return pairs;
};

// Column 5 is `mac`, column 3 is `xkb`. Row shape: { usb, evdev, xkb, win, mac, "DomCode" }.
const pairs = tableFor(5, [0xffff]);
const linuxPairs = tableFor(3, [0x0000, 0xffff]);

// The xkb column is the X11 keycode, which is the evdev scancode plus 8. Asserted here rather
// than believed: 180 of the 180 named rows with a non-zero xkb code satisfied it when this was
// written, and a Chromium table that ever stopped satisfying it would mean the column had been
// re-based under us and every Linux key would be off by a constant.
for (const row of rows) {
  const evdev = parseInt(row[2], 16);
  const xkb = parseInt(row[3], 16);
  if (row[6] === "" || xkb === 0 || xkb === 0xffff) continue;
  if (xkb !== evdev + 8) {
    console.error(`xkb column is not evdev+8 at ${row[6]}: xkb=${xkb} evdev=${evdev}`);
    process.exit(1);
  }
}

const lines = pairs.map(([mac, value, dom]) => `        ${mac}: ${value},  // ${dom}`);
const swift = `// macOS virtual keyCode -> the C key enum (GhosttyKey values, see
// vendor/libghostty-vt/include/ghostty/vt/key/event.h). GENERATED from the oracle's
// keycode table (src/input/keycodes.zig, itself Chromium's dom_code_data mac column)
// by scripts/gen-keymap.ts -- regenerate, never hand-edit. ${pairs.length} entries.
enum KeyMap {
    static let keyByCode: [UInt16: UInt32] = [
${lines.join("\n")}
    ]
}
`;
writeFileSync(`${repo}/swift/Sources/mind2t-host/KeyMap.swift`, swift);

// The same table for Rust hosts. TWO consumers, ONE generator: Mind2t needs this mapping
// because Tauri exposes no keyboard events at the window level, so the app reads NSEvent
// itself - and hand-porting 110 entries from the Swift file is exactly how the two drift.
const rustRows = pairs.map(([mac, value, dom]) => `    (${mac}, ${value}), // ${dom}`);
const linuxRows = linuxPairs.map(([code, value, dom]) => `    (${code}, ${value}), // ${dom}`);
const rust = `//! Native hardware keycode -> \`Key\`, one table per platform. GENERATED by
//! \`scripts/gen-keymap.ts\` from the oracle's keycode table (\`src/input/keycodes.zig\`, itself
//! Chromium's dom_code_data) and the C key enum order. Regenerate, never hand-edit.
//! ${pairs.length} macOS entries, ${linuxPairs.length} Linux entries.
//!
//! Why this exists at all: a host that reads native key events gets a hardware keycode, which
//! is not the W3C code name the encoder is keyed on. Every host needs that translation, and it
//! comes from ONE generator because two hand-kept copies of a 110-entry table drift silently
//! and present as "one key does nothing".
//!
//! Why it is per platform rather than one table: the hardware code for a physical key is not
//! the same number on macOS and on Linux, and nothing can make it be. What IS shared is the
//! row - both columns come from the same Chromium entry, so the two tables cannot disagree
//! about which physical key a DomCode names. That is the whole reason they are generated
//! together instead of in two passes.
//!
//! **The Linux table's identification is a hypothesis and is labelled one.** The xkb column is
//! the X11 keycode (evdev + 8, asserted by the generator over every row). GDK documents
//! \`GdkEventKey.hardware_keycode\` only as "the raw code of the key that was pressed or
//! released" and says nothing about which convention that is, so the claim that a GTK host can
//! hand this table its \`hardware_keycode\` directly is unproven until a key is pressed on a real
//! Linux machine. It is written down here rather than discovered as "one key does nothing".

use crate::key::Key;

/// \`(macOS virtual keycode, GhosttyKey value)\`, sorted by keycode for binary search.
///
/// The second element indexes \`Key::ALL\` by construction: the C enum's value IS its position
/// in that list, which \`keys!\` guarantees.
pub const MACOS_KEYCODES: &[(u16, u16)] = &[
${rustRows.join("\n")}
];

/// The key a macOS virtual keycode denotes, or \`Unidentified\` for one with no mapping.
pub fn key_from_macos_keycode(code: u16) -> Key {
    lookup(MACOS_KEYCODES, code)
}

/// \`(X11/xkb keycode, GhosttyKey value)\`, sorted by keycode for binary search.
///
/// This is the evdev scancode plus 8, which is what X11 reports and what GTK is believed to
/// pass through as \`hardware_keycode\`. See the module card: the belief is not yet measured.
pub const LINUX_KEYCODES: &[(u16, u16)] = &[
${linuxRows.join("\n")}
];

/// The key an X11/xkb hardware keycode denotes, or \`Unidentified\` for one with no mapping.
pub fn key_from_linux_keycode(code: u16) -> Key {
    lookup(LINUX_KEYCODES, code)
}

/// Shared by both tables, so a platform cannot acquire its own lookup rule by accident.
///
/// \`Unidentified\` rather than a guess: the encoder already treats it as "produces no bytes
/// unless the event carries text", which is the correct behaviour for a key we cannot name.
fn lookup(table: &[(u16, u16)], code: u16) -> Key {
    match table.binary_search_by_key(&code, |(keycode, _)| *keycode) {
        Ok(index) => table
            .get(index)
            .and_then(|(_, value)| Key::ALL.get(usize::from(*value)).copied())
            .unwrap_or(Key::Unidentified),
        Err(_) => Key::Unidentified,
    }
}
`;
writeFileSync(`${repo}/crates/pty/src/keycode.rs`, rust);

console.log(
  `wrote ${pairs.length} macOS entries (Swift and Rust) and ${linuxPairs.length} Linux entries (Rust)`,
);
