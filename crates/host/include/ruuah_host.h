/* ruuah_host.h -- the embedder surface of libruuah-vt-host.a.
 *
 * This is NOT part of the ghostty ABI mirror. The 13 `ghostty_*` entry points ride in the
 * same archive unchanged; this header is the small surface a GUI host needs on top of them:
 * spawn a shell on a pty, poll rendered pixels, send input bytes, resize. One handle wraps
 * the whole Rust pipeline (pty host -> frame channel -> GPU renderer).
 *
 * Threading contract: a RuuahHost is NOT thread-safe. Every call on one handle must come
 * from the same thread (in practice: the UI thread that polls and forwards key input).
 */

#ifndef RUUAH_HOST_H
#define RUUAH_HOST_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque. Create with ruuah_host_spawn, destroy with ruuah_host_free. */
typedef struct RuuahHost RuuahHost;

/* Opaque. Create with ruuah_config_load, destroy with ruuah_config_free. */
typedef struct RuuahConfig RuuahConfig;

typedef enum {
  RUUAH_HOST_SUCCESS = 0,
  /* A NULL pointer, a zero geometry, or an otherwise malformed argument. */
  RUUAH_HOST_INVALID_VALUE = 1,
  /* Opening the pty or starting the child failed. */
  RUUAH_HOST_SPAWN_FAILED = 2,
  /* The requested geometry exceeds the frame channel's fixed capacity. */
  RUUAH_HOST_RESIZE_REFUSED = 3,
  /* The renderer could not be built or could not draw (e.g. no GPU adapter, no fonts). */
  RUUAH_HOST_RENDER_FAILED = 4,
  /* Writing to the child failed (typically: the child is gone). */
  RUUAH_HOST_SEND_FAILED = 5,
} RuuahHostResult;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  /* Font size in pixels. 0 means the default (16). */
  float font_size;
  /* Shell command line, run via `/bin/sh -c`. NULL means an interactive $SHELL. */
  const char *command;
  /* When true, each row's base direction is detected from its own text (Hebrew-first
   * rows flow right-to-left; rows that resolve LTR are laid out exactly as with false).
   * False keeps the terminal default: left-to-right base, RTL runs reordered in place. */
  bool auto_direction;
  /* Contributes ONLY the theme palette; NULL keeps the built-in scheme. Read during the
   * spawn call and not retained -- freeing the config afterwards is legal. The scalar
   * settings are read through the ruuah_config_* getters instead, because the embedder
   * owns their precedence (CLI flags, Retina scaling). */
  const RuuahConfig *config;
} RuuahHostOptions;

/* One rendered frame, filled by ruuah_host_poll. */
typedef struct {
  /* RGBA8, row-major, width*height*4 bytes. Borrowed: valid until the next poll, resize,
   * or free on the same handle. Never NULL after the first poll that drew. */
  const uint8_t *pixels;
  uint32_t width;
  uint32_t height;
  /* The frame channel generation the pixels were drawn from. */
  uint64_t generation;
  /* True when this poll drew new content. False means pixels/width/height still describe
   * the previous draw (or nothing yet, if no poll has drawn -- then pixels is NULL). */
  bool drew;
  /* True once the child has exited. Frames already published remain readable. */
  bool child_exited;
  /* The background the grid currently shows at its edge, RGBA -- the top-left cell's
   * resolved style, falling back to the renderer default before any frame. Painting
   * window margins with it makes the terminal read as continuing into the frame, and
   * it follows program backgrounds (vim themes, BCE clears) and future palette themes
   * alike. Never pixel-sampled: the corner pixel is the caret when the cursor is home. */
  uint8_t background[4];
  /* One byte per grid row (row_count of them): the row's shell-semantic class per
   * OSC 133 -- RUUAH_ROW_OUTPUT, RUUAH_ROW_PROMPT or RUUAH_ROW_INPUT. What a block
   * gutter draws from. Borrowed with the same lifetime as pixels: valid until the next
   * poll, resize, or free. NULL before the first drawn frame. */
  const uint8_t *row_semantics;
  uint32_t row_count;
} RuuahHostFrame;

/* RuuahHostFrame.row_semantics values, and ruuah_host_row_text filters. */
#define RUUAH_ROW_OUTPUT 0
#define RUUAH_ROW_PROMPT 1
#define RUUAH_ROW_INPUT 2
/* ruuah_host_row_text only: take every cell regardless of its OSC 133 mark. */
#define RUUAH_TEXT_ALL 255

/* Spawns the command on a fresh pty and starts the parse/publish pipeline.
 *
 * options and out must be non-NULL. On success writes the new handle to *out and returns
 * RUUAH_HOST_SUCCESS; on any failure writes NULL to *out. */
RuuahHostResult ruuah_host_spawn(const RuuahHostOptions *options, RuuahHost **out);

/* Reads the latest published frame and, if it is new, draws it.
 *
 * host and out must be non-NULL and host must be live. Cheap when nothing changed. */
RuuahHostResult ruuah_host_poll(RuuahHost *host, RuuahHostFrame *out);

/* Writes bytes to the child's input. This is the Host::send seam: key encodings from the
 * GUI and (in slice 9) DSR/DA replies both travel through it.
 *
 * bytes may be NULL only when len is 0. */
RuuahHostResult ruuah_host_send(RuuahHost *host, const uint8_t *bytes, size_t len);

/* Encodes clipboard bytes for the child and writes them to the pty. Pass raw clipboard
 * bytes: unsafe control bytes become spaces (xterm's strip set), and the data is wrapped
 * in the bracketed-paste fenceposts when the child enabled mode 2004, or has newlines
 * folded to carriage returns when it did not. The mode rides the last polled frame, so
 * poll at least once after the child enables it -- a rendering host does continuously.
 *
 * bytes may be NULL only when len is 0. */
RuuahHostResult ruuah_host_paste(RuuahHost *host, const uint8_t *bytes, size_t len);

/* Resizes the pty, the terminal and the render target. Refused (with no state change)
 * when the geometry exceeds the frame channel's capacity or either dimension is 0. */
RuuahHostResult ruuah_host_resize(RuuahHost *host, uint16_t cols, uint16_t rows);

/* Copies one grid row's text as UTF-8 into out, trailing blanks trimmed. `semantic`
 * filters by the per-cell OSC 133 mark: RUUAH_TEXT_ALL takes every cell, the RUUAH_ROW_*
 * values take only cells wearing that mark -- the input filter on a prompt row is what
 * makes "copy command" return `ls -la` out of `$ ls -la`. Reads the last POLLED frame --
 * poll at least once first. Writes at most cap bytes (no NUL added; a truncated copy
 * backs off to a UTF-8 boundary), stores the row's full byte length in *len, and fails
 * with INVALID_VALUE when the row is out of range or nothing has been polled. Size cap
 * from a first call's *len, then call again. The copy-command / copy-output seam for
 * blocks: group rows with row_semantics, then read the text. */
RuuahHostResult ruuah_host_row_text(
    RuuahHost *host, uint16_t row, uint8_t semantic, uint8_t *out, size_t cap, size_t *len);

/* Tears down the child, the pump thread and the renderer. NULL is a no-op. Any pixels
 * pointer previously returned for this handle is dead after this call. */
void ruuah_host_free(RuuahHost *host);

/* -- Settings (S1): dir/config.toml plus dir/themes/<name>.toml. --------------------------
 *
 * config.toml keys (all optional; unknown keys are an error, not ignored):
 *   font-size = 16.0          logical pixels; the embedder applies scale and defaults
 *   auto-direction = true     per-row Hebrew-first layout
 *   shell = "/bin/bash"       command line for new sessions, run via /bin/sh -c
 *   theme = "name"            themes/name.toml
 *
 * themes/<name>.toml keys (all optional):
 *   foreground = "#rrggbb"
 *   background = "#rrggbb"
 *   palette = ["#rrggbb", x16]   the named system colors; the cube/ramp stay absolute
 *
 * Loading never fails into an unusable state: a missing file is the defaults, and a file
 * that could not be honoured is the defaults plus ruuah_config_error -- which a GUI must
 * show loudly. A bad theme applies NOTHING (never a half-theme). */

/* Loads dir/config.toml into a new handle. dir NULL means ~/.ruuah. Fails only on a NULL
 * out-param or a non-UTF-8 dir; on failure writes NULL to *out. */
RuuahHostResult ruuah_config_load(const char *dir, RuuahConfig **out);

/* Font size in logical pixels, 0 when the config does not set one. */
float ruuah_config_font_size(const RuuahConfig *config);

/* The configured auto-direction, or `fallback` when the config does not say. */
bool ruuah_config_auto_direction(const RuuahConfig *config, bool fallback);

/* The configured shell command line, or NULL when unset. Borrowed: valid until
 * ruuah_config_free on the same handle. */
const char *ruuah_config_shell(const RuuahConfig *config);

/* Everything that went wrong while loading, newline-joined; NULL when clean. Borrowed:
 * valid until ruuah_config_free on the same handle. */
const char *ruuah_config_error(const RuuahConfig *config);

/* Frees a config handle. NULL is a no-op. Strings lent by the getters die here. */
void ruuah_config_free(RuuahConfig *config);

#ifdef __cplusplus
}
#endif

#endif /* RUUAH_HOST_H */
