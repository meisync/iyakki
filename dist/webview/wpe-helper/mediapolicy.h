/* The media policy message (0x18, F-BROX-18 and F-BROX-20), pure parts: plain libc, no WebKit and no GLib, so policy-check.c compiles and runs them
 * anywhere. The helper (helper.c) uses them to tell the page its ceiling and to keep the add-on's decoders out of the engine's choice when the
 * tenant or the person switched them off. */
#ifndef IYAKKI_MEDIAPOLICY_H
#define IYAKKI_MEDIAPOLICY_H

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MP_MAX_HEIGHT 8640u

/* the tallest video a page is offered: 0 is no ceiling, anything above 8640 is no ceiling either (a typo is not a limit nobody can see) */
static inline uint32_t mp_height(uint32_t h) { return h > MP_MAX_HEIGHT ? 0 : h; }

/* the line put before media-cap.js and run in the live page; always a complete statement, only digits in it */
static inline void mp_prefix(char *out, size_t cap, uint32_t max_height, int codecs) {
    snprintf(out, cap, "window.__iyakkiCap={h:%u,c:%d};\n", mp_height(max_height), codecs ? 1 : 0);
}

/* The decoders the audio and video add-on brings (the LGPL bridge plug-in's). Rank 0 keeps them from being chosen. */
#define MP_ADDON_RANK_OFF "avdec_h264:0,avdec_h265:0,avdec_aac:0,avdec_aac_latm:0,avdec_ac3:0,avdec_eac3:0,avdec_mpeg4:0"

/* GST_PLUGIN_FEATURE_RANK for web processes that start after this: the value the helper started with, plus the rank-off list when the
 * decoders are off. base may be NULL. Returns 0 when the result fits, -1 when it was cut (then nothing is changed by the caller). */
static inline int mp_rank(char *out, size_t cap, const char *base, int codecs) {
    int n = snprintf(out, cap, "%s%s%s", base ? base : "", base && *base && !codecs ? "," : "", codecs ? "" : MP_ADDON_RANK_OFF);
    return n < 0 || (size_t)n >= cap ? -1 : 0;
}

#endif
