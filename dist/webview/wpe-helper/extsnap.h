/* The extension snapshot, C side (the layout is in crates/iyakki-webview/src/ext.rs). Plain libc, no WebKit and no GLib, so it
 * compiles and runs anywhere: ext-check.c tests it against the Rust encoder and matcher. Used by webext.c, the web process extension.
 *
 * A snapshot holds extensions; an extension holds hooks (kind + opaque payload). xs_parse() routes every hook by kind. Only kind 1
 * (request.headers) has code; any other kind, a hook without the capability its kind needs, and a broken payload are skipped and
 * named in the report, never fatal. A snapshot of another layout version is refused whole (nothing is kept). The report never holds
 * a header value. */
#ifndef IYAKKI_EXTSNAP_H
#define IYAKKI_EXTSNAP_H

#include <ctype.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

#define XS_VERSION 1u
#define XS_MAX_SNAPSHOT (512u * 1024u)
#define XS_MAX_ENTRIES 16u
#define XS_MAX_CAPS 16u
#define XS_MAX_HOOKS 16u
#define XS_MAX_RULES 50u
#define XS_MAX_PATTERNS 50u
#define XS_MAX_SEGMENTS 32u
#define XS_MAX_STR 4096u
#define XS_MAX_PER_REQUEST 16

enum { XS_HOOK_REQUEST_HEADERS = 1, XS_HOOK_RESPONSE_HEADERS, XS_HOOK_REQUEST_BLOCK, XS_HOOK_CONTENT_SCRIPT, XS_HOOK_PAGE_ACTION };

typedef struct { uint8_t scheme, suffix; char *host; uint32_t port, nseg; char **segs; } XPat;
typedef struct { char *header, *value; int replace; uint32_t nd, np; XPat *d, *p; } XRule;
typedef struct { XRule *rules; uint32_t n; } XHeaders;

/* ---- reading ----------------------------------------------------------------------------------------------------------- */

typedef struct { const uint8_t *p; size_t n, o; int bad; } XRd;

static inline uint32_t xs_u32(XRd *r) {
    uint32_t v = 0;
    if (r->o + 4 > r->n) { r->bad = 1; return 0; }
    memcpy(&v, r->p + r->o, 4); /* little-endian hosts only, like proto.h */
    r->o += 4;
    return v;
}
static inline uint8_t xs_u8(XRd *r) {
    if (r->o + 1 > r->n) { r->bad = 1; return 0; }
    return r->p[r->o++];
}
/* a newly allocated NUL-terminated string; invalid input sets bad and returns an empty string */
static inline char *xs_str(XRd *r) {
    uint32_t n = xs_u32(r);
    if (r->bad || n > XS_MAX_STR || r->o + n > r->n) { r->bad = 1; return calloc(1, 1); }
    char *s = malloc(n + 1);
    if (!s) abort();
    memcpy(s, r->p + r->o, n);
    s[n] = 0;
    r->o += n;
    return s;
}
static inline uint32_t xs_count(XRd *r, uint32_t max) {
    uint32_t n = xs_u32(r);
    if (n > max) { r->bad = 1; return 0; }
    return n;
}

static inline void xs_free_pats(XPat *ps, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) {
        free(ps[i].host);
        for (uint32_t j = 0; j < ps[i].nseg; j++) free(ps[i].segs[j]);
        free(ps[i].segs);
    }
    free(ps);
}

static inline void xs_free(XHeaders *h) {
    for (uint32_t i = 0; i < h->n; i++) {
        XRule *r = &h->rules[i];
        free(r->header);
        if (r->value) { memset(r->value, 0, strlen(r->value)); free(r->value); }
        xs_free_pats(r->d, r->nd);
        xs_free_pats(r->p, r->np);
    }
    free(h->rules);
    h->rules = NULL;
    h->n = 0;
}

static inline XPat *xs_read_pats(XRd *r, uint32_t *out_n) {
    uint32_t n = xs_count(r, XS_MAX_PATTERNS);
    XPat *ps = calloc(n ? n : 1, sizeof(XPat));
    if (!ps) abort();
    uint32_t done = 0;
    for (; done < n && !r->bad; done++) {
        XPat *p = &ps[done];
        p->scheme = xs_u8(r);
        p->suffix = xs_u8(r) != 0;
        p->host = xs_str(r);
        p->port = xs_u32(r);
        if (p->port > 65535) r->bad = 1;
        uint32_t ns = xs_count(r, XS_MAX_SEGMENTS);
        p->segs = calloc(ns ? ns : 1, sizeof(char *));
        if (!p->segs) abort();
        for (uint32_t j = 0; j < ns && !r->bad; j++) p->segs[p->nseg++] = xs_str(r);
    }
    *out_n = done;
    return ps;
}

/* the payload of a request.headers hook; appends to h. Returns 0 when whole, -1 when broken (what was read is dropped). */
static inline int xs_read_header_rules(const uint8_t *b, size_t len, XHeaders *h) {
    XRd r = { b, len, 0, 0 };
    uint32_t n = xs_count(&r, XS_MAX_RULES);
    XHeaders got = { calloc(n ? n : 1, sizeof(XRule)), 0 };
    if (!got.rules) abort();
    for (uint32_t i = 0; i < n && !r.bad; i++) {
        XRule *x = &got.rules[got.n++];
        x->header = xs_str(&r);
        x->value = xs_str(&r);
        x->replace = xs_u8(&r) != 0;
        x->d = xs_read_pats(&r, &x->nd);
        x->p = xs_read_pats(&r, &x->np);
    }
    if (r.bad) { xs_free(&got); return -1; }
    XRule *all = realloc(h->rules, (h->n + got.n + 1) * sizeof(XRule));
    if (!all) abort();
    memcpy(all + h->n, got.rules, got.n * sizeof(XRule));
    h->rules = all;
    h->n += got.n;
    free(got.rules);
    return 0;
}

static inline void xs_note(char *rep, size_t cap, const char *fmt, ...) __attribute__((format(printf, 3, 4)));
static inline void xs_note(char *rep, size_t cap, const char *fmt, ...) {
    size_t used = strlen(rep);
    if (used + 2 >= cap) return;
    va_list ap;
    va_start(ap, fmt);
    if (used) { rep[used++] = ';'; rep[used++] = ' '; rep[used] = 0; }
    vsnprintf(rep + used, cap - used, fmt, ap);
    va_end(ap);
}

static inline const char *xs_hook_name(uint32_t k) {
    switch (k) {
    case XS_HOOK_REQUEST_HEADERS: return "request.headers";
    case XS_HOOK_RESPONSE_HEADERS: return "response.headers";
    case XS_HOOK_REQUEST_BLOCK: return "request.block";
    case XS_HOOK_CONTENT_SCRIPT: return "content.script";
    case XS_HOOK_PAGE_ACTION: return "page.action";
    default: return NULL;
    }
}
static inline const char *xs_hook_cap(uint32_t k) {
    switch (k) {
    case XS_HOOK_REQUEST_HEADERS: return "web.request.headers";
    case XS_HOOK_RESPONSE_HEADERS: return "web.response.headers";
    case XS_HOOK_REQUEST_BLOCK: return "web.request.block";
    case XS_HOOK_CONTENT_SCRIPT: return "web.content.script";
    case XS_HOOK_PAGE_ACTION: return "web.page.action";
    default: return "";
    }
}

/* Parse a whole snapshot into the header rules it carries. Returns 0 (rules in *h, which was emptied first; report says what was
 * ignored) or -1 (refused: *h is empty, report says why). */
static inline int xs_parse(const uint8_t *b, size_t len, XHeaders *h, char *rep, size_t cap) {
    xs_free(h);
    if (cap) rep[0] = 0;
    if (len > XS_MAX_SNAPSHOT) { xs_note(rep, cap, "snapshot too long"); return -1; }
    XRd r = { b, len, 0, 0 };
    uint32_t v = xs_u32(&r);
    if (r.bad) { xs_note(rep, cap, "snapshot is cut short"); return -1; }
    if (v != XS_VERSION) { xs_note(rep, cap, "snapshot version %u is not supported (this build reads %u)", v, XS_VERSION); return -1; }
    uint32_t ne = xs_count(&r, XS_MAX_ENTRIES);
    for (uint32_t i = 0; i < ne && !r.bad; i++) {
        char *id = xs_str(&r), *ver = xs_str(&r);
        uint32_t nc = xs_count(&r, XS_MAX_CAPS);
        char **caps = calloc(nc ? nc : 1, sizeof(char *));
        if (!caps) abort();
        uint32_t got_c = 0;
        for (uint32_t j = 0; j < nc && !r.bad; j++) caps[got_c++] = xs_str(&r);
        uint32_t nh = xs_count(&r, XS_MAX_HOOKS);
        for (uint32_t j = 0; j < nh && !r.bad; j++) {
            uint32_t kind = xs_u32(&r), plen = xs_u32(&r);
            if (r.bad || r.o + plen > r.n) { r.bad = 1; break; }
            const uint8_t *payload = r.p + r.o;
            r.o += plen;
            const char *name = xs_hook_name(kind);
            if (!name) { xs_note(rep, cap, "%s: hook kind %u is unknown, ignored", id, kind); continue; }
            if (kind != XS_HOOK_REQUEST_HEADERS) { xs_note(rep, cap, "%s: hook %s is not implemented in this build, ignored", id, name); continue; }
            int has = 0;
            for (uint32_t c = 0; c < got_c; c++) if (!strcmp(caps[c], xs_hook_cap(kind))) has = 1;
            if (!has) { xs_note(rep, cap, "%s: hook %s without the capability %s, ignored", id, name, xs_hook_cap(kind)); continue; }
            if (xs_read_header_rules(payload, plen, h) < 0) xs_note(rep, cap, "%s: hook %s is broken, ignored", id, name);
        }
        for (uint32_t c = 0; c < got_c; c++) free(caps[c]);
        free(caps);
        free(id);
        free(ver);
    }
    if (r.bad) { xs_free(h); xs_note(rep, cap, "snapshot is cut short or has a part that is too long"); return -1; }
    return 0;
}

/* ---- matching (twin of ext.rs `headers_for`) ------------------------------------------------------------------------------ */

typedef struct { int https; char host[256]; uint32_t port; char *segs[XS_MAX_SEGMENTS]; int nseg; int ambiguous; char buf[2100]; } XTarget;

/* Reduce an http(s) URL to what matching needs; -1 for anything that cannot be judged (then nothing is added). */
static inline int xs_target(const char *url, XTarget *t) {
    memset(t, 0, sizeof *t);
    size_t ul = strlen(url);
    if (ul == 0 || ul > 2048) return -1;
    for (size_t i = 0; i < ul; i++) if ((unsigned char)url[i] <= 0x20 || (unsigned char)url[i] >= 0x7f) return -1;
    const char *rest;
    if (!strncasecmp(url, "https://", 8)) { t->https = 1; rest = url + 8; }
    else if (!strncasecmp(url, "http://", 7)) rest = url + 7;
    else return -1;
    size_t alen = strcspn(rest, "/?#");
    if (alen == 0 || memchr(rest, '@', alen) || memchr(rest, '\\', alen)) return -1;
    size_t hlen = alen;
    t->port = t->https ? 443 : 80;
    const char *colon = memchr(rest, ':', alen);
    if (rest[0] == '[') return -1; /* address literals are not exported to this engine */
    if (colon) {
        hlen = (size_t)(colon - rest);
        size_t pl = alen - hlen - 1;
        if (pl > 5) return -1;
        uint32_t p = 0;
        for (size_t i = 0; i < pl; i++) { if (!isdigit((unsigned char)colon[1 + i])) return -1; p = p * 10 + (uint32_t)(colon[1 + i] - '0'); }
        if (pl > 0) { if (p == 0 || p > 65535) return -1; t->port = p; }
    }
    if (hlen == 0 || hlen > 253) return -1;
    for (size_t i = 0; i < hlen; i++) {
        unsigned char c = (unsigned char)rest[i];
        if (!(isalnum(c) || c == '-' || c == '.')) return -1;
        t->host[i] = (char)tolower(c);
    }
    if (t->host[hlen - 1] == '.' || t->host[0] == '.') return -1;
    /* a dotted number is an address, not a name */
    const char *lastdot = strrchr(t->host, '.');
    const char *last = lastdot ? lastdot + 1 : t->host;
    int alldigit = *last != 0;
    for (const char *c = last; *c; c++) if (!isdigit((unsigned char)*c)) alldigit = 0;
    if (alldigit) return -1;
    const char *path = rest + alen;
    size_t pl = strcspn(path, "?#");
    if (pl >= sizeof t->buf) return -1;
    memcpy(t->buf, path, pl);
    t->buf[pl] = 0;
    if (strchr(t->buf, '%') || strchr(t->buf, '\\')) t->ambiguous = 1;
    char *p = t->buf;
    if (*p == '/') p++;
    while (*p) {
        char *e = strchr(p, '/');
        size_t sl = e ? (size_t)(e - p) : strlen(p);
        if (sl == 0 || (sl == 1 && p[0] == '.') || (sl == 2 && p[0] == '.' && p[1] == '.')) t->ambiguous = 1;
        else if (t->nseg < (int)XS_MAX_SEGMENTS) { if (e) *e = 0; t->segs[t->nseg++] = p; }
        else t->ambiguous = 1;
        if (!e) break;
        p = e + 1;
    }
    return 0;
}

static inline int xs_pat_hits(const XPat *p, const XTarget *t) {
    size_t hl = strlen(t->host), pl = strlen(p->host);
    int host_ok = p->suffix ? (hl > pl + 1 && !strcmp(t->host + hl - pl, p->host) && t->host[hl - pl - 1] == '.') : !strcmp(t->host, p->host);
    if (!host_ok) return 0;
    if ((p->scheme == 1 && t->https) || (p->scheme == 2 && !t->https)) return 0;
    if (p->port && p->port != t->port) return 0;
    if (p->nseg == 0) return 1;
    if (t->ambiguous || p->nseg > (uint32_t)t->nseg) return 0;
    for (uint32_t i = 0; i < p->nseg; i++) if (strcmp(p->segs[i], t->segs[i])) return 0;
    return 1;
}

static inline int xs_rule_matches(const XRule *r, const XTarget *t) {
    if (!t->https && strcmp(t->host, "localhost") && !(strlen(t->host) > 10 && !strcmp(t->host + strlen(t->host) - 10, ".localhost"))) return 0;
    int d = r->nd == 0, p = r->np == 0;
    for (uint32_t i = 0; i < r->nd && !d; i++) d = xs_pat_hits(&r->d[i], t);
    for (uint32_t i = 0; i < r->np && !p; i++) p = xs_pat_hits(&r->p[i], t);
    return d && p;
}

typedef struct { const char *name, *value; int replace; } XOut;

/* the headers for one request: each matching rule once per header name (first wins), at most XS_MAX_PER_REQUEST */
static inline int xs_headers_for(const XHeaders *h, const XTarget *t, XOut out[XS_MAX_PER_REQUEST]) {
    int n = 0;
    for (uint32_t i = 0; i < h->n && n < XS_MAX_PER_REQUEST; i++) {
        if (!xs_rule_matches(&h->rules[i], t)) continue;
        int dup = 0;
        for (int j = 0; j < n; j++) if (!strcasecmp(out[j].name, h->rules[i].header)) dup = 1;
        if (!dup) { out[n].name = h->rules[i].header; out[n].value = h->rules[i].value; out[n].replace = h->rules[i].replace; n++; }
    }
    return n;
}

#endif
