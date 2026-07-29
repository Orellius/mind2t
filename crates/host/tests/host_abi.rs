//! Slice 8's harness: can anything see a pixel cross the C boundary?
//!
//! The tenth blind spot in a row. Every earlier harness stops at a Rust API: vim.rs proves
//! the pty -> frame -> renderer chain, backend.rs proves CPU == GPU, differential.rs proves
//! the `ghostty_*` readout -- and a `ruuah_host_*` surface that returned a black buffer, or
//! never polled, or drew with the wrong font size, would fail none of them.
//!
//! The oracle: spawn a child through the C surface whose output is a known byte sequence,
//! and byte-compare the polled RGBA against a reference renderer fed the identical bytes
//! through the Rust API. The reference publishes with the same `Publisher` the pump uses
//! (equality by construction, not by reimplementation) and draws on the CPU backend, so the
//! comparison also re-asserts CPU == GPU through the C boundary.
//!
//! Sensitivity control (Phase 2, when poll can draw at all): a host whose draw skips a row
//! must fail the same comparison.

use std::ffi::CString;
use std::ptr;
use std::time::{Duration, Instant};

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{BaseDirection, Frame, Publisher, channel};
use ruuah_vt_host::{
    DEFAULT_FONT_SIZE, RuuahConfig, RuuahHost, RuuahHostFrame, RuuahHostOptions, RuuahHostResult,
    ruuah_config_error, ruuah_config_free, ruuah_config_load, ruuah_host_free, ruuah_host_paste,
    ruuah_host_poll, ruuah_host_poll_skipping_row_for_testing, ruuah_host_resize,
    ruuah_host_row_text, ruuah_host_send, ruuah_host_spawn,
};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(10);

/// What the child's printf produces on the wire: the pty's ONLCR turns `\n` into `\r\n`.
const WIRE: &[u8] = b"RUUAH-VT-HOST\r\n";

/// The pixels a correct host must produce for WIRE, rendered through the Rust API.
fn reference_pixels() -> (Vec<u8>, u32, u32) {
    reference_pixels_for(WIRE, BaseDirection::LeftToRight)
}

/// The reference oracle, parameterized: identical bytes through the Rust API, published
/// with the same `Publisher` the pump uses, laid out under the given base direction.
fn reference_pixels_for(wire: &[u8], base: BaseDirection) -> (Vec<u8>, u32, u32) {
    let mut terminal = Terminal::new(COLS, ROWS);
    terminal.write(wire);

    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("publish reference");

    let mut frame = Frame::new();
    frame.base_direction = base;
    reader.read_into(&mut frame);
    assert!(frame.is_valid(), "single-threaded read cannot tear");

    let fonts = FontStack::system(DEFAULT_FONT_SIZE).expect("system fonts");
    let mut renderer = Renderer::new(fonts, COLS, ROWS);
    renderer.draw_all(&frame);
    let width = renderer.canvas().width();
    let height = renderer.canvas().height();
    (renderer.pixels(), width, height)
}

/// First index at which the buffers differ, for a failure message that locates the pixel.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[test]
fn host_pixels_match_a_reference_renderer_fed_the_same_bytes() {
    let (want, want_width, want_height) = reference_pixels();

    let command = CString::new("printf 'RUUAH-VT-HOST\\n'").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    let result = unsafe { ruuah_host_spawn(&options, &mut host) };
    assert_eq!(
        result,
        RuuahHostResult::Success,
        "spawn through the C surface failed: {result:?}"
    );

    let mut last: Option<RuuahHostFrame> = None;
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        let result = unsafe { ruuah_host_poll(host, &mut polled) };
        assert_eq!(result, RuuahHostResult::Success, "poll failed: {result:?}");

        if !polled.pixels.is_null() {
            assert_eq!((polled.width, polled.height), (want_width, want_height));
            let len = polled.width as usize * polled.height as usize * 4;
            let got = unsafe { std::slice::from_raw_parts(polled.pixels, len) };
            if got == want.as_slice() {
                unsafe { ruuah_host_free(host) };
                return;
            }
            last = Some(polled);
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let verdict = match last {
        Some(frame) => {
            let len = frame.width as usize * frame.height as usize * 4;
            let got = unsafe { std::slice::from_raw_parts(frame.pixels, len) };
            match first_difference(got, &want) {
                Some(index) => {
                    let pixel = index / 4;
                    format!(
                        "last frame (generation {}) differs from the reference at byte {index} \
                         (pixel x={} y={})",
                        frame.generation,
                        pixel % frame.width as usize,
                        pixel / frame.width as usize,
                    )
                }
                None => "buffers differ only in length".to_string(),
            }
        }
        None => "no poll ever returned pixels".to_string(),
    };
    unsafe { ruuah_host_free(host) };
    panic!("no polled frame matched the reference within {PATIENCE:?}: {verdict}");
}

/// The failure contract from the first stub onward: a refused spawn NULLs the out-param
/// (the same rule finding 5 pinned on `ghostty_terminal_new`).
#[test]
fn a_refused_spawn_nulls_the_out_param() {
    let options = RuuahHostOptions {
        cols: 0,
        rows: ROWS,
        font_size: 0.0,
        command: ptr::null(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::dangling_mut();
    let result = unsafe { ruuah_host_spawn(&options, &mut host) };
    assert_eq!(result, RuuahHostResult::InvalidValue);
    assert!(
        host.is_null(),
        "a failed spawn must not leave *out dangling"
    );
}

/// The sensitivity control: a host whose draw skips the text row must fail the comparison
/// the passing test relies on -- and fail it inside the skipped row, not by accident.
#[test]
fn a_host_that_skips_a_row_is_caught() {
    let (want, _, _) = reference_pixels();

    let command = CString::new("printf 'RUUAH-VT-HOST\\n'").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    let result = unsafe { ruuah_host_spawn(&options, &mut host) };
    assert_eq!(result, RuuahHostResult::Success);

    // Drain until the child is gone and its final frame has been drawn, exactly as the
    // passing test would -- except every draw declines row 0, where the text lives.
    let deadline = Instant::now() + PATIENCE;
    let mut frame = None;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        let result = unsafe { ruuah_host_poll_skipping_row_for_testing(host, 0, &mut polled) };
        assert_eq!(result, RuuahHostResult::Success);
        if polled.child_exited && !polled.pixels.is_null() && !polled.drew {
            frame = Some(polled);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let frame = frame.expect("the broken host never settled on a frame");

    let len = frame.width as usize * frame.height as usize * 4;
    let got = unsafe { std::slice::from_raw_parts(frame.pixels, len) };
    let index = first_difference(got, &want)
        .expect("a renderer that skipped the text row still matched the reference");
    let row_band = frame.width as usize
        * FontStack::system(DEFAULT_FONT_SIZE)
            .expect("system fonts")
            .metrics()
            .height as usize
        * 4;
    assert!(
        index < row_band,
        "the difference must lie in the skipped row's band, not at byte {index}"
    );
    unsafe { ruuah_host_free(host) };
}

/// The send seam, end to end: bytes written through the C surface reach the child's input,
/// and what the pty and the child do with them comes back as pixels. `cat` makes the round
/// trip observable -- the line discipline echoes the typed line, then cat repeats it.
#[test]
fn send_reaches_the_child_and_comes_back_as_pixels() {
    // The tty echoes "ping\r" as "ping\r\n"; cat then writes "ping\n", which ONLCR turns
    // into "ping\r\n" again. Two identical rows.
    let reference = {
        let mut terminal = Terminal::new(COLS, ROWS);
        terminal.write(b"ping\r\nping\r\n");
        let (writer, reader) = channel(COLS, ROWS);
        let mut publisher = Publisher::new(writer);
        publisher.publish(&mut terminal).expect("publish reference");
        let mut frame = Frame::new();
        reader.read_into(&mut frame);
        let fonts = FontStack::system(DEFAULT_FONT_SIZE).expect("system fonts");
        let mut renderer = Renderer::new(fonts, COLS, ROWS);
        renderer.draw_all(&frame);
        renderer.pixels()
    };

    let command = CString::new("cat").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let line = b"ping\r";
    assert_eq!(
        unsafe { ruuah_host_send(host, line.as_ptr(), line.len()) },
        RuuahHostResult::Success
    );

    let deadline = Instant::now() + PATIENCE;
    let mut matched = false;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if !polled.pixels.is_null() {
            let len = polled.width as usize * polled.height as usize * 4;
            let got = unsafe { std::slice::from_raw_parts(polled.pixels, len) };
            if got == reference.as_slice() {
                matched = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { ruuah_host_free(host) };
    assert!(
        matched,
        "the sent line never came back as the expected pixels"
    );
}

/// The `auto_direction` option must be provably not inert (the SCAR-004 shape: a flag
/// that changes nothing looks exactly like a flag that works). Two oracles are built for
/// the same Hebrew line — LTR base and Auto base — and must DISAGREE with each other
/// (sensitivity: the line really exercises reordering), and a host spawned with the flag
/// must byte-match the Auto oracle, which the LTR layout by construction cannot.
#[test]
fn auto_direction_reorders_a_hebrew_row_through_the_c_boundary() {
    const HEBREW_WIRE: &[u8] = "שלום עולם\r\n".as_bytes();
    let (ltr, ..) = reference_pixels_for(HEBREW_WIRE, BaseDirection::LeftToRight);
    let (want, want_width, want_height) =
        reference_pixels_for(HEBREW_WIRE, BaseDirection::Auto);
    assert_ne!(
        ltr, want,
        "the Hebrew line lays out identically under both bases — the oracle cannot see the flag"
    );

    let command = CString::new("printf 'שלום עולם\\n'").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: true,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    let result = unsafe { ruuah_host_spawn(&options, &mut host) };
    assert_eq!(result, RuuahHostResult::Success, "spawn failed: {result:?}");

    let mut matched = false;
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if !polled.pixels.is_null() {
            assert_eq!((polled.width, polled.height), (want_width, want_height));
            let len = polled.width as usize * polled.height as usize * 4;
            let got = unsafe { std::slice::from_raw_parts(polled.pixels, len) };
            if got == want.as_slice() {
                matched = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { ruuah_host_free(host) };
    assert!(
        matched,
        "a host spawned with auto_direction never produced the Auto-base layout"
    );
}

/// Waits until the host's polled pixels byte-match `want`, or the patience runs out.
fn poll_until_pixels(host: *mut RuuahHost, want: &[u8]) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if !polled.pixels.is_null() {
            let len = polled.width as usize * polled.height as usize * 4;
            let got = unsafe { std::slice::from_raw_parts(polled.pixels, len) };
            if got == want {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The paste seam under bracketed paste, end to end. The child enables mode 2004 and
/// prints READY; polling until READY is visible guarantees a frame carrying the mode bit
/// was polled (the mode is set in the same write). The paste then arrives at the pty
/// wrapped in ESC[200~ / ESC[201~, and the proof needs no cooperating child at all: the
/// pty's default ECHOCTL renders the pasted ESC as the two printables `^[`, so the
/// fenceposts come back as visible grid text. A paste that ignored the mode would echo
/// bare `hi` and match nothing here -- the control below pins the opposite direction.
#[test]
fn a_paste_is_fenced_when_the_child_enabled_2004() {
    let enable_2004 = b"\x1b[?2004hREADY\r\n";
    let (ready, ..) = reference_pixels_for(enable_2004, BaseDirection::LeftToRight);
    let after_paste = b"\x1b[?2004hREADY\r\n^[[200~hi^[[201~";
    let (want, ..) = reference_pixels_for(after_paste, BaseDirection::LeftToRight);

    let command = CString::new("printf '\\033[?2004hREADY\\n'; exec cat").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let saw_ready = poll_until_pixels(host, &ready);
    if !saw_ready {
        unsafe { ruuah_host_free(host) };
        panic!("the child's READY line never appeared, so the mode bit was never polled");
    }

    let paste = b"hi";
    assert_eq!(
        unsafe { ruuah_host_paste(host, paste.as_ptr(), paste.len()) },
        RuuahHostResult::Success
    );
    let matched = poll_until_pixels(host, &want);
    unsafe { ruuah_host_free(host) };
    assert!(matched, "the fenced paste never echoed back as pixels");
}

/// The control for the test above: without mode 2004 the same paste must arrive bare.
/// A paste path that fences unconditionally echoes `^[[200~hi^[[201~` here and never
/// matches; together the pair proves the fence is governed by the mode rather than
/// hardcoded in either direction.
#[test]
fn a_paste_is_bare_when_the_child_did_not_enable_2004() {
    let (ready, ..) = reference_pixels_for(b"READY\r\n", BaseDirection::LeftToRight);
    let (want, ..) = reference_pixels_for(b"READY\r\nhi", BaseDirection::LeftToRight);

    let command = CString::new("printf 'READY\\n'; exec cat").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let saw_ready = poll_until_pixels(host, &ready);
    if !saw_ready {
        unsafe { ruuah_host_free(host) };
        panic!("the child's READY line never appeared");
    }

    let paste = b"hi";
    assert_eq!(
        unsafe { ruuah_host_paste(host, paste.as_ptr(), paste.len()) },
        RuuahHostResult::Success
    );
    let matched = poll_until_pixels(host, &want);
    unsafe { ruuah_host_free(host) };
    assert!(matched, "the bare paste never echoed back as pixels");
}

/// S1's observable: a theme with a distinct background must recolor the grid AND the
/// reported margin background through the C surface -- and survive a resize, because
/// resize rebuilds the renderer and a rebuild that forgets the theme silently reverts
/// to the built-in scheme (the looks-like-success shape).
///
/// The control is `a_null_config_keeps_the_builtin_scheme` below: together the pair
/// proves the color comes from the theme file rather than either default.
#[test]
fn a_theme_background_reaches_the_pixels_and_survives_a_resize() {
    const THEME_BG: [u8; 4] = [0x20, 0x40, 0x60, 255];

    let dir = tempdir();
    std::fs::create_dir_all(dir.join("themes")).expect("themes dir");
    std::fs::write(dir.join("config.toml"), "theme = \"deep\"\n").expect("config");
    std::fs::write(dir.join("themes/deep.toml"), "background = \"#204060\"\n").expect("theme");

    let dir_c = CString::new(dir.to_str().expect("utf-8 tempdir")).expect("dir");
    let mut config: *mut RuuahConfig = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_config_load(dir_c.as_ptr(), &mut config) },
        RuuahHostResult::Success
    );
    assert!(
        unsafe { ruuah_config_error(config) }.is_null(),
        "this config must load clean"
    );

    // A child that keeps producing output, so frames keep arriving after the resize.
    let command = CString::new("while :; do printf X; sleep 0.2; done").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config,
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );
    // The config contributes at spawn/resize only; freeing it here pins that lifetime.
    unsafe { ruuah_config_free(config) };

    let polled = poll_until_background(host, THEME_BG)
        .unwrap_or_else(|got| die(host, format!("themed background never appeared; last {got:?}")));
    assert_corner_pixel(&polled, THEME_BG, "after spawn");

    // The trap this test exists for: resize rebuilds the renderer.
    assert_eq!(
        unsafe { ruuah_host_resize(host, COLS - 20, ROWS - 4) },
        RuuahHostResult::Success
    );
    let polled = poll_until_background(host, THEME_BG).unwrap_or_else(|got| {
        die(host, format!("the theme did not survive the resize; last {got:?}"))
    });
    assert_corner_pixel(&polled, THEME_BG, "after resize");
    unsafe { ruuah_host_free(host) };
}

/// The pair's control: the identical child with a NULL config wears the built-in
/// near-black. If theme application were hardcoded or leaked between handles, one of
/// the two tests would fail.
#[test]
fn a_null_config_keeps_the_builtin_scheme() {
    let builtin = ruuah_vt_render::Palette::default().default_background;

    let command = CString::new("while :; do printf X; sleep 0.2; done").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );
    let polled = poll_until_background(host, builtin)
        .unwrap_or_else(|got| die(host, format!("builtin background never appeared; got {got:?}")));
    assert_corner_pixel(&polled, builtin, "with a NULL config");
    unsafe { ruuah_host_free(host) };
}

/// Polls until a drawn frame reports the wanted margin background. Ok carries the frame
/// for further pixel assertions; Err carries the last background seen.
fn poll_until_background(
    host: *mut RuuahHost,
    want: [u8; 4],
) -> Result<RuuahHostFrame, [u8; 4]> {
    let deadline = Instant::now() + PATIENCE;
    let mut last = [0; 4];
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        let result = unsafe { ruuah_host_poll(host, &mut polled) };
        assert_eq!(result, RuuahHostResult::Success, "poll failed: {result:?}");
        if !polled.pixels.is_null() {
            if polled.background == want {
                return Ok(polled);
            }
            last = polled.background;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(last)
}

/// The bottom-right pixel belongs to a cell no child in these tests ever touches, so it
/// must wear the default background -- which is where a theme shows up as actual ink.
fn assert_corner_pixel(frame: &RuuahHostFrame, want: [u8; 4], when: &str) {
    let len = frame.width as usize * frame.height as usize * 4;
    let pixels = unsafe { std::slice::from_raw_parts(frame.pixels, len) };
    let corner = &pixels[len - 4..];
    assert_eq!(
        &corner[..3],
        &want[..3],
        "{when}: the untouched corner pixel does not wear the expected background"
    );
}

fn die(host: *mut RuuahHost, message: String) -> ! {
    unsafe { ruuah_host_free(host) };
    panic!("{message}");
}

/// A zeroed out-param for polls, matching the C caller's `RuuahHostFrame frame = {0}`.
fn empty_frame() -> RuuahHostFrame {
    RuuahHostFrame {
        pixels: ptr::null(),
        width: 0,
        height: 0,
        generation: 0,
        drew: false,
        child_exited: false,
        background: [0; 4],
        row_semantics: ptr::null(),
        row_count: 0,
    }
}

/// S2a: the OSC 133 marks the core tracks must cross the C surface as per-row classes,
/// and the text of those rows must be readable back -- the gutter and the copy actions
/// are built entirely on these two. The child emits the marks itself (printf, no shell
/// integration involved), so the test isolates the seam.
///
/// The control is `rows_without_osc133_all_read_as_output` below: same child shape, no
/// marks, all zeros -- together they prove the classes come from OSC 133 and not from a
/// hardcoded pattern.
#[test]
fn osc133_rows_cross_the_c_surface_with_their_text() {
    // Row 0: a prompt mark, the prompt text, an input mark, the typed command.
    // Row 1: an output mark, then output text.
    let command = CString::new(
        "printf '\\033]133;A\\007$ \\033]133;B\\007ls -la\\n\\033]133;C\\007total 42\\n'; sleep 8",
    )
    .expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    // Poll until the prompt row classifies -- the child's output arrives when it arrives.
    let deadline = Instant::now() + PATIENCE;
    let mut classes: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if !polled.row_semantics.is_null() {
            let all = unsafe {
                std::slice::from_raw_parts(polled.row_semantics, polled.row_count as usize)
            };
            if all.first() == Some(&1) {
                classes = all.to_vec();
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if classes.is_empty() {
        die(host, "the prompt row never classified as RUUAH_ROW_PROMPT".into());
    }
    assert_eq!(classes.len(), ROWS as usize, "one class per grid row");
    assert_eq!(classes[0], 1, "row 0 starts a prompt (and holds the input)");
    assert_eq!(classes[1], 0, "row 1 is command output");

    let text_of = |row: u16, semantic: u8| -> String {
        let mut len = 0usize;
        assert_eq!(
            unsafe { ruuah_host_row_text(host, row, semantic, ptr::null_mut(), 0, &mut len) },
            RuuahHostResult::Success
        );
        let mut buffer = vec![0u8; len];
        assert_eq!(
            unsafe {
                ruuah_host_row_text(host, row, semantic, buffer.as_mut_ptr(), len, &mut len)
            },
            RuuahHostResult::Success
        );
        String::from_utf8(buffer).expect("row text is UTF-8")
    };
    assert_eq!(text_of(0, 255), "$ ls -la", "prompt + typed command, blanks trimmed");
    assert_eq!(text_of(1, 255), "total 42", "the output row's text");
    // The filter is what makes "copy command" exact -- and it discriminates: a filter
    // that ignored the marks would return the whole line here.
    assert_eq!(text_of(0, 2), "ls -la", "the input filter drops the prompt itself");
    assert_eq!(text_of(0, 1), "$", "the prompt filter keeps only the shell's own cells");

    let mut len = 0usize;
    assert_eq!(
        unsafe { ruuah_host_row_text(host, ROWS + 5, 255, ptr::null_mut(), 0, &mut len) },
        RuuahHostResult::InvalidValue,
        "an out-of-range row is refused, not clamped"
    );
    unsafe { ruuah_host_free(host) };
}

/// The pair's control: the same shape of child with NO marks must classify every row as
/// output. A classifier keying on the '$ ' text instead of OSC 133 fails here.
#[test]
fn rows_without_osc133_all_read_as_output() {
    let command = CString::new("printf '$ ls -la\\ntotal 42\\n'; sleep 8").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let deadline = Instant::now() + PATIENCE;
    let mut seen_text = false;
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if !polled.row_semantics.is_null() {
            let classes = unsafe {
                std::slice::from_raw_parts(polled.row_semantics, polled.row_count as usize)
            };
            assert!(
                classes.iter().all(|class| *class == 0),
                "unmarked rows must all be RUUAH_ROW_OUTPUT, got {classes:?}"
            );
            let mut len = 0usize;
            let ok = unsafe { ruuah_host_row_text(host, 1, 255, ptr::null_mut(), 0, &mut len) };
            if ok == RuuahHostResult::Success && len > 0 {
                seen_text = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { ruuah_host_free(host) };
    assert!(seen_text, "the child's second line never arrived");
}

/// A per-test unique directory under the OS tmp dir; leaked on purpose so a failing
/// test's files are still there to read.
fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ruuah-host-abi-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("tempdir");
    dir
}

/// The zoom seam: a live font-size change must reach the pixels through the C surface.
/// Same grid, bigger font => the polled frame's pixel dimensions must grow, and the
/// metric query that drives the GUI's grid math must be monotonic in the size. A
/// set_font_size that silently kept the old renderer passes neither.
#[test]
fn a_live_font_size_change_reaches_the_polled_pixels() {
    use ruuah_vt_host::{ruuah_host_cell_metrics, ruuah_host_set_font_size};

    let (mut small_w, mut small_h) = (0u32, 0u32);
    let (mut large_w, mut large_h) = (0u32, 0u32);
    assert_eq!(
        unsafe { ruuah_host_cell_metrics(14.0, &mut small_w, &mut small_h) },
        RuuahHostResult::Success
    );
    assert_eq!(
        unsafe { ruuah_host_cell_metrics(28.0, &mut large_w, &mut large_h) },
        RuuahHostResult::Success
    );
    assert!(
        large_w > small_w && large_h > small_h,
        "cell metrics must grow with the font size: {small_w}x{small_h} -> {large_w}x{large_h}"
    );
    assert_eq!(
        unsafe { ruuah_host_cell_metrics(0.0, &mut small_w, &mut small_h) },
        RuuahHostResult::InvalidValue,
        "a zero size is refused, not defaulted"
    );

    let command = CString::new("printf 'ZOOM'; sleep 8").expect("command");
    let options = RuuahHostOptions {
        cols: 20,
        rows: 4,
        font_size: 14.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let deadline = Instant::now() + PATIENCE;
    let mut before = (0u32, 0u32);
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if polled.drew && polled.width > 0 {
            before = (polled.width, polled.height);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if before.0 == 0 {
        die(host, "the small-font frame never drew".into());
    }
    assert_eq!(before.0, 20 * small_w, "width is cols * cell width");

    assert_eq!(
        unsafe { ruuah_host_set_font_size(host, 28.0, 20, 4) },
        RuuahHostResult::Success
    );
    let deadline = Instant::now() + PATIENCE;
    let mut after = (0u32, 0u32);
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if polled.drew && polled.width > before.0 {
            after = (polled.width, polled.height);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if after.0 == 0 {
        die(host, "the frame never re-drew at the larger font".into());
    }
    assert_eq!(after.0, 20 * large_w, "the new width wears the 28px metrics");
    assert!(after.1 > before.1, "height grew with the font");
    unsafe { ruuah_host_free(host) };
}

/// OSC 8 through the C boundary: the URI a child printed under comes back from
/// ruuah_host_link_at at the linked cell, an unlinked cell answers SUCCESS with len 0,
/// and out-of-range is refused. The child emits the marks itself, no shell involved.
#[test]
fn a_hyperlink_is_readable_at_its_cell_through_the_c_surface() {
    use ruuah_vt_host::ruuah_host_link_at;

    let command =
        CString::new("printf 'a\\033]8;;https://x.il/p\\033\\\\bc\\033]8;;\\033\\\\d'; sleep 8")
            .expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let link = |host, col, row| -> Option<(RuuahHostResult, String)> {
        let mut len = 0usize;
        let result = unsafe { ruuah_host_link_at(host, col, row, ptr::null_mut(), 0, &mut len) };
        if result != RuuahHostResult::Success {
            return Some((result, String::new()));
        }
        let mut buffer = vec![0u8; len];
        let result = unsafe {
            ruuah_host_link_at(host, col, row, buffer.as_mut_ptr(), len, &mut len)
        };
        Some((result, String::from_utf8(buffer).ok()?))
    };

    let deadline = Instant::now() + PATIENCE;
    let mut found = String::new();
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        if let Some((RuuahHostResult::Success, uri)) = link(host, 1, 0) {
            if !uri.is_empty() {
                found = uri;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if found.is_empty() {
        die(host, "the linked cell never answered with a URI".into());
    }
    assert_eq!(found, "https://x.il/p");
    assert_eq!(
        link(host, 2, 0),
        Some((RuuahHostResult::Success, "https://x.il/p".into())),
        "the second linked cell"
    );

    // The control half: cell 0 was printed before the link and must answer len 0.
    let mut len = 5usize;
    assert_eq!(
        unsafe { ruuah_host_link_at(host, 0, 0, ptr::null_mut(), 0, &mut len) },
        RuuahHostResult::Success
    );
    assert_eq!(len, 0, "an unlinked cell is SUCCESS with no bytes");
    assert_eq!(
        unsafe { ruuah_host_link_at(host, COLS + 1, 0, ptr::null_mut(), 0, &mut len) },
        RuuahHostResult::InvalidValue,
        "out of range is refused"
    );
    unsafe { ruuah_host_free(host) };
}

/// The event seam through the C boundary: OSC 52 lands as a clipboard event with its
/// payload decoded, OSC 777 as a notification, BEL as a bell -- in order, exactly once,
/// and a sizing call (cap too small) must NOT consume. The control is the empty queue
/// answering kind 0 both before and after.
#[test]
fn host_events_cross_in_order_and_exactly_once() {
    use ruuah_vt_host::ruuah_host_next_event;

    let command = CString::new(
        "printf '\\033]52;c;aGVsbG8=\\007\\033]777;notify;T;B\\007\\007'; sleep 8",
    )
    .expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let next = |host| -> (u32, Vec<u8>) {
        let (mut kind, mut len) = (0u32, 0usize);
        assert_eq!(
            unsafe { ruuah_host_next_event(host, &mut kind, ptr::null_mut(), 0, &mut len) },
            RuuahHostResult::Success
        );
        if kind == 0 {
            return (0, Vec::new());
        }
        if len == 0 {
            // An empty payload fits cap 0, so the first call already consumed it.
            return (kind, Vec::new());
        }
        let mut buffer = vec![0u8; len];
        let mut fetched = 0u32;
        assert_eq!(
            unsafe {
                ruuah_host_next_event(host, &mut fetched, buffer.as_mut_ptr(), len, &mut len)
            },
            RuuahHostResult::Success
        );
        assert_eq!(fetched, kind, "the sizing call must not have consumed it");
        (kind, buffer)
    };

    let deadline = Instant::now() + PATIENCE;
    let mut first = (0u32, Vec::new());
    while Instant::now() < deadline {
        first = next(host);
        if first.0 != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(first.0, 1, "clipboard first");
    assert_eq!(first.1, b"hello", "base64 decoded before crossing");

    let deadline = Instant::now() + PATIENCE;
    let mut second = (0u32, Vec::new());
    while Instant::now() < deadline {
        second = next(host);
        if second.0 != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(second.0, 2, "notification second");
    assert_eq!(second.1, b"T\nB");

    let deadline = Instant::now() + PATIENCE;
    let mut third = (0u32, Vec::new());
    while Instant::now() < deadline {
        third = next(host);
        if third.0 != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(third.0, 3, "bell third");

    let (kind, _) = next(host);
    assert_eq!(kind, 0, "drained: exactly once means nothing repeats");
    unsafe { ruuah_host_free(host) };
}

/// Slice 9's seam end to end: a DSR from the child produces reply bytes that travel
/// back down the pty. The child's line discipline (canonical, ECHOCTL) echoes them as
/// printable ^[[0n -- the same fencepost trick the paste test measured -- so the reply
/// is visible as cells without any od. Seen red with the pump's drain skipped.
#[test]
fn a_dsr_reply_travels_back_down_the_pty() {
    let command = CString::new("printf 'Q\\033[5n\\n'; sleep 8").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );

    let text_of = |host, row: u16| -> String {
        let mut len = 0usize;
        if unsafe { ruuah_host_row_text(host, row, 255, ptr::null_mut(), 0, &mut len) }
            != RuuahHostResult::Success
        {
            return String::new();
        }
        let mut buffer = vec![0u8; len];
        if unsafe { ruuah_host_row_text(host, row, 255, buffer.as_mut_ptr(), len, &mut len) }
            != RuuahHostResult::Success
        {
            return String::new();
        }
        String::from_utf8_lossy(&buffer).into_owned()
    };

    let deadline = Instant::now() + PATIENCE;
    let mut seen = String::new();
    while Instant::now() < deadline {
        let mut polled = empty_frame();
        assert_eq!(
            unsafe { ruuah_host_poll(host, &mut polled) },
            RuuahHostResult::Success
        );
        let mut all = String::new();
        for row in 0..4 {
            all.push_str(&text_of(host, row));
            all.push('\n');
        }
        if all.contains("^[[0n") {
            seen = all;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        seen.contains("^[[0n"),
        "the DSR 5 reply never echoed back through the pty"
    );
    unsafe { ruuah_host_free(host) };
}
