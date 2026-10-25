/* Wire format of docs/WEBVIEW-PROTOCOL.md: message types and little-endian readers and writers. */
#ifndef IYAKKI_PROTO_H
#define IYAKKI_PROTO_H

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <glib.h>

enum { T_HELLO = 1, T_NAVIGATE, T_RELOAD, T_STOP, T_BACK, T_FORWARD, T_RESIZE, T_MOUSE, T_WHEEL, T_KEY, T_TEXT, T_ZOOM, T_FOCUS, T_VISIBLE, T_ACK, T_QUIT, T_SCRIPT, T_LEAVE_FULLSCREEN };
enum { F_READY = 0x81, F_FRAME, F_TITLE, F_URL, F_LOADING, F_HISTORY, F_CURSOR, F_DOWNLOAD, F_CRASHED, F_LOG, F_CLIPBOARD, F_SCRIPT_RESULT, F_FULLSCREEN, F_AUDIO_PLAYING, F_MEDIA_UNSUPPORTED };
enum { CUR_DEFAULT = 0, CUR_POINTER = 1, CUR_TEXT = 2 };

typedef struct { uint8_t *p; size_t len, cap; } Buf;

static inline void buf_put(Buf *b, const void *d, size_t n) {
    if (b->len + n > b->cap) { b->cap = (b->len + n) * 2 + 64; b->p = realloc(b->p, b->cap); if (!b->p) abort(); }
    memcpy(b->p + b->len, d, n);
    b->len += n;
}
static inline void buf_u8(Buf *b, uint8_t v) { buf_put(b, &v, 1); }
static inline void buf_u32(Buf *b, uint32_t v) { buf_put(b, &v, 4); }      /* little-endian hosts only (x86, arm64) */
static inline void buf_str(Buf *b, const char *s) {
    size_t n = s ? strlen(s) : 0;
    if (n > (1u << 20)) n = 1u << 20;
    buf_u32(b, (uint32_t)n);
    buf_put(b, s, n);
}
static inline void buf_free(Buf *b) { free(b->p); b->p = NULL; b->len = b->cap = 0; }

typedef struct { const uint8_t *p; size_t n, o; int bad; } Rd;

static inline int rd_take(Rd *r, void *out, size_t n) {
    if (r->o + n > r->n) { r->bad = 1; memset(out, 0, n); return 0; }
    memcpy(out, r->p + r->o, n);
    r->o += n;
    return 1;
}
static inline uint8_t rd_u8(Rd *r) { uint8_t v; rd_take(r, &v, 1); return v; }
static inline uint32_t rd_u32(Rd *r) { uint32_t v; rd_take(r, &v, 4); return v; }
static inline int32_t rd_i32(Rd *r) { int32_t v; rd_take(r, &v, 4); return v; }
/* a newly allocated, NUL-terminated UTF-8 string (invalid bytes replaced); never NULL */
static inline char *rd_str(Rd *r) {
    uint32_t n = rd_u32(r);
    if (n > (1u << 20) || r->o + n > r->n) { r->bad = 1; return g_strdup(""); }
    char *s = g_utf8_make_valid((const char *)r->p + r->o, (gssize)n);
    r->o += n;
    return s;
}

#endif
