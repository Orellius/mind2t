/* mind2t-vt: a terminal core as a C library.
 *
 * This header is OURS. Until 2026-08-08 the only declaration of this library's surface was
 * `vendor/include/ghostty/vt/*.h`, produced by building Ghostty - so anyone linking this archive
 * read another project's documentation to use our code.
 *
 * The types below are ABI-IDENTICAL to that surface, deliberately and permanently: the engine's
 * correctness signal is a differential corpus run against the real libghostty-vt, and being able
 * to stand in behind that ABI is the claim the corpus earns. Identical layout is the point; the
 * names here are ours because the code is.
 *
 * Layout is not asserted by eye. `crates/abi/tests/header_parity.rs` generates C static assertions
 * from Rust's own `size_of` and `offset_of` and compiles this header against them, so a field
 * added on one side and forgotten on the other fails the build rather than corrupting a caller.
 *
 * Both symbol sets are exported by libmind2t-vt.a: `mind2t_vt_*` (declared here) and `ghostty_*`
 * (declared by the vendored headers, kept for drop-in compatibility). They are the same functions.
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

#ifndef MIND2T_VT_H
#define MIND2T_VT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- results ---- */

typedef int32_t mind2t_vt_result;

#define MIND2T_VT_SUCCESS 0
#define MIND2T_VT_OUT_OF_MEMORY -1
#define MIND2T_VT_INVALID_VALUE -2

/* ---- opaque handles and packed values ---- */

/* A terminal. Created by mind2t_vt_terminal_new, released by mind2t_vt_terminal_free. */
typedef void *mind2t_vt_terminal;

/* Accepted and ignored: this implementation owns its allocation. Present so the signature
 * matches the ABI it stands in for. */
typedef void mind2t_vt_allocator;

/* A cell and a row are PACKED VALUES, not pointers. Read them with mind2t_vt_cell_get and
 * mind2t_vt_row_get rather than by masking bits yourself - the packing is not part of the
 * contract and has changed. */
typedef uint64_t mind2t_vt_cell;
typedef uint64_t mind2t_vt_row;

/* ---- selectors ---- */

typedef int32_t mind2t_vt_terminal_data;
typedef int32_t mind2t_vt_terminal_screen;
typedef uint16_t mind2t_vt_mode;
typedef int32_t mind2t_vt_point_tag;
typedef int32_t mind2t_vt_cell_data;
typedef int32_t mind2t_vt_row_data;
typedef int32_t mind2t_vt_cell_content_tag;
typedef int32_t mind2t_vt_cell_wide;
typedef int32_t mind2t_vt_cell_semantic_content;
typedef int32_t mind2t_vt_row_semantic_prompt;
typedef int32_t mind2t_vt_style_color_tag;
typedef int32_t mind2t_vt_sgr_underline;

/* ---- structures ---- */

typedef struct {
  uint8_t r;
  uint8_t g;
  uint8_t b;
} mind2t_vt_color_rgb;

typedef union {
  uint8_t palette;
  mind2t_vt_color_rgb rgb;
  uint64_t _padding;
} mind2t_vt_style_color_value;

typedef struct {
  mind2t_vt_style_color_tag tag;
  mind2t_vt_style_color_value value;
} mind2t_vt_style_color;

/* `size` leads and MUST be set to sizeof(mind2t_vt_style) before any call that fills one.
 * A zero there claims to have been compiled against a zero-byte struct. */
typedef struct {
  size_t size;
  mind2t_vt_style_color fg_color;
  mind2t_vt_style_color bg_color;
  mind2t_vt_style_color underline_color;
  bool bold;
  bool italic;
  bool faint;
  bool blink;
  bool inverse;
  bool invisible;
  bool strikethrough;
  bool overline;
  int32_t underline;
} mind2t_vt_style;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  size_t max_scrollback;
} mind2t_vt_terminal_options;

typedef struct {
  uint16_t x;
  uint32_t y;
} mind2t_vt_point_coordinate;

typedef union {
  mind2t_vt_point_coordinate coordinate;
  uint64_t _padding[2];
} mind2t_vt_point_value;

typedef struct {
  mind2t_vt_point_tag tag;
  mind2t_vt_point_value value;
} mind2t_vt_point;

typedef struct {
  const uint8_t *ptr;
  size_t len;
} mind2t_vt_string;

/* `size` leads, same rule as mind2t_vt_style. A grid reference is INVALIDATED by the next call
 * that mutates the terminal - it is a borrow, not a handle. */
typedef struct {
  size_t size;
  void *node;
  uint16_t x;
  uint16_t y;
} mind2t_vt_grid_ref;

/* ---- functions ---- */

/* Creates a terminal into *out. On failure *out is set to NULL rather than left alone. */
mind2t_vt_result mind2t_vt_terminal_new(const mind2t_vt_allocator *allocator,
                                        mind2t_vt_terminal *out,
                                        mind2t_vt_terminal_options options);

/* Releases a terminal. NULL is accepted. */
void mind2t_vt_terminal_free(mind2t_vt_terminal handle);

/* Feeds bytes to the parser. Resumable: a sequence may be split across calls. */
void mind2t_vt_terminal_vt_write(mind2t_vt_terminal handle, const uint8_t *bytes, size_t len);

/* Resizes, reflowing soft-wrapped lines and mapping the cursor through the transform.
 * The pixel arguments are accepted and ignored; this core has no pixels. */
mind2t_vt_result mind2t_vt_terminal_resize(mind2t_vt_terminal handle, uint16_t cols, uint16_t rows,
                                           uint32_t cell_width_px, uint32_t cell_height_px);

/* Reads one terminal-level datum. `out` must point at storage of the type `data` selects. */
mind2t_vt_result mind2t_vt_terminal_get(mind2t_vt_terminal handle, mind2t_vt_terminal_data data,
                                        void *out);

/* Reads one TRACKED mode. A mode this core does not track answers MIND2T_VT_INVALID_VALUE
 * rather than a guessed false, so a caller can tell "off" from "unknown". */
mind2t_vt_result mind2t_vt_terminal_mode_get(mind2t_vt_terminal handle, mind2t_vt_mode mode,
                                             bool *out_value);

/* Takes a reference to one grid position. Set out->size before calling. */
mind2t_vt_result mind2t_vt_terminal_grid_ref(mind2t_vt_terminal handle, mind2t_vt_point point,
                                             mind2t_vt_grid_ref *out);

/* The four readers below accept a NULL `out`: the reference is still validated, and only the
 * write is skipped. An out-of-bounds point or a dead reference remains an error. */
mind2t_vt_result mind2t_vt_grid_ref_cell(const mind2t_vt_grid_ref *grid_ref, mind2t_vt_cell *out);
mind2t_vt_result mind2t_vt_grid_ref_row(const mind2t_vt_grid_ref *grid_ref, mind2t_vt_row *out);
mind2t_vt_result mind2t_vt_grid_ref_graphemes(const mind2t_vt_grid_ref *grid_ref, uint32_t *buf,
                                              size_t buf_len, size_t *out_len);
mind2t_vt_result mind2t_vt_grid_ref_style(const mind2t_vt_grid_ref *grid_ref, mind2t_vt_style *out);

/* Unpack a cell or a row. */
mind2t_vt_result mind2t_vt_cell_get(mind2t_vt_cell cell, mind2t_vt_cell_data data, void *out);
mind2t_vt_result mind2t_vt_row_get(mind2t_vt_row row, mind2t_vt_row_data data, void *out);

/* Fills *out with the default style. Set out->size before calling. */
void mind2t_vt_style_default(mind2t_vt_style *out);

#ifdef __cplusplus
}
#endif

#endif /* MIND2T_VT_H */
