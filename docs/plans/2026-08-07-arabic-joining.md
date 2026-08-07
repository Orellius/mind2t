# Arabic joining: the diagnosis, and the fix

Written 2026-08-07. Not implemented.

## The claim in the README was the wrong explanation

The README said Arabic "does not join across cells, which is a property of terminal grids rather
than of this implementation, and every terminal shares it."

The first half of that is a real constraint. **The second half is wrong for this codebase**, and
it hid a tractable bug behind a law of physics.

## What actually happens

`renderer.rs:490`:

```rust
let placed = if self.shaping && needs_shaping(cluster) {
    self.shaper.shape(&mut self.fonts, cluster)
} else {
    self.shaper.place_at_origin(&mut self.fonts, cluster)
};
```

and `shape.rs`:

```rust
pub fn needs_shaping(cluster: &str) -> bool {
    cluster.chars().nth(1).is_some()
}
```

A lone Arabic letter is **one** codepoint, so `needs_shaping` is false, so it takes
`place_at_origin`, which maps the character through the charmap and draws the nominal glyph.
The nominal glyph for an Arabic letter is its **isolated** form.

So Arabic never reaches the shaper at all. It is not that joining is lost crossing a cell
boundary; it is that nothing ever asks for a joined form. The GSUB lookups that produce
`init`, `medi` and `fina` are never run.

Hebrew works because its unit of shaping and its unit of layout are the same thing: a base plus
its niqqud is one grapheme cluster, one cell, and marks carry zero advance. Arabic's unit of
shaping is the **run**, and this renderer has never shaped one for it.

## Why this is fixable rather than a grid limit

Arabic joining is overwhelmingly a **1:1 substitution**. A run of N characters shapes to N
glyphs; the letters change form, the count does not. One glyph per cell is exactly what a grid
wants, so the correct forms fit the grid perfectly.

The genuine exception is the mandatory **lam-alef** ligature, where two characters collapse to
one glyph. That one really does not fit a cell grid, and it is the only case that needs a
policy rather than an implementation.

## The fix

`shape_run` already exists (`shape.rs:113`) and already does most of this. It was written for
Latin programming ligatures, and two things in it are Latin-specific:

1. `.script(Script::Latin)` is hardcoded. Arabic needs `Script::Arabic`, or the script derived
   from the run, or the shaper will not apply the joining lookups at all.
2. The pen advances by `glyph.advance`, which is proportional. A grid needs the pen to advance
   by one **cell width** per source character.

So the work is:

- **Segment the row into runs by script**, not just the ASCII segments the ligature path uses.
  A contiguous Arabic run is the unit.
- **Shape the run with its own script**, features on, to get the contextual glyphs.
- **Lay out one glyph per cell**, advancing by the cell width rather than the font's advance.
  Shape for FORM, position for GRID.
- **Fall back to per-cell isolated shaping when the glyph count does not equal the character
  count.** That is the lam-alef case and any other collapsing ligature. Degrade honestly to what
  is drawn today rather than silently dropping or overlapping a cell.

## The harness comes first, as always

Nothing in the suite can currently see this. Every existing test either uses Latin, or Hebrew
where isolated and contextual forms are the same thing, so a renderer that draws every Arabic
letter isolated scores perfectly.

The observable to add: **the glyph id chosen for a letter depends on its neighbours.** Shape the
same Arabic letter alone and in the middle of a word, and the middle one must resolve to a
different glyph id. That is a direct, positional assertion with no reference image, and it fails
today by construction because both paths return the nominal glyph.

Then a pixel test in the shape of `tests/shaping.rs`: the ink for a medial letter differs from
the ink for the same letter isolated.

Mutants to run before believing any of it:

- force `Script::Latin` on an Arabic run: the joining lookups stop applying and the glyph-id
  test must go red
- keep the font's own advance instead of the cell width: glyphs must drift off the grid, which
  the positional test catches
- remove the count-mismatch fallback: a lam-alef run must be seen to misplace rather than
  silently pass

## What will still not work afterwards

Cursive **connection across a cell gap** is cosmetic and stays imperfect: the joining strokes of
adjacent letters are drawn at cell boundaries rather than meeting exactly, because each glyph is
positioned in its own cell. The letters will be in their correct joined FORMS, which is the part
that makes Arabic readable; the strokes will not always kiss. That is the real grid limit, and it
is much smaller than the one the README claimed.

## Scope note

This is renderer-only. Nothing here touches the core, the corpus, or the C ABI, and the
differential oracle has no opinion about it: `libghostty-vt` has no bidi or shaping surface at
all, which is why bidi lives in the renderer in the first place.
