//! Purpose: read what libghostty-vt says a renderer would have to repaint.
//! Public surface: `RenderState`.
//! Why this file: damage is the slice-5 blind spot. The ABI keeps it in a separate object
//!   rather than on the terminal, because a renderer updates it on its own schedule -- so
//!   observing it needs its own handle, its own lifecycle, and its own unsafe surface, all
//!   confined here like the rest of the C ABI.
//! NOT responsible for: deciding what SHOULD be dirty (that is the terminal's behaviour, and
//!   comparing it is the corpus's job).
//! Test strategy: `tests/oracle.rs` pins that a write dirties the row it wrote to and that a
//!   reset clears both layers, so the harness is known to detect damage before any is
//!   implemented in the core.

use std::ffi::c_void;
use std::mem;

use ruuah_vt_snapshot::{Damage, Dirty};

use crate::sys;
use crate::terminal::{Error, Terminal, check};

/// A libghostty-vt render state: the accumulated damage since it was last reset.
pub struct RenderState {
    raw: sys::GhosttyRenderState,
}

impl RenderState {
    pub fn new() -> Result<RenderState, Error> {
        let mut raw: sys::GhosttyRenderState = std::ptr::null_mut();
        check("ghostty_render_state_new", unsafe {
            sys::ghostty_render_state_new(std::ptr::null(), &mut raw)
        })?;
        if raw.is_null() {
            return Err(Error::NullTerminal);
        }
        Ok(RenderState { raw })
    }

    /// Pulls the terminal's current state in, accumulating dirt rather than replacing it.
    pub fn update(&mut self, terminal: &Terminal) -> Result<(), Error> {
        check("ghostty_render_state_update", unsafe {
            sys::ghostty_render_state_update(self.raw, terminal.raw())
        })
    }

    /// Clears both dirty layers.
    ///
    /// Both, deliberately: the header is explicit that clearing the global state does not
    /// clear the per-row flags, and a half-reset would make the next frame's damage look
    /// larger than it is.
    pub fn clear_dirty(&mut self) -> Result<(), Error> {
        let global = sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        check("ghostty_render_state_set(DIRTY)", unsafe {
            sys::ghostty_render_state_set(
                self.raw,
                sys::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&raw const global).cast::<c_void>().cast_mut(),
            )
        })?;

        self.for_each_row(|iterator| {
            let clear = false;
            check("ghostty_render_state_row_set(DIRTY)", unsafe {
                sys::ghostty_render_state_row_set(
                    iterator,
                    sys::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                    (&raw const clear).cast::<c_void>().cast_mut(),
                )
            })?;
            Ok(None::<()>)
        })
        .map(|_| ())
    }

    /// Reads both dirty layers out into the comparison type.
    pub fn damage(&self) -> Result<Damage, Error> {
        let mut raw_global: sys::GhosttyRenderStateDirty = 0;
        check("ghostty_render_state_get(DIRTY)", unsafe {
            sys::ghostty_render_state_get(
                self.raw,
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY,
                (&raw mut raw_global).cast::<c_void>(),
            )
        })?;

        let global = match raw_global {
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE => Dirty::None,
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => Dirty::Partial,
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => Dirty::Full,
            other => {
                return Err(Error::UnknownEnum {
                    kind: "GhosttyRenderStateDirty",
                    value: other,
                });
            }
        };

        let mut rows = Vec::new();
        self.for_each_row(|iterator| {
            let mut dirty = false;
            check("ghostty_render_state_row_get(DIRTY)", unsafe {
                sys::ghostty_render_state_row_get(
                    iterator,
                    sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                    (&raw mut dirty).cast::<c_void>(),
                )
            })?;
            Ok(Some(dirty))
        })
        .map(|collected| rows.extend(collected))?;

        Ok(Damage { global, rows })
    }

    /// Walks every row of the render state, running `visit` on each.
    ///
    /// The iterator is allocated separately and then bound to the state by asking for it, per
    /// the header: `ROW_ITERATOR` populates a pre-allocated handle rather than returning one.
    fn for_each_row<T>(
        &self,
        mut visit: impl FnMut(sys::GhosttyRenderStateRowIterator) -> Result<Option<T>, Error>,
    ) -> Result<Vec<T>, Error> {
        let mut iterator: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
        check("ghostty_render_state_row_iterator_new", unsafe {
            sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut iterator)
        })?;

        let result = (|| {
            check("ghostty_render_state_get(ROW_ITERATOR)", unsafe {
                sys::ghostty_render_state_get(
                    self.raw,
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                    (&raw mut iterator).cast::<c_void>().cast(),
                )
            })?;

            let mut collected = Vec::new();
            while unsafe { sys::ghostty_render_state_row_iterator_next(iterator) } {
                if let Some(value) = visit(iterator)? {
                    collected.push(value);
                }
            }
            Ok(collected)
        })();

        unsafe { sys::ghostty_render_state_row_iterator_free(iterator) };
        result
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        unsafe { sys::ghostty_render_state_free(self.raw) };
    }
}

const _: () = {
    // The iterator handle is passed by pointer to be populated, so its size has to match what
    // the header expects to write through that pointer.
    assert!(mem::size_of::<sys::GhosttyRenderStateRowIterator>() == mem::size_of::<*mut c_void>());
};
