//! The ruuah-vt terminal core: bytes in, grid mutations out.
//!
//! Pure and deterministic by construction -- no PTY, no GPU, no clock, no I/O. That split
//! is what makes headless CI and differential testing against libghostty-vt possible, and
//! Ghostty enforces the same one physically between `src/terminal/` and `src/renderer/`.
//!
//! Slice 1 covers echo, SGR, and cursor movement. Autowrap, scrolling, scroll regions, the
//! alternate screen, tabs and erase operations are slice 2 and are deliberately absent; the
//! differential corpus records each gap as a case that is expected to differ.
//!
//! Bidi is **not** a core concern and never becomes one -- it is a slice 5 renderer item.
//! See `CLAUDE.md`: the C ABI has no bidi surface, so reordering here would break drop-in
//! compatibility and make every RTL line diverge from the oracle by construction.

pub mod cell;
pub mod grid;
pub mod sgr;
pub mod style;
pub mod terminal;

pub use cell::{Cell, CellFlags, Wide};
pub use grid::{Grid, RowMeta};
pub use style::{DEFAULT_STYLE_ID, StyleId, StyleTable};
pub use terminal::Terminal;
