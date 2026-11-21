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

#define XS_MAX_BODY (32u * 1024u)
#define XS_MAX_SCRIPTS 16u
#define XS_MAX_UA 400u

enum { XS_HOOK_REQUEST_HEADERS = 1, XS_HOOK_RESPONSE_HEADERS, XS_HOOK_REQUEST_BLOCK, XS_HOOK_CONTENT_SCRIPT, XS_HOOK_PAGE_ACTION, XS_HOOK_USER_AGENT };

typedef struct { uint8_t scheme, suffix; char *host; uint32_t port, nseg; char **segs; } XPat;
typedef struct { char *header, *value; int replace; uint32_t nd, np; XPat *d, *p; } XRule;
typedef struct { XRule *rules; uint32_t n; } XHeaders;
/* The sites of a user script or style as the engine's own match patterns (`scheme://host/path`, host `*.example.com`), a NULL-terminated list
 * for webkit_user_script_new. A plug-in's `*.example.com` means the domain and its subdomains (browserplugins.rs), which is what the engine's
 * `*.` pattern matches. */
typedef struct { char **allow; uint32_t nallow; } XSites;
/* One user script or style sheet (hook kind 4). The helper puts it into the page; the web process never does. */
typedef struct { int style, at_end; XSites sites; char *body; } XScript;
/* What the helper (not the web process) acts on: scripts, styles and the User-Agent (hook kinds 4 and 6). */
typedef struct { XScript *s; uint32_t n; int has_ua; char *ua; } XContent;

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
/* like xs_str with a longer limit (the body of a script) */
static inline char *xs_str_max(XRd *r, uint32_t max) {
    uint32_t n = xs_u32(r);
    if (r->bad || n > max || r->o + n > r->n) { r->bad = 1; return calloc(1, 1); }
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


static inline void xs_free_sites(XSites *t) {
    for (uint32_t i = 0; i < t->nallow; i++) free(t->allow[i]);
    free(t->allow);
    memset(t, 0, sizeof *t);
}

static inline void xs_content_free(XContent *c) {
    for (uint32_t i = 0; i < c->n; i++) { xs_free_sites(&c->s[i].sites); free(c->s[i].body); }
    free(c->s);
    free(c->ua);
    memset(c, 0, sizeof *c);
}

static inline void xs_site_add(char ***list, uint32_t *n, char *text) {
    char **grown = realloc(*list, (*n + 2) * sizeof(char *));
    if (!grown) abort();
    *list = grown;
    grown[(*n)++] = text;
    grown[*n] = NULL;
}

static inline char *xs_fmt(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static inline char *xs_fmt(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int len = vsnprintf(NULL, 0, fmt, ap);
    va_end(ap);
    char *out = malloc((size_t)len + 1);
    if (!out) abort();
    va_start(ap, fmt);
    vsnprintf(out, (size_t)len + 1, fmt, ap);
    va_end(ap);
    return out;
}

/* Patterns to the engine's match patterns. A pattern with a port is left out (the engine's patterns have no port, so it could only match
 * more than it says); returns how many patterns were used. Never produces a catch-all: an empty host is left out too. */
static inline uint32_t xs_sites_from(const XPat *ps, uint32_t n, XSites *out) {
    uint32_t used = 0;
    for (uint32_t i = 0; i < n; i++) {
        const XPat *p = &ps[i];
        if (p->port || !p->host[0] || strchr(p->host, '*') || strchr(p->host, '/')) continue;
        char path[512] = "/";
        for (uint32_t j = 0; j < p->nseg; j++) {
            if (strlen(path) + strlen(p->segs[j]) + 3 >= sizeof path || strchr(p->segs[j], '*')) { path[0] = 0; break; }
            strcat(path, p->segs[j]);
            strcat(path, "/");
        }
        if (!path[0]) continue;
        used++;
        for (int https = 0; https < 2; https++) {
            if ((p->scheme == 1 && https) || (p->scheme == 2 && !https)) continue;
            xs_site_add(&out->allow, &out->nallow, xs_fmt("%s://%s%s/%s*", https ? "https" : "http", p->suffix ? "*." : "", p->host, path + 1));
        }
    }
    return used;
}

/* the payload of a content.script hook; appends to c. 0 when whole, -1 when broken. An entry without a usable site is dropped and named. */
static inline int xs_read_content_scripts(const uint8_t *b, size_t len, XContent *c, char *rep, size_t cap, const char *id) {
    XRd r = { b, len, 0, 0 };
    uint32_t n = xs_count(&r, XS_MAX_SCRIPTS);
    XScript *got = calloc(n ? n : 1, sizeof(XScript));
    if (!got) abort();
    uint32_t have = 0, dropped = 0;
    for (uint32_t i = 0; i < n && !r.bad; i++) {
        XScript x = {0};
        x.style = xs_u8(&r) != 0;
        x.at_end = xs_u8(&r) != 0;
        uint32_t nd = 0;
        XPat *d = xs_read_pats(&r, &nd);
        x.body = xs_str_max(&r, XS_MAX_BODY);
        uint32_t used = r.bad ? 0 : xs_sites_from(d, nd, &x.sites);
        xs_free_pats(d, nd);
        if (r.bad || used == 0 || !x.body[0]) { xs_free_sites(&x.sites); free(x.body); if (!r.bad) dropped++; continue; }
        got[have++] = x;
    }
    if (r.bad) {
        for (uint32_t i = 0; i < have; i++) { xs_free_sites(&got[i].sites); free(got[i].body); }
        free(got);
        return -1;
    }
    if (dropped) xs_note(rep, cap, "%s: %u script or style without a usable site (a port or no site), ignored", id, dropped);
    XScript *all = realloc(c->s, (c->n + have + 1) * sizeof(XScript));
    if (!all) abort();
    memcpy(all + c->n, got, have * sizeof(XScript));
    c->s = all;
    c->n += have;
    free(got);
    return 0;
}

/* the payload of a user.agent hook: one line of printable ASCII, at most 400 characters; empty means the engine's own */
static inline int xs_read_user_agent(const uint8_t *b, size_t len, XContent *c) {
    XRd r = { b, len, 0, 0 };
    char *ua = xs_str(&r);
    if (r.bad || strlen(ua) > XS_MAX_UA) { free(ua); return -1; }
    for (const char *p = ua; *p; p++) if ((unsigned char)*p < 0x20 || (unsigned char)*p > 0x7e) { free(ua); return -1; }
    free(c->ua);
    c->ua = ua;
    c->has_ua = 1;
    return 0;
}

static inline const char *xs_hook_name(uint32_t k) {
    switch (k) {
    case XS_HOOK_REQUEST_HEADERS: return "request.headers";
    case XS_HOOK_RESPONSE_HEADERS: return "response.headers";
    case XS_HOOK_REQUEST_BLOCK: return "request.block";
    case XS_HOOK_CONTENT_SCRIPT: return "content.script";
    case XS_HOOK_PAGE_ACTION: return "page.action";
    case XS_HOOK_USER_AGENT: return "user.agent";
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
    case XS_HOOK_USER_AGENT: return "web.user.agent";
    default: return "";
    }
}

/* Parse a whole snapshot. The header rules go to *h (the web process acts on them) and the scripts, styles and User-Agent to *c (the helper
 * acts on them); either may be NULL when the caller does not act on that part. Both are emptied first. Returns 0 (report says what was ignored)
 * or -1 (refused: both are empty, report says why). */
static inline int xs_parse_all(const uint8_t *b, size_t len, XHeaders *h, XContent *c, char *rep, size_t cap) {
    XHeaders sink_h = {0};
    XContent sink_c = {0};
    if (!h) h = &sink_h;
    if (!c) c = &sink_c;
    xs_free(h);
    xs_content_free(c);
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
            if (kind != XS_HOOK_REQUEST_HEADERS && kind != XS_HOOK_CONTENT_SCRIPT && kind != XS_HOOK_USER_AGENT) { xs_note(rep, cap, "%s: hook %s is not implemented in this build, ignored", id, name); continue; }
            int has = 0;
            for (uint32_t k = 0; k < got_c; k++) if (!strcmp(caps[k], xs_hook_cap(kind))) has = 1;
            if (!has) { xs_note(rep, cap, "%s: hook %s without the capability %s, ignored", id, name, xs_hook_cap(kind)); continue; }
            int rc = 0;
            if (kind == XS_HOOK_REQUEST_HEADERS) rc = xs_read_header_rules(payload, plen, h);
            else if (kind == XS_HOOK_CONTENT_SCRIPT) rc = xs_read_content_scripts(payload, plen, c, rep, cap, id);
            else rc = xs_read_user_agent(payload, plen, c);
            if (rc < 0) xs_note(rep, cap, "%s: hook %s is broken, ignored", id, name);
        }
        for (uint32_t k = 0; k < got_c; k++) free(caps[k]);
        free(caps);
        free(id);
        free(ver);
    }
    if (r.bad) { xs_free(h); xs_content_free(c); xs_note(rep, cap, "snapshot is cut short or has a part that is too long"); return -1; }
    xs_content_free(&sink_c);
    if (h == &sink_h) xs_free(&sink_h);
    return 0;
}

/* The web process only needs the header rules. */
static inline int xs_parse(const uint8_t *b, size_t len, XHeaders *h, char *rep, size_t cap) {
    return xs_parse_all(b, len, h, NULL, rep, cap);
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
