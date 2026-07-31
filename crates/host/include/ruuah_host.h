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
  /* The call was valid but the protocol produced nothing: mouse reporting off, motion
   * deduplicated, or a wheel left to the embedder's viewport scroll. Not an error --
   * the embedder's own handling of the event is next. */
  RUUAH_HOST_IGNORED = 6,
} RuuahHostResult;

/* ruuah_host_mouse vocabularies. */
#define RUUAH_MOUSE_PRESS 0u
#define RUUAH_MOUSE_RELEASE 1u
#define RUUAH_MOUSE_MOTION 2u
#define RUUAH_MOUSE_BUTTON_NONE 0u
#define RUUAH_MOUSE_BUTTON_LEFT 1u
#define RUUAH_MOUSE_BUTTON_MIDDLE 2u
#define RUUAH_MOUSE_BUTTON_RIGHT 3u
#define RUUAH_MOUSE_MODS_SHIFT 1u
#define RUUAH_MOUSE_MODS_CTRL 2u
#define RUUAH_MOUSE_MODS_ALT 4u

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
  /* Rows the displayed view is scrolled up into history; 0 is the live bottom. Set by
   * the pump after clamping, so it reports where the view actually IS -- draw scroll
   * indicators from this, never from accumulated deltas. */
  uint32_t viewport_offset;
  /* The caret's cell in this frame, and whether it is shown. Ghost-suggestion
   * overlays anchor here; never re-derive the caret from pixels. */
  uint16_t cursor_col;
  uint16_t cursor_row;
  bool cursor_visible;
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

/* Sets the view geometry mouse encoding converts through: surface size and content
 * insets, in the same backing-pixel space the frame's pixels use. Call after layout
 * changes (resize, zoom, inset change); until the first call, pointer events answer
 * RUUAH_HOST_IGNORED. */
RuuahHostResult ruuah_host_mouse_geometry(RuuahHost *host, uint32_t screen_width,
                                          uint32_t screen_height, uint32_t padding_left,
                                          uint32_t padding_top, uint32_t padding_right,
                                          uint32_t padding_bottom);

/* Feeds one pointer event to mouse reporting. action/button/mods use the RUUAH_MOUSE_*
 * constants (buttons 4..9 are the protocol's wheel/aux codes); x/y are surface pixels
 * from the view's top-left. Success means a report was written to the pty; IGNORED
 * means the protocol produced nothing and the event is the embedder's again
 * (selection, menus). Send press/release pairs even while reporting is off -- held-
 * button bookkeeping happens on every call. Modes ride the last polled frame. */
RuuahHostResult ruuah_host_mouse(RuuahHost *host, uint32_t action, uint32_t button,
                                 uint32_t mods, float x, float y);

/* Feeds one keyboard event to the key encoder and writes the result to the pty.
 * action: 0 release, 1 press, 2 repeat. key: the GhosttyKey C enum value
 * (KeyMap.swift). mods/consumed_mods: GhosttyMods bits (shift 1, ctrl 2, alt 4,
 * super 8, caps 16, num 32); consumed = mods spent producing text (macOS: mods
 * minus control and command). text: the event's translated UTF-8 (NULL/0 = none);
 * unshifted_codepoint: the key with no modifiers, 0 if none. Encoding modes --
 * DECCKM, keypad, 1035/1036, modifyOtherKeys, the kitty flags -- ride the last
 * polled frame. Success = bytes written; IGNORED = the event encodes to nothing. */
RuuahHostResult ruuah_host_key(RuuahHost *host, uint32_t action, uint32_t key,
                               uint32_t mods, uint32_t consumed_mods,
                               const uint8_t *text, size_t text_len,
                               uint32_t unshifted_codepoint);

/* Routes a wheel gesture through the terminal's precedence: active mouse mode ->
 * wheel-button reports; else alternate screen + alternate scroll (1007, default on)
 * -> arrow keys (ESC O form under DECCKM); else IGNORED, and the embedder scrolls its
 * viewport. ticks is whole notches, positive up; the embedder owns fractional
 * banking. Success includes a consumed wheel that encoded to nothing (X10 event mode
 * cannot name wheel buttons) -- a program holding the mouse must not also have the
 * view scrolled under it. */
RuuahHostResult ruuah_host_wheel(RuuahHost *host, float x, float y, int32_t ticks,
                                 uint32_t mods);

/* Workflow templates (~/.ruuah/workflows/*.toml, the cmd+K palette's data). All
 * string getters use the row_text buffer protocol: NULL out sizes the value into
 * out_len, a short buffer refuses with the needed length, no NUL is written.
 * Field selectors: 0 name, 1 description, 2 command (or 2 default for args). */
typedef struct RuuahWorkflows RuuahWorkflows;

#define RUUAH_WORKFLOW_NAME 0u
#define RUUAH_WORKFLOW_DESCRIPTION 1u
#define RUUAH_WORKFLOW_COMMAND 2u
#define RUUAH_WORKFLOW_ARG_DEFAULT 2u

/* dir NULL = ~/.ruuah/workflows. Broken files are skipped, their errors kept on the
 * handle; a missing directory is a valid empty handle. */
RuuahHostResult ruuah_workflows_load(const char *dir, RuuahWorkflows **out);
void ruuah_workflows_free(RuuahWorkflows *handle);
uint32_t ruuah_workflows_count(const RuuahWorkflows *handle);
/* Loader error lines, newline-joined; empty when every file parsed. Show loudly. */
RuuahHostResult ruuah_workflows_errors(const RuuahWorkflows *handle, uint8_t *out,
                                       size_t cap, size_t *out_len);
RuuahHostResult ruuah_workflow_field(const RuuahWorkflows *handle, uint32_t index,
                                     uint32_t field, uint8_t *out, size_t cap,
                                     size_t *out_len);
uint32_t ruuah_workflow_arg_count(const RuuahWorkflows *handle, uint32_t index);
/* A missing default answers RUUAH_HOST_IGNORED with length 0 -- distinct from an
 * empty-string default, so the palette prefills one and prompts bare for the other. */
RuuahHostResult ruuah_workflow_arg(const RuuahWorkflows *handle, uint32_t index,
                                   uint32_t arg_index, uint32_t field, uint8_t *out,
                                   size_t cap, size_t *out_len);
/* args_blob: pairs of NUL-terminated strings (name, value, ...), blob_len total
 * bytes. An unresolved placeholder refuses with INVALID_VALUE -- a command with a
 * hole in it must never reach the paste path. */
RuuahHostResult ruuah_workflow_render(const RuuahWorkflows *handle, uint32_t index,
                                      const uint8_t *args_blob, size_t blob_len,
                                      uint8_t *out, size_t cap, size_t *out_len);

/* Command history for S4's ghost suggestions. path NULL = ~/.ruuah/history. Append
 * records one EXECUTED command and persists (blank/multiline/consecutive-duplicate
 * are dropped, answering IGNORED); suggest returns the most recent entry input is a
 * PROPER prefix of via the buffer protocol, IGNORED with length 0 when none. */
typedef struct RuuahHistory RuuahHistory;
RuuahHostResult ruuah_history_load(const char *path, RuuahHistory **out);
void ruuah_history_free(RuuahHistory *handle);
/* `cwd` is the RAW OSC 7 report (event kind 7), or NULL. The host normalizes it --
 * percent-escapes and the file:// host -- so an embedder passes the bytes through
 * untouched. A command is recorded against the directory it ran in, and a suggestion
 * PREFERS a match made in the current one, falling back to the newest match anywhere. */
RuuahHostResult ruuah_history_append(RuuahHistory *handle, const uint8_t *command,
                                     size_t len, const uint8_t *cwd, size_t cwd_len);
RuuahHostResult ruuah_history_suggest(const RuuahHistory *handle, const uint8_t *input,
                                      size_t len, const uint8_t *cwd, size_t cwd_len,
                                      uint8_t *out, size_t cap, size_t *out_len);

/* Scrolls the displayed view through scrollback: positive rows climbs into history,
 * negative returns toward the live bottom, INT32_MIN snaps straight to it. Deltas
 * accumulate on the pump thread and are clamped against what history actually holds;
 * the landed position comes back in the next polled frame's viewport_offset. Typing
 * does not snap the view -- apply that policy in the embedder, via INT32_MIN. */
RuuahHostResult ruuah_host_scroll(RuuahHost *host, int32_t rows);

/* Resizes the pty, the terminal and the render target. Refused (with no state change)
 * when the geometry exceeds the frame channel's capacity or either dimension is 0. */
RuuahHostResult ruuah_host_resize(RuuahHost *host, uint16_t cols, uint16_t rows);

/* Reports the pixel cell size a renderer would use at font_size, without a host. The
 * zoom flow needs this BEFORE the new renderer exists: window pixels stay fixed, the
 * grid that fits them moves with the metrics. Pure query. */
RuuahHostResult ruuah_host_cell_metrics(
    float font_size, const char *font_family, uint32_t *out_width, uint32_t *out_height);

/* Changes the font size live: pty resize to the new grid plus render-target rebuild at
 * the new metrics, in one call. Derive cols/rows from ruuah_host_cell_metrics and the
 * window's fixed pixel size. Refusal rules match ruuah_host_resize. */
RuuahHostResult ruuah_host_set_font_size(
    RuuahHost *host, float font_size, uint16_t cols, uint16_t rows);

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

/* Copies the OSC 8 URI under one cell (last POLLED frame). No link = SUCCESS with *len
 * 0 -- clicking plain text is not an error. Truncation contract as ruuah_host_row_text. */
RuuahHostResult ruuah_host_link_at(
    RuuahHost *host, uint16_t col, uint16_t row, uint8_t *out, size_t cap, size_t *len);

/* Pops the next host-facing event, oldest first. *kind: 0 = none, 1 = set clipboard to
 * payload, 2 = notification (payload = title, '\n', body), 3 = bell. An event is
 * 4 = title (payload = UTF-8), 5 = progress (payload = state,value), 6 = command
 * started (OSC 133;C, no payload -- read the input cells for the text),
 * 7 = working directory (OSC 7; payload is the RAW report, usually a file:// URI, and
 * is NOT percent-decoded -- an empty payload means the child cleared it). An event is
 * consumed only when cap held its whole payload; a smaller cap reports kind + *len and
 * leaves it queued (size-then-fetch never loses an event). */
RuuahHostResult ruuah_host_next_event(
    RuuahHost *host, uint32_t *kind, uint8_t *out, size_t cap, size_t *len);

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

/* Whether the operator granted screen-inspection replies -- DECRQCRA checksums and
 * the WINOPS 18 size report (config `reports = true`). FALSE by default and for a
 * NULL handle: these let a program read back what is on screen, the same posture
 * question as OSC 52 clipboard reads. The grant itself travels through
 * RuuahHostOptions.config at spawn; this is for showing the posture. */
bool ruuah_config_reports(const RuuahConfig *config);

/* The configured lead font family (config font-family), or NULL when unset.
 * Borrowed: valid until ruuah_config_free. */
const char *ruuah_config_font_family(const RuuahConfig *config);

/* Everything that went wrong while loading, newline-joined; NULL when clean. Borrowed:
 * valid until ruuah_config_free on the same handle. */
const char *ruuah_config_error(const RuuahConfig *config);

/* Frees a config handle. NULL is a no-op. Strings lent by the getters die here. */
void ruuah_config_free(RuuahConfig *config);

#ifdef __cplusplus
}
#endif

#endif /* RUUAH_HOST_H */
