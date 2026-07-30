//! The ruuah-vt terminal core: bytes in, grid mutations out.
//!
//! Pure and deterministic by construction -- no PTY, no GPU, no clock, no I/O. That split
//! is what makes headless CI and differential testing against libghostty-vt possible, and
//! Ghostty enforces the same one physically between `src/terminal/` and `src/renderer/`.
//!
//! Slices 1 through 4 cover echo, SGR, cursor movement, the autowrap phantom state,
//! scrolling and scroll regions, the alternate screen, tab stops, the erase / insert /
//! delete operations, paged scrollback, and reflow on resize. The differential corpus
//! records each remaining gap as a case expected to differ.
//!
//! Bidi is **not** a core concern and never becomes one -- it is a slice 5 renderer item.
//! See `CLAUDE.md`: the C ABI has no bidi surface, so reordering here would break drop-in
//! compatibility and make every RTL line diverge from the oracle by construction.

pub mod cell;
pub mod dispatch;
pub mod events;
pub mod graphics;
pub mod grid;
pub mod history;
mod hyperlink;
pub mod kitty_keys;
pub mod mouse;
pub mod page;
pub mod reflow;
mod replies;
pub mod resize;
pub mod screen;
mod semantic;
pub mod sgr;
mod sixel;
mod split;
pub mod style;
pub mod tabs;
pub mod terminal;

pub use cell::{Cell, CellFlags, Wide};
pub use grid::{Grid, RowMeta};
pub use history::History;
pub use page::Page;
pub use reflow::{Anchor, Mode, Reflowed, resize_rows};
pub use resize::apply as resize;
pub use screen::Screen;
pub use style::{DEFAULT_STYLE_ID, StyleId, StyleTable};
pub use tabs::TabStops;
pub use terminal::Terminal;
