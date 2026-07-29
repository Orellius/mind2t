//! Purpose: sixel (DCS q) decoded into the SAME image store and placement path kitty
//! graphics built -- one rendering pipeline, two wire protocols.
//! Public surface (crate): `SixelDecoder`, wired from the vte hook/put/unhook path.
//! Reference: there is NO oracle for this one -- Ghostty implements kitty graphics and
//!   not sixel (its only sixel mention is the DA1 capability table), so the gate is the
//!   protocol description (VT330/340 manual, libsixel's emitter behavior) and these
//!   unit tests. Weaker than a differential gate, said out loud.
//! V1 boundaries: color space 2 (RGB 0..100) only -- HLS registers decode as black
//!   with the boundary documented; P2=1 and 0-bits both render transparent; the cursor
//!   does not move after a placement (the core cannot know pixel cell size; modes
//!   80/8452 are future work); dimensions cap at 4096x4096 and data past it is dropped.

/// Sixel images get ids in a private range so they never collide with kitty's
/// client-chosen ids; the counter wraps within the range.
pub(crate) const SIXEL_ID_BASE: u32 = 0xFFF0_0000;

const MAX_DIM: u32 = 4096;

#[derive(Debug)]
pub(crate) struct SixelDecoder {
    colors: [[u8; 4]; 256],
    current: usize,
    /// Pixel cursor within the current six-row band.
    x: u32,
    band: u32,
    repeat: u32,
    /// Parameter accumulation for `#` and `!` and `"` sequences.
    pending: Pending,
    params: Vec<u32>,
    width: u32,
    height: u32,
    /// Straight RGBA, grown as bands complete.
    rgba: Vec<u8>,
    dropped: bool,
}

#[derive(Debug, PartialEq)]
enum Pending {
    None,
    Color,
    Repeat,
    Raster,
}

impl SixelDecoder {
    pub(crate) fn new() -> SixelDecoder {
        SixelDecoder {
            colors: [[0, 0, 0, 255]; 256],
            current: 0,
            x: 0,
            band: 0,
            repeat: 1,
            pending: Pending::None,
            params: Vec::new(),
            width: 0,
            height: 0,
            rgba: Vec::new(),
            dropped: false,
        }
    }

    pub(crate) fn put(&mut self, byte: u8) {
        if self.dropped {
            return;
        }
        match byte {
            b'0'..=b'9' if self.pending != Pending::None => {
                let slot = self.params.last_mut().expect("param slot exists");
                *slot = slot.saturating_mul(10) + u32::from(byte - b'0');
            }
            b';' if self.pending != Pending::None => self.params.push(0),
            b'#' => self.begin(Pending::Color),
            b'!' => self.begin(Pending::Repeat),
            b'"' => self.begin(Pending::Raster),
            b'$' => {
                self.flush_pending();
                self.x = 0;
            }
            b'-' => {
                self.flush_pending();
                self.x = 0;
                self.band += 1;
            }
            b'?'..=b'~' => {
                self.flush_pending();
                let bits = byte - b'?';
                let repeat = self.repeat.max(1);
                self.repeat = 1;
                for _ in 0..repeat {
                    self.plot(bits);
                    self.x += 1;
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, pending: Pending) {
        self.flush_pending();
        self.pending = pending;
        self.params.clear();
        self.params.push(0);
    }

    /// Applies whichever `#`/`!`/`"` sequence the digits belonged to.
    fn flush_pending(&mut self) {
        match std::mem::replace(&mut self.pending, Pending::None) {
            Pending::None => {}
            Pending::Color => {
                let register = *self.params.first().unwrap_or(&0) as usize % 256;
                if self.params.len() >= 5 && self.params[1] == 2 {
                    // RGB, 0..100 scale, rounded the way libsixel rounds.
                    let scale =
                        |value: u32| -> u8 { ((value.min(100) * 255 + 50) / 100) as u8 };
                    self.colors[register] = [
                        scale(self.params[2]),
                        scale(self.params[3]),
                        scale(self.params[4]),
                        255,
                    ];
                }
                // With params it DEFINES; with or without, it SELECTS.
                self.current = register;
            }
            Pending::Repeat => {
                self.repeat = (*self.params.first().unwrap_or(&1)).clamp(1, MAX_DIM);
            }
            Pending::Raster => {
                // "Pan;Pad;Ph;Pv -- only the size hint matters here; it pre-sizes the
                // bitmap so a well-formed image never reallocs per band.
                if self.params.len() >= 4 {
                    let (width, height) = (self.params[2], self.params[3]);
                    if width > MAX_DIM || height > MAX_DIM {
                        // A hint past the cap is a hostile or broken emitter; dropping
                        // the whole image beats plotting an unbounded one piecemeal.
                        self.dropped = true;
                    } else {
                        self.ensure(width.max(1), height.max(1));
                    }
                }
            }
        }
    }

    fn plot(&mut self, bits: u8) {
        if bits == 0 {
            // Transparent column; still occupies width.
            self.ensure(self.x + 1, self.band * 6 + 6);
            return;
        }
        let base_y = self.band * 6;
        self.ensure(self.x + 1, base_y + 6);
        if self.dropped {
            return;
        }
        let color = self.colors[self.current];
        for bit in 0..6u32 {
            if bits & (1 << bit) != 0 {
                let y = base_y + bit;
                let offset = ((y * self.width + self.x) * 4) as usize;
                self.rgba[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }

    /// Grows the bitmap to at least the given size, preserving content.
    fn ensure(&mut self, width: u32, height: u32) {
        if width <= self.width && height <= self.height {
            return;
        }
        if width > MAX_DIM || height > MAX_DIM {
            self.dropped = true;
            return;
        }
        let new_width = width.max(self.width);
        let new_height = height.max(self.height);
        let mut grown = vec![0u8; (new_width * new_height * 4) as usize];
        for y in 0..self.height {
            let src = ((y * self.width) * 4) as usize;
            let dst = ((y * new_width) * 4) as usize;
            let row = (self.width * 4) as usize;
            grown[dst..dst + row].copy_from_slice(&self.rgba[src..src + row]);
        }
        self.rgba = grown;
        self.width = new_width;
        self.height = new_height;
    }

    /// The finished image, or None when nothing plottable arrived.
    pub(crate) fn finish(mut self) -> Option<crate::graphics::Image> {
        self.flush_pending();
        if self.dropped || self.width == 0 || self.height == 0 {
            return None;
        }
        Some(crate::graphics::Image {
            width: self.width,
            height: self.height,
            rgba: self.rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::graphics::ImageOp;
    use crate::terminal::Terminal;

    fn decoded(body: &str) -> Option<crate::graphics::Image> {
        let mut terminal = Terminal::new(40, 10);
        terminal.write(format!("\x1bP0;0;0q{body}\x1b\\").as_bytes());
        match terminal.take_image_ops().pop() {
            Some(ImageOp::Add(id, image)) => {
                assert!(id >= super::SIXEL_ID_BASE, "sixel ids live in the private range");
                Some(image)
            }
            _ => None,
        }
    }

    #[test]
    fn a_full_column_in_a_defined_color_decodes() {
        let image = decoded("#1;2;100;0;0#1~~").expect("an image");
        assert_eq!((image.width, image.height), (2, 6));
        assert!(
            image.rgba.chunks_exact(4).all(|px| px == [255, 0, 0, 255]),
            "every pixel is the defined red"
        );
    }

    #[test]
    fn repeat_and_bands_shape_the_bitmap() {
        let image = decoded("#1;2;0;100;0#1!4~-!4~").expect("an image");
        assert_eq!((image.width, image.height), (4, 12), "two bands of four columns");
    }

    #[test]
    fn zero_bits_are_transparent_and_still_take_width() {
        let image = decoded("#1;2;0;0;100#1?~").expect("an image");
        assert_eq!(image.width, 2);
        assert_eq!(&image.rgba[..4], &[0, 0, 0, 0], "the ? column is transparent");
        assert_eq!(&image.rgba[4..8], &[0, 0, 255, 255], "the ~ column is blue");
    }

    #[test]
    fn a_sixel_places_at_the_cursor() {
        let mut terminal = Terminal::new(40, 10);
        terminal.write(b"\x1b[3;5H\x1bP0;0;0q#1;2;100;0;0#1~\x1b\\");
        let placements = terminal.screen().placements.clone();
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].col, placements[0].row), (4, 2));
        assert_eq!((placements[0].cols, placements[0].rows), (0, 0), "native size");
    }

    #[test]
    fn garbage_and_oversize_produce_nothing() {
        assert!(decoded("").is_none());
        let mut terminal = Terminal::new(40, 10);
        terminal.write(b"\x1bP0;0;0q\"1;1;9999;9999#1~\x1b\\");
        assert_eq!(terminal.take_image_ops(), vec![], "past the cap, dropped");
    }
}

#[cfg(test)]
mod resize_tests {
    use crate::terminal::Terminal;

    /// The "evades its cell" defect, seen live 2026-07-30: reflow moves the text, a
    /// grid-anchored placement stays put, and the image detaches from the line it
    /// illustrated. v1 rule: a resize clears placements (predictable vanish, never a
    /// lying position); the store keeps the pixels so a re-place by id needs no
    /// retransmission. Applies to kitty and sixel alike -- one placement path.
    #[test]
    fn a_resize_clears_placements_but_keeps_the_store() {
        let mut terminal = Terminal::new(40, 10);
        terminal.write(b"\x1bP0;0;0q#1;2;100;0;0#1~\x1b\\");
        terminal.write(b"\x1b_Ga=T,f=32,s=1,v=1,i=5,c=1,r=1,q=2;/wAA/w==\x1b\\");
        assert_eq!(terminal.screen().placements.len(), 2, "both protocols placed");
        let stored = terminal.take_image_ops().len();
        assert_eq!(stored, 2, "both images in the store");

        terminal.resize(30, 8);
        assert!(
            terminal.screen().placements.is_empty(),
            "a resize clears placements -- the v1 rule this test pins"
        );
        assert_eq!(
            terminal.take_image_ops(),
            vec![],
            "no Remove ops: the store keeps the pixels for a re-place by id"
        );
    }
}
