/* The screen feed file, reader side (F-BROW-71). The shell writes its own picture into a private shared file about four times a second
 * (crates/iyakki-apps/src/apps/browser_media.rs `ScreenFeed`): `"IYSC"`, u32 version 1, u32 width, u32 height, u32 stride, u32 format (0 =
 * RGBA), u64 seq, all little-endian, 32 bytes, then width*height*4 bytes. seq is odd while a picture is written and even when it is
 * complete. Plain libc, no GLib and no GStreamer, so feed-check.c compiles and runs it anywhere and a Rust test writes the file with the
 * real writer. */
#ifndef IYAKKI_SCREENFEED_H
#define IYAKKI_SCREENFEED_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define SF_HEADER 32u
#define SF_MAX_DIM 8192u
#define SF_MAX_BYTES (SF_MAX_DIM * SF_MAX_DIM * 4ull / 2)

typedef struct { uint32_t w, h; uint64_t seq; uint8_t *rgba; } SfFrame;

static inline uint32_t sf_u32(const uint8_t *p) { uint32_t v; memcpy(&v, p, 4); return v; }
static inline uint64_t sf_u64(const uint8_t *p) { uint64_t v; memcpy(&v, p, 8); return v; }

/* Read the picture in the feed file, when there is a new complete one. Returns 1 and fills *f (rgba malloc'ed, the caller frees) when the
 * picture is complete (the same even seq before and after the copy) and its seq is not `after`; 0 when there is nothing new or nothing
 * whole (not a feed file, wrong version or format, a size out of range, a half-written picture); never reads outside the file. */
static inline int sf_read(const char *path, uint64_t after, SfFrame *f) {
    memset(f, 0, sizeof *f);
    FILE *fp = fopen(path, "rb");
    if (!fp) return 0;
    uint8_t head[SF_HEADER], again[8];
    int ok = 0;
    if (fread(head, 1, SF_HEADER, fp) != SF_HEADER || memcmp(head, "IYSC", 4) || sf_u32(head + 4) != 1 || sf_u32(head + 20) != 0) goto out;
    uint32_t w = sf_u32(head + 8), h = sf_u32(head + 12), stride = sf_u32(head + 16);
    uint64_t seq = sf_u64(head + 24);
    if ((seq & 1) || seq == after || !w || !h || w > SF_MAX_DIM || h > SF_MAX_DIM || stride != w * 4 || (uint64_t)w * h * 4 > SF_MAX_BYTES) goto out;
    size_t n = (size_t)w * h * 4;
    uint8_t *px = malloc(n);
    if (!px) goto out;
    if (fread(px, 1, n, fp) != n) { free(px); goto out; }
    /* a writer that started the next picture meanwhile changed seq: the copy may be torn, so it is dropped */
    if (fseek(fp, 24, SEEK_SET) != 0 || fread(again, 1, 8, fp) != 8 || sf_u64(again) != seq) { free(px); goto out; }
    f->w = w; f->h = h; f->seq = seq; f->rgba = px;
    ok = 1;
out:
    fclose(fp);
    return ok;
}

#endif
