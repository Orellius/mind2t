//! Purpose: the kitty graphics protocol, v1 -- APC `G` commands parsed into an image
//! store, placements on the grid, and query replies through the answerback seam.
//! Public surface: `Image`, `ImageOp`, `Placement`, `Terminal::take_image_ops`,
//! `State::apc_graphics` (crate).
//! Reference: the oracle implements the full protocol in
//!   `../ruuah/src/terminal/kitty/graphics_*.zig`; its C ABI exposes none of it, so the
//!   gate is source-reading plus unit tests plus the C-surface pixel test -- the OSC 8
//!   precedent. Ghostty itself ships kitty graphics and no sixel, the same order this
//!   project chose.
//! V1 boundaries (documented, not hidden):
//!   - transmission: DIRECT only (`t=d`, the default). File/shared-memory transmission
//!     answers `ENOTSUPPORTED`, which is what tells icat to fall back.
//!   - formats: f=32 (RGBA), f=24 (RGB), f=100 (PNG via the `png` crate -- icat's
//!     default wire format; GATE 01 weighed, decode is pure and deterministic).
//!   - no unicode placeholders, no z-index, no animation, no source-rect cropping.
//!   - a placement WITHOUT explicit `c,r` spans the image's native pixels; the renderer
//!     computes the cell box (the core is deliberately pixel-cell-ignorant) and the
//!     cursor does not move. With `c,r` given, the cursor steps past the placement the
//!     way kitty's does.
//!   - storage budget: 128 MiB decoded; a transmit past it is refused with `ENOMEM`.

use std::collections::HashMap;

use crate::terminal::State;

/// One decoded image, always straight RGBA8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the pump applies to the shared store the renderer reads. Drained in order via
/// `Terminal::take_image_ops`; the pixels move ONCE (they never ride the seqlock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageOp {
    Add(u32, Image),
    Remove(u32),
    Clear,
}

/// One visible placement: an image anchored to a grid cell. Rows are screen-absolute
/// and scroll with the content; a placement pushed fully off-screen is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub image: u32,
    pub col: u16,
    pub row: u16,
    /// Cell span; 0 means "native pixel size", resolved by the renderer.
    pub cols: u16,
    pub rows: u16,
}

const BUDGET_BYTES: usize = 128 * 1024 * 1024;
/// One APC string's accumulation ceiling; kitty chunks at 4096 payload bytes, so
/// anything near this is a protocol violation, not a big image.
pub(crate) const APC_CEILING: usize = 1024 * 1024;

/// A chunked transmission in flight (`m=1` seen, waiting for the rest).
#[derive(Debug, Default)]
pub(crate) struct PendingTransmit {
    id: u32,
    format: u16,
    width: u32,
    height: u32,
    quiet: u16,
    display: bool,
    cols: u16,
    rows: u16,
    data: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct Graphics {
    pub(crate) images: HashMap<u32, Image>,
    pub(crate) budget_used: usize,
    pub(crate) ops: Vec<ImageOp>,
    pub(crate) pending: Option<PendingTransmit>,
}

/// The parsed key=value half of an APC G command.
#[derive(Debug, Default, Clone, Copy)]
struct Keys {
    action: u8,
    format: u16,
    transmission: u8,
    width: u32,
    height: u32,
    id: u32,
    more: u16,
    quiet: u16,
    delete: u8,
    cols: u16,
    rows: u16,
}

fn parse_keys(control: &[u8]) -> Keys {
    let mut keys = Keys {
        action: b't',
        format: 32,
        transmission: b'd',
        ..Keys::default()
    };
    for pair in control.split(|&byte| byte == b',') {
        let mut halves = pair.splitn(2, |&byte| byte == b'=');
        let (Some(key), Some(value)) = (halves.next(), halves.next()) else {
            continue;
        };
        let number = |bytes: &[u8]| -> u64 {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.parse().ok())
                .unwrap_or(0)
        };
        match key {
            b"a" => keys.action = value.first().copied().unwrap_or(b't'),
            b"f" => keys.format = number(value) as u16,
            b"t" => keys.transmission = value.first().copied().unwrap_or(b'd'),
            b"s" => keys.width = number(value) as u32,
            b"v" => keys.height = number(value) as u32,
            b"i" => keys.id = number(value) as u32,
            b"m" => keys.more = number(value) as u16,
            b"q" => keys.quiet = number(value) as u16,
            b"d" => keys.delete = value.first().copied().unwrap_or(b'a'),
            b"c" => keys.cols = number(value) as u16,
            b"r" => keys.rows = number(value) as u16,
            _ => {}
        }
    }
    keys
}

impl State {
    /// Entry point: a complete APC string starting with `G`.
    pub(crate) fn apc_graphics(&mut self, apc: &[u8]) {
        let body = &apc[1..];
        let (control, payload) = match body.iter().position(|&byte| byte == b';') {
            Some(split) => (&body[..split], &body[split + 1..]),
            None => (body, &[][..]),
        };
        let keys = parse_keys(control);

        match keys.action {
            b't' | b'T' => self.graphics_transmit(keys, payload, keys.action == b'T'),
            b'p' => self.graphics_place(keys),
            b'd' => self.graphics_delete(keys),
            b'q' => self.graphics_query(keys, payload),
            _ => self.graphics_reply(keys, "ENOTSUPPORTED:action"),
        }
    }

    fn graphics_transmit(&mut self, keys: Keys, payload: &[u8], display: bool) {
        if keys.transmission != b'd' {
            return self.graphics_reply(keys, "ENOTSUPPORTED:transmission");
        }
        let Some(chunk) = crate::events::base64_decode(payload) else {
            return self.graphics_reply(keys, "EINVAL:base64");
        };

        let mut pending = match self.graphics.pending.take() {
            // A continuation keeps the FIRST chunk's metadata; kitty sends none on
            // later chunks.
            Some(pending) if pending.id == keys.id || keys.id == 0 => pending,
            _ => PendingTransmit {
                id: keys.id,
                format: keys.format,
                width: keys.width,
                height: keys.height,
                quiet: keys.quiet,
                display,
                cols: keys.cols,
                rows: keys.rows,
                data: Vec::new(),
            },
        };
        pending.data.extend_from_slice(&chunk);
        if pending.data.len() > APC_CEILING * 32 {
            self.graphics_reply(keys, "ENOMEM:transmission too large");
            return;
        }
        if keys.more == 1 {
            self.graphics.pending = Some(pending);
            return;
        }
        self.graphics_finalize(pending);
    }

    fn graphics_finalize(&mut self, pending: PendingTransmit) {
        let keys = Keys {
            id: pending.id,
            quiet: pending.quiet,
            ..Keys::default()
        };
        let image = match decode_image(pending.format, pending.width, pending.height, pending.data)
        {
            Ok(image) => image,
            Err(error) => return self.graphics_reply(keys, error),
        };

        let cost = image.rgba.len();
        if self.graphics.budget_used + cost > BUDGET_BYTES {
            return self.graphics_reply(keys, "ENOMEM:budget");
        }
        if let Some(old) = self.graphics.images.insert(pending.id, image.clone()) {
            self.graphics.budget_used -= old.rgba.len();
        }
        self.graphics.budget_used += cost;
        self.graphics.ops.push(ImageOp::Add(pending.id, image.clone()));
        self.graphics_reply(keys, "OK");

        if pending.display {
            self.place_at_cursor(pending.id, pending.cols, pending.rows, &image);
        }
    }

    fn graphics_place(&mut self, keys: Keys) {
        let Some(image) = self.graphics.images.get(&keys.id).cloned() else {
            return self.graphics_reply(keys, "ENOENT:image");
        };
        self.place_at_cursor(keys.id, keys.cols, keys.rows, &image);
        self.graphics_reply(keys, "OK");
    }

    fn place_at_cursor(&mut self, id: u32, cols: u16, rows: u16, _image: &Image) {
        let (col, row) = (self.screen().x, self.screen().y);
        self.screen_mut().placements.push(Placement {
            image: id,
            col,
            row,
            cols,
            rows,
        });
        // Every row the placement MIGHT cover is stale. With an explicit span that is
        // exact; native-size spans are resolved renderer-side, so the whole frame is
        // marked -- placements change rarely enough that this is the honest cost.
        if cols > 0 && rows > 0 {
            for y in row..row.saturating_add(rows).min(self.screen().rows()) {
                self.screen_mut().grid.mark_dirty(y);
            }
            // The cursor steps past an explicitly-sized placement, kitty-style.
            let cols_total = self.screen().cols();
            self.screen_mut().x = col.saturating_add(cols).min(cols_total.saturating_sub(1));
        } else {
            self.mark_full_damage();
        }
    }

    /// Sixel arrives through the same placement path, native-sized, cursor unmoved
    /// (the v1 boundary shared with kitty's no-c,r case).
    pub(crate) fn place_sixel(&mut self, id: u32) {
        let image = self
            .graphics
            .images
            .get(&id)
            .cloned()
            .expect("placed immediately after insert");
        self.place_at_cursor(id, 0, 0, &image);
    }

    fn graphics_delete(&mut self, keys: Keys) {
        match keys.delete {
            b'a' | b'A' => {
                self.screen_mut().placements.clear();
                if keys.delete == b'A' {
                    for id in self.graphics.images.keys().copied().collect::<Vec<_>>() {
                        self.graphics.ops.push(ImageOp::Remove(id));
                    }
                    self.graphics.images.clear();
                    self.graphics.budget_used = 0;
                }
            }
            b'i' | b'I' => {
                self.screen_mut()
                    .placements
                    .retain(|placement| placement.image != keys.id);
                if keys.delete == b'I' {
                    if let Some(old) = self.graphics.images.remove(&keys.id) {
                        self.graphics.budget_used -= old.rgba.len();
                        self.graphics.ops.push(ImageOp::Remove(keys.id));
                    }
                }
            }
            _ => return self.graphics_reply(keys, "ENOTSUPPORTED:delete"),
        }
        self.mark_full_damage();
    }

    /// `a=q`: validate without storing, answer OK or the error. This is the probe icat
    /// sends before choosing the protocol -- it works because the reply seam exists.
    fn graphics_query(&mut self, keys: Keys, payload: &[u8]) {
        if keys.transmission != b'd' {
            return self.graphics_reply(keys, "ENOTSUPPORTED:transmission");
        }
        let result = crate::events::base64_decode(payload)
            .ok_or("EINVAL:base64")
            .and_then(|data| decode_image(keys.format, keys.width, keys.height, data));
        match result {
            Ok(_) => self.graphics_reply(keys, "OK"),
            Err(error) => self.graphics_reply(keys, error),
        }
    }

    /// Kitty responses ride APC back: `ESC _ G i=<id> ; <message> ESC \`. `q=2`
    /// suppresses everything, `q=1` suppresses OK, errors always answer.
    fn graphics_reply(&mut self, keys: Keys, message: &str) {
        if keys.quiet >= 2 || (keys.quiet == 1 && message == "OK") {
            return;
        }
        let reply = format!("\x1b_Gi={};{}\x1b\\", keys.id, message);
        self.replies.extend_from_slice(reply.as_bytes());
    }
}

fn decode_image(
    format: u16,
    width: u32,
    height: u32,
    data: Vec<u8>,
) -> Result<Image, &'static str> {
    match format {
        32 => {
            let expected = width as usize * height as usize * 4;
            if width == 0 || height == 0 || data.len() != expected {
                return Err("EINVAL:dimensions");
            }
            Ok(Image {
                width,
                height,
                rgba: data,
            })
        }
        24 => {
            let expected = width as usize * height as usize * 3;
            if width == 0 || height == 0 || data.len() != expected {
                return Err("EINVAL:dimensions");
            }
            let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
            for pixel in data.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            Ok(Image {
                width,
                height,
                rgba,
            })
        }
        100 => {
            let decoder = png::Decoder::new(std::io::Cursor::new(data));
            let mut reader = decoder.read_info().map_err(|_| "EINVAL:png")?;
            let mut buffer = vec![0u8; reader.output_buffer_size()];
            let info = reader.next_frame(&mut buffer).map_err(|_| "EINVAL:png")?;
            buffer.truncate(info.buffer_size());
            let rgba = match info.color_type {
                png::ColorType::Rgba => buffer,
                png::ColorType::Rgb => {
                    let mut rgba = Vec::with_capacity(buffer.len() / 3 * 4);
                    for pixel in buffer.chunks_exact(3) {
                        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                    }
                    rgba
                }
                _ => return Err("ENOTSUPPORTED:png color type"),
            };
            Ok(Image {
                width: info.width,
                height: info.height,
                rgba,
            })
        }
        _ => Err("ENOTSUPPORTED:format"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    fn b64(data: &[u8]) -> String {
        // A tiny encoder for tests only; the decoder under test is the real one.
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let mut acc = 0u32;
            for (index, &byte) in chunk.iter().enumerate() {
                acc |= u32::from(byte) << (16 - 8 * index);
            }
            for position in 0..4 {
                if position <= chunk.len() {
                    out.push(ALPHABET[((acc >> (18 - 6 * position)) & 0x3F) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn a_direct_rgba_transmit_stores_and_replies_ok() {
        let mut terminal = Terminal::new(20, 5);
        let pixels = [255, 0, 0, 255, 0, 255, 0, 255];
        terminal.write(format!("\x1b_Ga=t,f=32,s=2,v=1,i=7;{}\x1b\\", b64(&pixels)).as_bytes());
        assert_eq!(terminal.take_replies(), b"\x1b_Gi=7;OK\x1b\\");
        let ops = terminal.take_image_ops();
        assert_eq!(ops.len(), 1);
        let ImageOp::Add(7, image) = &ops[0] else {
            panic!("expected Add(7, ..), got {ops:?}");
        };
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, pixels);
    }

    #[test]
    fn chunked_transmission_reassembles() {
        let mut terminal = Terminal::new(20, 5);
        let pixels = [1, 2, 3, 255, 4, 5, 6, 255];
        let encoded = b64(&pixels);
        let (head, tail) = encoded.split_at(4);
        terminal.write(format!("\x1b_Ga=t,f=32,s=2,v=1,i=9,m=1;{head}\x1b\\").as_bytes());
        assert_eq!(terminal.take_image_ops(), vec![], "not finalized yet");
        terminal.write(format!("\x1b_Gm=0;{tail}\x1b\\").as_bytes());
        let ops = terminal.take_image_ops();
        let ImageOp::Add(9, image) = &ops[0] else {
            panic!("expected Add(9, ..), got {ops:?}");
        };
        assert_eq!(image.rgba, pixels);
    }

    #[test]
    fn transmit_and_display_places_at_the_cursor_and_advances_it() {
        let mut terminal = Terminal::new(20, 5);
        let pixels = [9, 9, 9, 255];
        terminal.write(b"\x1b[2;3H");
        terminal
            .write(format!("\x1b_Ga=T,f=32,s=1,v=1,i=3,c=4,r=2;{}\x1b\\", b64(&pixels)).as_bytes());
        let placements = terminal.screen().placements.clone();
        assert_eq!(
            placements,
            vec![Placement {
                image: 3,
                col: 2,
                row: 1,
                cols: 4,
                rows: 2
            }]
        );
        assert_eq!(terminal.cursor().x, 6, "cursor stepped past the placement");
    }

    #[test]
    fn queries_answer_without_storing() {
        let mut terminal = Terminal::new(20, 5);
        let pixels = [1, 1, 1];
        terminal.write(format!("\x1b_Ga=q,f=24,s=1,v=1,i=31;{}\x1b\\", b64(&pixels)).as_bytes());
        assert_eq!(terminal.take_replies(), b"\x1b_Gi=31;OK\x1b\\");
        assert_eq!(terminal.take_image_ops(), vec![], "a query stores nothing");

        let mut terminal = Terminal::new(20, 5);
        terminal.write(b"\x1b_Ga=q,t=f,i=5;\x1b\\");
        assert_eq!(
            terminal.take_replies(),
            b"\x1b_Gi=5;ENOTSUPPORTED:transmission\x1b\\",
            "file transmission is refused loudly, which is what makes icat fall back"
        );
    }

    #[test]
    fn delete_by_id_and_delete_all() {
        let mut terminal = Terminal::new(20, 5);
        let pixels = [8, 8, 8, 255];
        let encoded = b64(&pixels);
        terminal.write(format!("\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1;{encoded}\x1b\\").as_bytes());
        terminal.write(format!("\x1b_Ga=T,f=32,s=1,v=1,i=2,c=1,r=1;{encoded}\x1b\\").as_bytes());
        terminal.take_image_ops();

        terminal.write(b"\x1b_Ga=d,d=i,i=1\x1b\\");
        assert_eq!(terminal.screen().placements.len(), 1, "only image 1's went");
        assert_eq!(terminal.take_image_ops(), vec![], "lowercase keeps the data");

        terminal.write(b"\x1b_Ga=d,d=A\x1b\\");
        assert!(terminal.screen().placements.is_empty());
        let ops = terminal.take_image_ops();
        assert_eq!(ops.len(), 2, "uppercase frees the store: {ops:?}");
    }

    #[test]
    fn wrong_dimensions_and_bad_base64_answer_errors() {
        let mut terminal = Terminal::new(20, 5);
        terminal.write(format!("\x1b_Ga=t,f=32,s=9,v=9,i=4;{}\x1b\\", b64(&[0, 0, 0, 0])).as_bytes());
        assert_eq!(terminal.take_replies(), b"\x1b_Gi=4;EINVAL:dimensions\x1b\\");
        terminal.write(b"\x1b_Ga=t,f=32,s=1,v=1,i=4;!!!\x1b\\");
        assert_eq!(terminal.take_replies(), b"\x1b_Gi=4;EINVAL:base64\x1b\\");
    }
}
