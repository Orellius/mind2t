//! The blind spot this file closes: nothing in the suite could see that an Arabic letter's
//! FORM depends on its neighbours.
//!
//! Every shaping test before this one uses Latin, where there is one form, or Hebrew, where
//! the isolated and contextual forms of a letter are the same glyph. So a renderer that draws
//! every Arabic letter in its isolated form -- which is exactly what this one did -- scored
//! perfectly on all of them, and the README explained the result away as a property of
//! terminal grids.
//!
//! The observable is positional and needs no reference image: **the three cells of a
//! three-letter Arabic word must not all look the same.** Drawn isolated they are three
//! copies of one glyph and are pixel-identical; drawn joined they are the initial, medial and
//! final forms and are three different glyphs. Measured on this machine at 32px, beh alone is
//! glyph 867 and the same letter in a word resolves to 870 / 869 / 868.
//!
//! **A pixel comparison here is not a clean measurement of form, and the tests say where they
//! stop.** The Arabic faces installed on this machine are proportional -- beh advances 22.4px
//! against a 19px cell -- so a glyph drawn at its cell's origin OVERHANGS to the right, and
//! whether that overhang survives depends on paint order: in a right-to-left run cells are
//! painted in logical order, which is right to left on screen, so each letter's overhang lands
//! on a cell already painted. Measured 2026-08-08: an empty space between two Arabic letters
//! carries 50 inked pixels that belong to its neighbour. That is a separate defect from
//! joining, it predates this file, and it is recorded in `docs/BACKLOG-2026.md` rather than
//! papered over here. The consequence for these tests is that FORM is asserted against glyph
//! ids, and pixels are used only where the comparison is between two cells that are equally
//! contaminated or equally clean.
//!
//! ## The mutants, including the ones that lived
//!
//! Killed:
//!
//! - forcing `Script::Latin` on the run: swash gates its joining state machine on the script,
//!   so all three letters collapse to the nominal glyph. Three tests red.
//! - letting the pen walk the font's advance across cells instead of restarting per cell: the
//!   glyph ids stay correct and the letters drift off the grid, so only the PIXEL test goes
//!   red. That split is why both a glyph-id test and a pixel test are here.
//! - removing the run-of-one refusal.
//! - removing BOTH lam-alef guards at once.
//!
//! Survived, recorded rather than quietly dropped, because each says something true:
//!
//! - removing EITHER lam-alef guard alone. The cluster-spans-a-cell test and the
//!   one-cluster-per-cell count are redundant: swash emits lam-alef as a single cluster
//!   covering both cells, which trips both. Neither is dead; either alone would still refuse.
//! - removing the `is_joined` gates, in the segmenter and in the shaper, against the whole
//!   render suite. **No output test can see them.** Latin shaped through the run path resolves
//!   to the same nominal glyphs at the same per-cell positions, because that path enables no
//!   features. They stay as a SCOPE and COST gate -- they keep every row of code off the
//!   string-building path and keep a future font's default-on features away from Latin -- and
//!   that is a claim about blast radius, not one this file proves.
//! - removing the segmenter's boundary entirely, so a segment spans the space in
//!   `a_space_breaks_the_join`. It still passes, because a space is joining type U and the
//!   SHAPER already gives its neighbours isolated forms. So that test guards swash's behaviour,
//!   not the segmenter's, and saying otherwise would be the kind of false credit this project
//!   spends its time hunting.
//! - the one-script and one-font segment boundaries cannot be exercised at all here. Measured
//!   2026-08-08 against the live font stack: Arabic and Persian resolve, and N'Ko, Syriac,
//!   Adlam, Thaana and Mongolian all resolve to nothing, so there is no second cursive script
//!   on this machine to mix into a run. The boundaries are correct and unproven, and they are
//!   kept because a glyph id means nothing outside the font it came from.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, channel};
use ruuah_vt_render::{FontStack, Renderer, Shaper};

/// Large enough that a form difference is many pixels rather than a rounding artefact.
const SIZE: f32 = 32.0;

/// Arabic beh. Dual-joining, so it has all four forms, and three of them are distinct glyphs
/// in an Arabic face -- which is what makes a three-letter word of it a complete probe.
const BEH: char = '\u{0628}';

/// The ink of one cell, as a flat coverage mask. Compared for equality, never for beauty.
type Cell = Vec<bool>;

/// Renders `text` into a row `cols` wide and returns one mask per cell, left to right.
fn cells(text: &str, cols: u16) -> Vec<Cell> {
    let mut terminal = Terminal::new(cols, 1);
    terminal.write(b"\x1b[?25l"); // the cursor would otherwise paint a cell we measure
    terminal.write(text.as_bytes());

    let (writer, reader) = channel(cols, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("fits");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);

    let fonts = FontStack::system(SIZE).expect("system fonts");
    let metrics = fonts.metrics();
    let mut renderer = Renderer::new(fonts, cols, 1);
    renderer.draw_all(&frame);

    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();

    (0..cols)
        .map(|column| {
            let origin = u32::from(column) * metrics.width;
            let mut mask = Vec::with_capacity((metrics.width * metrics.height) as usize);
            for y in 0..metrics.height {
                for x in 0..metrics.width {
                    mask.push(canvas.pixel(origin + x, y) != background);
                }
            }
            mask
        })
        .collect()
}

fn inked(cell: &Cell) -> usize {
    cell.iter().filter(|on| **on).count()
}

/// The one cell of a single-letter render that carries ink.
fn only_inked(drawn: &[Cell]) -> &Cell {
    let mut with_ink = drawn.iter().filter(|cell| inked(cell) > 0);
    let first = with_ink.next().expect("the letter drew nothing at all");
    assert!(
        with_ink.next().is_none(),
        "a single letter inked more than one cell"
    );
    first
}

#[test]
fn the_letters_of_an_arabic_word_take_three_different_forms() {
    let word: String = [BEH, BEH, BEH].iter().collect();
    let drawn = cells(&word, 3);

    for (column, cell) in drawn.iter().enumerate() {
        assert!(inked(cell) > 0, "cell {column} of the word drew nothing");
    }
    // Any two equal means two letters took the same form, which for a dual-joining letter in
    // initial, medial and final position is the isolated-form bug.
    assert_ne!(drawn[0], drawn[1], "two cells of the word are identical");
    assert_ne!(drawn[1], drawn[2], "two cells of the word are identical");
    assert_ne!(drawn[0], drawn[2], "two cells of the word are identical");
}

#[test]
fn a_letters_glyph_id_depends_on_its_neighbours() {
    // The same claim as the pixel test above, stated where it actually lives. Pixels can
    // differ for reasons that have nothing to do with form -- see the overhang note at the
    // top of this file -- so the form itself is asserted against glyph ids.
    //
    // This one cannot be red before the fix, because the API it calls did not exist. Its red
    // proof is the mutant: forcing `Script::Latin` on the run stops swash running the joining
    // state machine at all, and all three ids collapse to the nominal one.
    let mut fonts = FontStack::system(SIZE).expect("system fonts");
    let mut shaper = Shaper::new();
    let word: String = [BEH, BEH, BEH].iter().collect();
    let starts: Vec<usize> = word.char_indices().map(|(at, _)| at).collect();

    let shaped = shaper
        .shape_joined_run(&mut fonts, &word, &starts, true)
        .expect("three dual-joining letters shape one glyph per cell");
    let ids: Vec<u16> = shaped.iter().map(|placed| placed.glyph.key.glyph).collect();
    let cells: Vec<u16> = shaped.iter().map(|placed| placed.cell).collect();

    assert_eq!(cells, vec![0, 1, 2], "one glyph per cell, in cell order");
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1], "initial and medial are the same glyph");
    assert_ne!(ids[1], ids[2], "medial and final are the same glyph");
    assert_ne!(ids[0], ids[2], "initial and final are the same glyph");

    let nominal = fonts.resolve(BEH).expect("beh resolves").glyph;
    assert!(
        !ids.contains(&nominal),
        "a letter in a word kept the isolated form {nominal}: {ids:?}"
    );
}

#[test]
fn a_collapsing_ligature_is_refused_rather_than_approximated() {
    // Lam-alef is the one genuine exception the plan named: two characters, one glyph, and no
    // honest way to put it on a cell grid. The run shaper returns None so the caller draws
    // what it drew before -- wrong in a known way rather than overlapping or losing a cell.
    let mut fonts = FontStack::system(SIZE).expect("system fonts");
    let mut shaper = Shaper::new();
    let lam_alef = "\u{0644}\u{0627}";
    let starts: Vec<usize> = lam_alef.char_indices().map(|(at, _)| at).collect();
    assert!(
        shaper
            .shape_joined_run(&mut fonts, lam_alef, &starts, true)
            .is_none(),
        "lam-alef collapsed into one cell instead of being refused"
    );

    // The control: the same lam beside a letter it does NOT ligate with must still shape.
    let lam_beh: String = ['\u{0644}', BEH].iter().collect();
    let starts: Vec<usize> = lam_beh.char_indices().map(|(at, _)| at).collect();
    assert!(
        shaper
            .shape_joined_run(&mut fonts, &lam_beh, &starts, true)
            .is_some(),
        "the refusal is unconditional, so it proves nothing about ligatures"
    );
}

#[test]
fn a_run_of_one_letter_is_not_shaped_at_all() {
    // A letter with no neighbours has no contextual form to take, so the run path refuses and
    // the ordinary per-cell path draws the nominal glyph. This is what makes
    // `an_arabic_letter_alone_is_still_its_isolated_form` true by construction.
    let mut fonts = FontStack::system(SIZE).expect("system fonts");
    let mut shaper = Shaper::new();
    let alone = BEH.to_string();
    assert!(
        shaper
            .shape_joined_run(&mut fonts, &alone, &[0], true)
            .is_none()
    );
}

#[test]
fn an_arabic_letter_alone_is_still_its_isolated_form() {
    // The other direction, and the reason joining cannot be faked by always substituting: a
    // lone letter has no neighbours, so it keeps the nominal glyph. Two renders of one letter
    // in rows of different widths must agree, which a run-shaper that treated a run of one as
    // an initial form would break.
    let narrow = cells(&BEH.to_string(), 1);
    let wide = cells(&BEH.to_string(), 4);
    assert_eq!(only_inked(&narrow), only_inked(&wide));
}

#[test]
fn a_space_breaks_the_join() {
    // Joining is a property of adjacent letters, not of the row. Two letters either side of a
    // space are each alone, so both draw the isolated form -- a segmenter that spanned the
    // whole row would join them through the gap.
    let separated: String = [BEH, ' ', BEH].iter().collect();
    let drawn = cells(&separated, 3);
    let alone = cells(&BEH.to_string(), 3);
    let alone = only_inked(&alone);

    // Only the letters' own cells are compared. The space between them is NOT asserted empty:
    // it holds the right-hand overhang of its neighbour, which is the pre-existing
    // proportional-face problem described at the top of this file and has nothing to do with
    // joining.
    assert_eq!(&drawn[0], alone, "a letter beside a space is not isolated");
    assert_eq!(&drawn[2], alone, "a letter beside a space is not isolated");
}

#[test]
fn latin_is_untouched_by_the_joining_path() {
    // The guard that keeps this change away from the 99% of cells that are code. Latin is not
    // a joining script, so every cell takes the path it always did.
    let drawn = cells("abc", 3);
    let a = cells("a", 3);
    assert_eq!(&drawn[0], only_inked(&a));
}
