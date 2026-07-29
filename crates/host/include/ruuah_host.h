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
} RuuahHostFrame;

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

/* Resizes the pty, the terminal and the render target. Refused (with no state change)
 * when the geometry exceeds the frame channel's capacity or either dimension is 0. */
RuuahHostResult ruuah_host_resize(RuuahHost *host, uint16_t cols, uint16_t rows);

/* Tears down the child, the pump thread and the renderer. NULL is a no-op. Any pixels
 * pointer previously returned for this handle is dead after this call. */
void ruuah_host_free(RuuahHost *host);

#ifdef __cplusplus
}
#endif

#endif /* RUUAH_HOST_H */
