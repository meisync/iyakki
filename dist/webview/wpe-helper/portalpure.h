/* The names the in-house ScreenCast portal (screenportal.c) builds, pure parts: plain libc, so portal-check.c compiles and runs them anywhere.
 * The portal's request and session objects live at paths made from the caller's unique bus name and a token the caller chose; the engine
 * (WebKit's DesktopPortal) waits for a `Response` signal on the request path it computes itself, so the strings must be exactly those of
 * the portal specification: the sender with its leading colon removed and every dot turned into an underscore. */
#ifndef IYAKKI_PORTALPURE_H
#define IYAKKI_PORTALPURE_H

#include <stdio.h>
#include <string.h>

#define PP_PREFIX "/org/freedesktop/portal/desktop"

/* a token is 1 to 64 characters of letters, digits and underscores (anything else would break the object path) */
static inline int pp_token_ok(const char *t) {
    size_t n = t ? strlen(t) : 0;
    if (n == 0 || n > 64) return 0;
    for (size_t i = 0; i < n; i++) {
        char c = t[i];
        if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_')) return 0;
    }
    return 1;
}

/* ":1.42" -> "1_42"; returns 0 when the name is not a unique bus name that fits */
static inline int pp_sender_part(const char *sender, char *out, size_t cap) {
    if (!sender || sender[0] != ':' || !sender[1] || strlen(sender) + 1 > cap) return 0;
    size_t j = 0;
    for (const char *p = sender + 1; *p; p++) {
        char c = *p == '.' ? '_' : *p;
        if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_')) return 0;
        out[j++] = c;
    }
    out[j] = 0;
    return 1;
}

/* kind is "request" or "session"; returns 0 and an empty path when the sender or token is not usable */
static inline int pp_path(const char *kind, const char *sender, const char *token, char *out, size_t cap) {
    char s[128];
    out[0] = 0;
    if (!pp_token_ok(token) || !pp_sender_part(sender, s, sizeof s)) return 0;
    int n = snprintf(out, cap, PP_PREFIX "/%s/%s/%s", kind, s, token);
    if (n < 0 || (size_t)n >= cap) { out[0] = 0; return 0; }
    return 1;
}

/* The portal answers Start only while the shell has handed over a screen source: the person's allow is the shell's, never the page's. */
static inline int pp_may_start(const char *feed_path) { return feed_path && feed_path[0]; }

#endif
