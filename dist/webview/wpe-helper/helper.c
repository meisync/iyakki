/*
 * IYAKKI web view helper for the web image: WPE WebKit rendered offscreen into a picture the shell shows.
 *
 * Speaks docs/WEBVIEW-PROTOCOL.md exactly like dist/webview/host-helper.py (the WebKitGTK helper for a desktop):
 *     webview-helper --socket-fd 0 --shm /dev/shm/iyakki-webview-PID-N.shm
 *
 * Rendering: libwpe + WPEBackend-fdo with shared-memory buffers. WebKit's web process draws with Mesa (llvmpipe or a GPU
 * driver) into wl_shm buffers of the fdo compositor that lives inside this process; export_shm_buffer hands each finished
 * picture to us, we copy it into a private picture, find what changed since the last one the shell has, write that into a
 * free slot of the shared file and send Frame. No display server, no compositor program.
 *
 * Environment: IYAKKI_WEBVIEW_FPS (default 30), IYAKKI_WEBVIEW_DEBUG=1 (more log lines), IYAKKI_WEBVIEW_NICE (default 0 = leave alone;
 * 1 to 19: the engine's processes run that much below the shell, so a page that keeps every core busy cannot starve the shell's own
 * drawing and input; off by default because an A/B on a MacBook Pro was inconclusive and other programs (k3s) share the machine).
 */
#define _GNU_SOURCE
#ifndef IYAKKI_AUTOPLAY_DEFAULT
#define IYAKKI_AUTOPLAY_DEFAULT "allow"
#endif
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include <glib.h>
#include <glib-unix.h>
#include <wayland-server.h>
#include <wpe/webkit.h>
#include <wpe/fdo.h>
#include <wpe/unstable/fdo-shm.h>
#include <wpe/fdo-egl.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <gbm.h>

#define IYAKKI_MIC_WITH_ALSA 1
#include "micscan.h"
#include "proto.h"

#define PROTOCOL_VERSION 1
#ifndef CAMSHARE_PLUGIN
#define CAMSHARE_PLUGIN "/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so"
#endif
#ifndef MIC_PLUGIN
#define MIC_PLUGIN "/usr/lib/gstreamer-1.0/libgstiyakkimic.so"
#define WEBSINK_PLUGIN "/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so"
#endif
#define MAX_SCREEN_W 4096
#define MAX_SCREEN_H 2304

typedef struct {
    int sock, shm_fd;
    GByteArray *in;
    /* Hello */
    uint32_t w, h, scale, max_w, max_h;
    char *ua, *profile, *locale;
    char **allow; int n_allow;
    /* engine */
    struct wpe_view_backend_exportable_fdo *exportable;
    struct wpe_view_backend *backend;
    WebKitWebView *view;
    WebKitNetworkSession *session;
    WebKitUserContentManager *ucm;
    int filter_wait;                 /* a content filter is being loaded: the first page waits for it */
    char *pending_nav;
    uint32_t zoom;
    int visible, focused;
    int fullscreen;                  /* the page is in DOM fullscreen (the shell shows it over the whole screen) */
    int ready;
    /* pixels */
    uint8_t *back;                   /* newest picture from the engine, bw x bh */
    uint32_t bw, bh;
    uint8_t *shown;                  /* the shell's copy: what it has after the frames sent so far */
    uint32_t sw, sh;
    gboolean back_new;               /* back holds a picture not yet sent */
    gboolean need_full;
    gboolean busy[2];
    int next_slot;
    guint flush_src;
    gint64 last_frame_us;
    guint fps;
    /* state reported */
    char *committed;
    int cursor;
    int crashes;
    uint32_t last_buttons;
    int debug;
    /* hardware path: EGL display on the render node, finished pictures read back with GL */
    int egl;                         /* 1: fdo is initialised for an EGL display and exports EGL images */
    EGLDisplay edpy; EGLContext ectx;
    struct gbm_device *gbm; int drm_fd;
    GLuint tex, fbo;
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC img_target;
    int bgra_ok;                     /* glReadPixels can return BGRA directly */
    unsigned nreadback;
    unsigned nframes, nexports, nacks, nblocked, nsame;
    GMainLoop *loop;
} H;

static H G;

/* ---------------------------------------------------------------------------------------------- sending */
static void quit_loop(void) { if (G.loop) g_main_loop_quit(G.loop); else _exit(0); }

static void send_raw(const uint8_t *p, size_t n) {
    while (n) {
        ssize_t r = write(G.sock, p, n);
        if (r < 0) {
            if (errno == EINTR) continue;
            if (errno == EAGAIN) { struct pollfd pf = { G.sock, POLLOUT, 0 }; poll(&pf, 1, 1000); continue; }
            quit_loop();
            return;
        }
        p += r; n -= (size_t)r;
    }
}

static void send_msg(uint8_t type, const Buf *b) {
    uint8_t hdr[5];
    uint32_t len = (uint32_t)(b ? b->len : 0) + 1;
    memcpy(hdr, &len, 4);
    hdr[4] = type;
    /* one write when it is small, so messages are whole on the wire */
    if (b && b->len < 60000) {
        uint8_t tmp[60005];
        memcpy(tmp, hdr, 5);
        memcpy(tmp + 5, b->p, b->len);
        send_raw(tmp, 5 + b->len);
    } else {
        send_raw(hdr, 5);
        if (b) send_raw(b->p, b->len);
    }
}

static void send_str_msg(uint8_t type, const char *s) {
    Buf b = {0};
    buf_str(&b, s ? s : "");
    send_msg(type, &b);
    buf_free(&b);
}

static void logf_(const char *fmt, ...) G_GNUC_PRINTF(1, 2);
static void logf_(const char *fmt, ...) {
    char line[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    send_str_msg(F_LOG, line);
    if (G.debug) fprintf(stderr, "wpe-helper: %s\n", line);
}

static void send_bb(uint8_t type, int a, int b2) {
    Buf b = {0};
    buf_u8(&b, a); buf_u8(&b, b2);
    send_msg(type, &b);
    buf_free(&b);
}

static void send_history(void) {
    if (G.view) send_bb(F_HISTORY, webkit_web_view_can_go_back(G.view), webkit_web_view_can_go_forward(G.view));
}

/* ---------------------------------------------------------------------------------------------- frames */
static gboolean flush(gpointer unused);

static void schedule_flush(void) {
    if (G.flush_src || !G.visible || !G.ready) return;
    gint64 now = g_get_monotonic_time();
    gint64 wait = 1000000 / (gint64)G.fps - (now - G.last_frame_us);
    if (wait < 0) wait = 0;
    G.flush_src = g_timeout_add((guint)(wait / 1000) + 1, flush, NULL);
}

static int free_slot(void) {
    if (!G.busy[G.next_slot]) return G.next_slot;
    if (!G.busy[1 - G.next_slot]) return 1 - G.next_slot;
    return -1;
}

/* send the newest picture (or what changed in it) if the shell can take it */
static gboolean flush(gpointer unused) {
    (void)unused;
    G.flush_src = 0;
    if (!G.back_new || !G.visible || !G.back) return G_SOURCE_REMOVE;
    int slot = free_slot();
    if (slot < 0) { G.nblocked++; return G_SOURCE_REMOVE; }           /* an Ack calls us again */
    uint32_t w = G.bw, h = G.bh;
    size_t stride = (size_t)w * 4;
    uint32_t y0 = 0, y1 = h, x0 = 0, x1 = w;
    gboolean full = G.need_full || !G.shown || G.sw != w || G.sh != h;
    if (!full) {
        while (y0 < h && memcmp(G.back + y0 * stride, G.shown + y0 * stride, stride) == 0) y0++;
        if (y0 == h) { G.nsame++; G.back_new = FALSE; return G_SOURCE_REMOVE; }      /* nothing changed */
        y1 = h;
        while (y1 > y0 && memcmp(G.back + (y1 - 1) * stride, G.shown + (y1 - 1) * stride, stride) == 0) y1--;
        /* narrow the columns too (cheap enough for the rows that changed) */
        x0 = w; x1 = 0;
        for (uint32_t y = y0; y < y1 && (x0 > 0 || x1 < w); y++) {
            const uint32_t *a = (const uint32_t *)(G.back + y * stride), *b = (const uint32_t *)(G.shown + y * stride);
            uint32_t x = 0;
            while (x < x0 && a[x] == b[x]) x++;
            if (x < x0) x0 = x;
            uint32_t e = w;
            while (e > x1 && a[e - 1] == b[e - 1]) e--;
            if (e > x1) x1 = e;
        }
        if (x1 <= x0) { x0 = 0; x1 = w; }
    }
    /* the rows y0..y1 are written whole (contiguous, one pwrite) */
    off_t base = (off_t)slot * G.max_w * G.max_h * 4;
    size_t off = (size_t)y0 * stride, len = (size_t)(y1 - y0) * stride;
    const uint8_t *src = G.back + off;
    while (len) {
        ssize_t r = pwrite(G.shm_fd, src, len, base + (off_t)off);
        if (r < 0) { if (errno == EINTR) continue; logf_("writing the pictures failed: %s", strerror(errno)); quit_loop(); return G_SOURCE_REMOVE; }
        src += r; off += (size_t)r; len -= (size_t)r;
    }
    if (!G.shown || G.sw != w || G.sh != h) {
        free(G.shown);
        G.shown = malloc((size_t)w * h * 4);
        G.sw = w; G.sh = h;
    }
    memcpy(G.shown + (size_t)y0 * stride, G.back + (size_t)y0 * stride, (size_t)(y1 - y0) * stride);
    G.nframes++;
    G.busy[slot] = TRUE;
    G.next_slot = 1 - slot;
    G.back_new = FALSE;
    G.need_full = FALSE;
    G.last_frame_us = g_get_monotonic_time();
    Buf b = {0};
    buf_u8(&b, slot); buf_u32(&b, w); buf_u32(&b, h); buf_u32(&b, 1);
    buf_u32(&b, full ? 0 : x0); buf_u32(&b, full ? 0 : y0); buf_u32(&b, full ? w : x1 - x0); buf_u32(&b, full ? h : y1 - y0);
    send_msg(F_FRAME, &b);
    buf_free(&b);
    return G_SOURCE_REMOVE;
}

/* the engine finished a picture as an EGL image (a dma-buf of the GPU): read it back into G.back */
static void export_egl_image(void *data, struct wpe_fdo_egl_exported_image *img) {
    (void)data;
    uint32_t bw = wpe_fdo_egl_exported_image_get_width(img), bh = wpe_fdo_egl_exported_image_get_height(img);
    if (bw > 0 && bh > 0 && bw <= G.max_w && bh <= G.max_h) {
        if (bw != G.bw || bh != G.bh || !G.back) {
            free(G.back);
            G.back = malloc((size_t)bw * bh * 4);
            G.bw = bw; G.bh = bh;
        }
        EGLImageKHR ei = wpe_fdo_egl_exported_image_get_egl_image(img);
        while (glGetError() != GL_NO_ERROR) {}
        glBindTexture(GL_TEXTURE_2D, G.tex);
        G.img_target(GL_TEXTURE_2D, (GLeglImageOES)ei);
        glBindFramebuffer(GL_FRAMEBUFFER, G.fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, G.tex, 0);
        if (glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE) {
            glPixelStorei(GL_PACK_ALIGNMENT, 4);
            glReadPixels(0, 0, (GLsizei)bw, (GLsizei)bh, G.bgra_ok ? GL_BGRA_EXT : GL_RGBA, GL_UNSIGNED_BYTE, G.back);
            if (glGetError() == GL_NO_ERROR) {
                if (!G.bgra_ok) {
                    uint32_t *q = (uint32_t *)G.back;
                    for (size_t i = 0, n = (size_t)bw * bh; i < n; i++) {
                        uint32_t v = q[i];
                        q[i] = (v & 0xff00ff00u) | ((v & 0xffu) << 16) | ((v >> 16) & 0xffu);
                    }
                }
                G.back_new = TRUE;
                G.nreadback++;
            }
        } else if (G.debug) logf_("framebuffer for the exported image is incomplete");
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, 0, 0);
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
    }
    G.nexports++;
    wpe_view_backend_exportable_fdo_egl_dispatch_release_exported_image(G.exportable, img);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(G.exportable);
    schedule_flush();
}

/* the engine finished a picture */
static void export_shm_buffer(void *data, struct wpe_fdo_shm_exported_buffer *eb) {
    (void)data;
    struct wl_shm_buffer *sb = wpe_fdo_shm_exported_buffer_get_shm_buffer(eb);
    if (sb) {
        int32_t bw = wl_shm_buffer_get_width(sb), bh = wl_shm_buffer_get_height(sb), stride = wl_shm_buffer_get_stride(sb);
        uint32_t fmt = wl_shm_buffer_get_format(sb);
        if (bw > 0 && bh > 0 && (uint32_t)bw <= G.max_w && (uint32_t)bh <= G.max_h) {
            if ((uint32_t)bw != G.bw || (uint32_t)bh != G.bh || !G.back) {
                free(G.back);
                G.back = malloc((size_t)bw * bh * 4);
                G.bw = (uint32_t)bw; G.bh = (uint32_t)bh;
            }
            wl_shm_buffer_begin_access(sb);
            const uint8_t *p = wl_shm_buffer_get_data(sb);
            for (int32_t y = 0; y < bh; y++) {
                uint8_t *d = G.back + (size_t)y * bw * 4;
                memcpy(d, p + (size_t)y * stride, (size_t)bw * 4);
                if (fmt == WL_SHM_FORMAT_XRGB8888) {
                    uint32_t *q = (uint32_t *)d;
                    for (int32_t x = 0; x < bw; x++) q[x] |= 0xff000000u;
                }
            }
            wl_shm_buffer_end_access(sb);
            G.back_new = TRUE;
        }
    }
    G.nexports++;
    if (G.egl) wpe_view_backend_exportable_fdo_egl_dispatch_release_shm_exported_buffer(G.exportable, eb);
    else wpe_view_backend_exportable_fdo_dispatch_release_shm_exported_buffer(G.exportable, eb);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(G.exportable);
    schedule_flush();
}

static const struct wpe_view_backend_exportable_fdo_client export_client = { .export_shm_buffer = export_shm_buffer };
static const struct wpe_view_backend_exportable_fdo_egl_client export_egl_client = { .export_fdo_egl_image = export_egl_image, .export_shm_buffer = export_shm_buffer };

/* A hardware EGL display on the render node (GBM, else surfaceless) with a GLES2 context for the read-back.
 * Returns 1 when everything needed exists; on any failure everything is released and 0 is returned (the caller falls back to wl_shm). */
static void hw_close(void) {
    if (G.edpy != EGL_NO_DISPLAY) {
        eglMakeCurrent(G.edpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        if (G.ectx != EGL_NO_CONTEXT) eglDestroyContext(G.edpy, G.ectx);
        eglTerminate(G.edpy);
    }
    G.edpy = EGL_NO_DISPLAY; G.ectx = EGL_NO_CONTEXT;
    if (G.gbm) gbm_device_destroy(G.gbm);
    G.gbm = NULL;
    if (G.drm_fd >= 0) close(G.drm_fd);
    G.drm_fd = -1;
}

/* One render node: open it, bring up GBM, EGL and a GLES2 context on it. 1 when G is set up on it; 0 when it is unusable (everything
   closed again). `software` says the node gave only Mesa's software renderer (llvmpipe/softpipe): a card the kernel declared wedged, or
   no driver for it, still hands out a render node and Mesa falls back to llvmpipe on it; that is not the card. */
static int hw_try_node(const char *path, PFNEGLGETPLATFORMDISPLAYEXTPROC get_pd, int *software) {
    *software = 0;
    int fd = open(path, O_RDWR | O_CLOEXEC);
    if (fd < 0) return 0;
    struct gbm_device *gbm = gbm_create_device(fd);
    EGLDisplay d = gbm ? get_pd(EGL_PLATFORM_GBM_KHR, gbm, NULL) : EGL_NO_DISPLAY;
    EGLint maj, min;
    if (d == EGL_NO_DISPLAY || !eglInitialize(d, &maj, &min)) { if (gbm) gbm_device_destroy(gbm); close(fd); return 0; }
    G.drm_fd = fd; G.gbm = gbm; G.edpy = d;
    const char *ext = eglQueryString(G.edpy, EGL_EXTENSIONS);
    if (!ext || !strstr(ext, "EGL_WL_bind_wayland_display") || !strstr(ext, "EGL_KHR_image_base")) {
        logf_("graphics: %s lacks wl_bind or image support", path); hw_close(); return 0;
    }
    eglBindAPI(EGL_OPENGL_ES_API);
    EGLint cfg_attr[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, 0, EGL_NONE };
    EGLConfig cfg; EGLint ncfg = 0;
    if (!eglChooseConfig(G.edpy, cfg_attr, &cfg, 1, &ncfg) || ncfg < 1) {
        EGLint a2[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_NONE };
        if (!eglChooseConfig(G.edpy, a2, &cfg, 1, &ncfg) || ncfg < 1) { logf_("graphics: %s has no ES2 config", path); hw_close(); return 0; }
    }
    EGLint ctx_attr[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    G.ectx = eglCreateContext(G.edpy, cfg, EGL_NO_CONTEXT, ctx_attr);
    if (G.ectx == EGL_NO_CONTEXT || !eglMakeCurrent(G.edpy, EGL_NO_SURFACE, EGL_NO_SURFACE, G.ectx)) {
        logf_("graphics: %s gives no GLES2 context (0x%x)", path, eglGetError()); hw_close(); return 0;
    }
    const char *gl = (const char *)glGetString(GL_EXTENSIONS);
    G.img_target = (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");
    if (!G.img_target || !gl || !strstr(gl, "GL_OES_EGL_image")) { logf_("graphics: %s has no GL_OES_EGL_image", path); hw_close(); return 0; }
    const char *renderer = (const char *)glGetString(GL_RENDERER);
    if (renderer && (strstr(renderer, "llvmpipe") || strstr(renderer, "softpipe") || strstr(renderer, "SWR"))) *software = 1;
    G.bgra_ok = strstr(gl, "GL_EXT_read_format_bgra") != NULL;
    return 1;
}

/* The rule: the graphics card when its driver and firmware work, software drawing otherwise. Every render node is tried in turn and the
   first one with a real renderer wins; a node that gives only llvmpipe (a wedged or driverless card: the HP's Meteor Lake without its GuC
   firmware, 2026-11-07) is left aside so a second card (an NVIDIA one) still gets its chance. When no card renders, the pictures go through
   shared memory: the same CPU work as llvmpipe, without buffers on a card that is not there. IYAKKI_WEBVIEW_RENDER_NODE pins one node. */
static int hw_open(void) {
    G.drm_fd = -1; G.edpy = EGL_NO_DISPLAY; G.ectx = EGL_NO_CONTEXT;
    const char *off = getenv("IYAKKI_WEBVIEW_GPU");
    if (getenv("LIBGL_ALWAYS_SOFTWARE") || (off && !strcmp(off, "0"))) { logf_("graphics: software requested, using shared memory pictures"); return 0; }
    const char *node = getenv("IYAKKI_WEBVIEW_RENDER_NODE");
    char path[64];
    PFNEGLGETPLATFORMDISPLAYEXTPROC get_pd = (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT");
    if (!get_pd) { logf_("graphics: no eglGetPlatformDisplayEXT, using shared memory pictures"); return 0; }
    int tried = 0, soft_nodes = 0;
    for (int i = 0; i < 4; i++) {
        if (node) { if (i) break; snprintf(path, sizeof path, "%s", node); }
        else snprintf(path, sizeof path, "/dev/dri/renderD%d", 128 + i);
        int software = 0;
        if (!hw_try_node(path, get_pd, &software)) continue;
        tried++;
        if (software && !node) {
            logf_("graphics: %s gives only %s (the card is not usable: wedged, or its driver has no firmware), looking further", path, (const char *)glGetString(GL_RENDERER));
            soft_nodes++; hw_close(); continue;
        }
        glGenTextures(1, &G.tex);
        glGenFramebuffers(1, &G.fbo);
        logf_("graphics: hardware %s on %s (%s)", (const char *)glGetString(GL_RENDERER), path, G.bgra_ok ? "BGRA read-back" : "RGBA read-back");
        return 1;
    }
    if (soft_nodes) logf_("graphics: no card renders (%d node(s) with a software renderer only), using shared memory pictures", soft_nodes);
    else if (!tried) logf_("graphics: no usable render node, using shared memory pictures");
    return 0;
}

static void mark_all(void) { G.need_full = TRUE; G.back_new = G.back != NULL; schedule_flush(); }

/* ---------------------------------------------------------------------------------------------- engine signals */
static gboolean host_allowed(const char *uri) {
    if (!uri) return FALSE;
    if (g_str_has_prefix(uri, "about:") || g_str_has_prefix(uri, "data:") || g_str_has_prefix(uri, "blob:")) return TRUE;
    if (!g_str_has_prefix(uri, "http://") && !g_str_has_prefix(uri, "https://")) return FALSE;
    GUri *u = g_uri_parse(uri, G_URI_FLAGS_PARSE_RELAXED, NULL);
    if (!u) return FALSE;
    char *host = g_ascii_strdown(g_uri_get_host(u) ? g_uri_get_host(u) : "", -1);
    gboolean ok = G.n_allow == 0;
    for (int i = 0; i < G.n_allow && !ok; i++) {
        char *a = g_ascii_strdown(G.allow[i], -1);
        char *dot = g_strconcat(".", a, NULL);
        ok = !strcmp(a, "*") || !strcmp(a, host) || g_str_has_suffix(host, dot);
        g_free(a); g_free(dot);
    }
    g_free(host);
    g_uri_unref(u);
    return ok;
}

static gboolean on_policy(WebKitWebView *v, WebKitPolicyDecision *d, WebKitPolicyDecisionType t, gpointer ud) {
    (void)v; (void)ud;
    if (t == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION || t == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        WebKitNavigationAction *na = webkit_navigation_policy_decision_get_navigation_action(WEBKIT_NAVIGATION_POLICY_DECISION(d));
        const char *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(na));
        if (!host_allowed(uri)) {
            logf_("blocked navigation to %s", uri ? uri : "(none)");
            webkit_policy_decision_ignore(d);
            return TRUE;
        }
    } else if (t == WEBKIT_POLICY_DECISION_TYPE_RESPONSE) {
        if (!webkit_response_policy_decision_is_mime_type_supported(WEBKIT_RESPONSE_POLICY_DECISION(d))) {
            webkit_policy_decision_download(d);
            return TRUE;
        }
    }
    return FALSE;
}

static gboolean load_later(gpointer s) { webkit_web_view_load_uri(G.view, s); g_free(s); return G_SOURCE_REMOVE; }

/* window.open / target=_blank: stay in this view */
static WebKitWebView *on_create(WebKitWebView *v, WebKitNavigationAction *a, gpointer ud) {
    (void)v; (void)ud;
    const char *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(a));
    if (uri && strcmp(uri, "about:blank") && host_allowed(uri)) g_idle_add(load_later, g_strdup(uri));
    return NULL;
}

static void on_uri(void) {
    const char *u = webkit_web_view_get_uri(G.view);
    if (!u) u = "";
    if (!G.committed || strcmp(u, G.committed)) {
        g_free(G.committed);
        G.committed = g_strdup(u);
        send_str_msg(F_URL, u);
    }
    send_history();
}

static void on_title(void) { send_str_msg(F_TITLE, webkit_web_view_get_title(G.view)); }

static void on_progress(void) {
    int p = (int)(webkit_web_view_get_estimated_load_progress(G.view) * 100);
    send_bb(F_LOADING, webkit_web_view_is_loading(G.view) ? 1 : 0, p > 100 ? 100 : p);
}

static unsigned dev_sent;        /* developer-tools lines sent for this page load (capped, the page is untrusted) */
static char media_need_sent[8];   /* the last MediaUnsupported sent for this page ("" before any) */

static void permission_deny_all(void);

static void on_load_changed(WebKitWebView *v, WebKitLoadEvent e, gpointer ud) {
    (void)v; (void)ud;
    if (e == WEBKIT_LOAD_STARTED) permission_deny_all();   /* a question about the old page is void */
    if (e == WEBKIT_LOAD_FINISHED) send_bb(F_LOADING, 0, 100);
    else if (e == WEBKIT_LOAD_STARTED) { media_need_sent[0] = 0; dev_sent = 0; send_bb(F_LOADING, 1, 0); }
    send_history();
}

static gboolean on_load_failed(WebKitWebView *v, WebKitLoadEvent e, const char *uri, GError *err, gpointer ud) {
    (void)v; (void)e; (void)ud;
    logf_("load failed: %s: %s", uri, err->message);
    send_bb(F_LOADING, 0, 100);
    return FALSE;   /* the engine's own error page */
}

static gboolean on_load_failed_tls(WebKitWebView *v, const char *uri, GTlsCertificate *c, GTlsCertificateFlags f, gpointer ud) {
    (void)v; (void)c; (void)ud;
    logf_("certificate refused for %s (flags %#x)", uri, (unsigned)f);
    return FALSE;
}

static void on_target(WebKitWebView *v, WebKitHitTestResult *hit, guint mods, gpointer ud) {
    (void)v; (void)mods; (void)ud;
    int kind = CUR_DEFAULT;
    if (webkit_hit_test_result_context_is_link(hit)) kind = CUR_POINTER;
    else if (webkit_hit_test_result_context_is_editable(hit) || webkit_hit_test_result_context_is_selection(hit)) kind = CUR_TEXT;
    if (kind != G.cursor) {
        G.cursor = kind;
        Buf b = {0}; buf_u8(&b, kind); send_msg(F_CURSOR, &b); buf_free(&b);
    }
}

static gboolean reload_later(gpointer v) { webkit_web_view_reload(v); return G_SOURCE_REMOVE; }

/* Crash-loop guard: a second crash within 60 s means reloading the same page will crash again (seen with a camera/microphone page on a
 * sound card whose capture device cannot be inspected). Stop reloading, say so, and keep the helper alive so the shell can show a button. */
#define CRASH_WINDOW_US (60 * G_USEC_PER_SEC)
#define CRASH_LIMIT 2
static gint64 crash_times[CRASH_LIMIT];
static int crash_n;
static uint32_t last_media_kinds;     /* the last camera/microphone/screen request, for the log */
static char last_media_origin[160];

/* true when this crash is the CRASH_LIMIT-th inside the window (pure: testable with fake times) */
static gboolean crash_loop(gint64 now_us) {
    if (crash_n == CRASH_LIMIT) { memmove(crash_times, crash_times + 1, sizeof(crash_times[0]) * (CRASH_LIMIT - 1)); crash_n--; }
    crash_times[crash_n++] = now_us;
    return crash_n == CRASH_LIMIT && now_us - crash_times[0] <= CRASH_WINDOW_US;
}

static void on_crash(WebKitWebView *v, WebKitWebProcessTerminationReason r, gpointer ud) {
    (void)ud;
    G.crashes++;
    permission_deny_all();
    gboolean media = last_media_kinds != 0;
    gboolean loop = crash_loop(g_get_monotonic_time());
    logf_("web process crash %d (reason %d); last media request: kinds=0x%x origin=%s; %s", G.crashes, (int)r, last_media_kinds,
          last_media_origin[0] ? last_media_origin : "(none)", loop ? "crash loop, not reloading" : "reloading");
    const char *why = r == WEBKIT_WEB_PROCESS_EXCEEDED_MEMORY_LIMIT ? "the web process used too much memory" : "the web process stopped";
    if (loop && media) why = "The page's process stopped (media device problem)";
    else if (loop) why = "The page's process stopped again and again";
    /* always on stderr as well, not only with the debug switch: a crash must be in the system log */
    fprintf(stderr, "wpe-helper: web process crash %d (reason %d: %s); %s\n", G.crashes, (int)r, why, loop ? "crash loop, not reloading" : "reloading");
    fflush(stderr);
    send_str_msg(F_CRASHED, why);
    if (!loop) g_timeout_add(500, reload_later, v);
}

/* Camera / microphone / screen requests (docs/development/BROWSER-MEDIA-DEVICES.md): the person decides, in the shell. Every other kind of
 * permission (location, notifications, ...) is denied here. The request waits for the shell's PermissionAnswer; silence, a full queue,
 * a crash or a navigation mean "deny". */
#define PERMISSION_TIMEOUT_MS 60000
#define MAX_PENDING_PERMISSIONS 4
typedef struct { uint32_t id; WebKitPermissionRequest *req; guint timer; } PendingPerm;
static GPtrArray *pending_perms;
static uint32_t perm_next = 1;

static void pending_permission_answer_allow(WebKitPermissionRequest *req);

static void perm_finish(PendingPerm *p, gboolean allow) {
    if (p->timer) g_source_remove(p->timer);
    if (allow) pending_permission_answer_allow(p->req);
    else webkit_permission_request_deny(p->req);
    g_object_unref(p->req);
    g_free(p);
}

static gboolean perm_timeout(gpointer data) {
    PendingPerm *p = data;
    p->timer = 0;
    g_ptr_array_remove(pending_perms, p);
    logf_("permission request %u: no answer in time, denied", p->id);
    perm_finish(p, FALSE);
    return G_SOURCE_REMOVE;
}

/* the one place that allows: only reached from the shell's PermissionAnswer (never from the page) */
static void pending_permission_answer_allow(WebKitPermissionRequest *req) { webkit_permission_request_allow(req); }

static void permission_answer(uint32_t id, int allow) {
    if (!pending_perms) return;
    for (guint i = 0; i < pending_perms->len; i++) {
        PendingPerm *p = g_ptr_array_index(pending_perms, i);
        if (p->id == id) {
            g_ptr_array_remove_index(pending_perms, i);
            perm_finish(p, allow != 0);
            return;
        }
    }
}

static void permission_deny_all(void) {
    while (pending_perms && pending_perms->len) {
        PendingPerm *p = g_ptr_array_remove_index(pending_perms, pending_perms->len - 1);
        perm_finish(p, FALSE);
    }
}

static int mic_plugin_on;
/* One line for the system log each time a page asks for the microphone: what the scan finds and what the provider will use. */
static void log_microphone_request(void) {
    const char *p = getenv("IYAKKI_MIC_PROC"), *sy = getenv("IYAKKI_MIC_SYS");
    IyakkiMicScan *sc = micscan_run(p && *p ? p : "/proc/asound", sy && *sy ? sy : "/sys", TRUE);
    char *r = micscan_report(sc, mic_plugin_on ? "requested by a page" : "requested by a page (microphone plugin not active: stock ALSA path)");
    fprintf(stderr, "wpe-helper: iyakki-mic: %s\n", r);   /* stderr reaches the system log whole; the shell log line is cut at 512 bytes */
    logf_("%s", r);
    g_free(r);
    micscan_free(sc);
}

static gboolean on_permission(WebKitWebView *v, WebKitPermissionRequest *req, gpointer ud) {
    (void)ud;
    uint8_t kinds = 0;
    if (WEBKIT_IS_USER_MEDIA_PERMISSION_REQUEST(req)) {
        WebKitUserMediaPermissionRequest *um = WEBKIT_USER_MEDIA_PERMISSION_REQUEST(req);
        if (webkit_user_media_permission_is_for_audio_device(um)) { kinds |= MK_MICROPHONE; log_microphone_request(); }
        if (webkit_user_media_permission_is_for_video_device(um)) kinds |= MK_CAMERA;
        if (webkit_user_media_permission_is_for_display_device(um)) kinds |= MK_SCREEN;
    } else if (WEBKIT_IS_DEVICE_INFO_PERMISSION_REQUEST(req)) {
        kinds = MK_DEVICE_LIST;
    }
    /* a screen share needs a screen source the engine can read (docs/development/BROWSER-MEDIA-DEVICES.md, "Screen share"): only with
     * the engine's mock display (IYAKKI_WEBVIEW_MOCK_MEDIA=1) or when the image says it has one (IYAKKI_WEBVIEW_SCREEN=1); else no question */
    if ((kinds & MK_SCREEN) && !getenv("IYAKKI_WEBVIEW_SCREEN") && !getenv("IYAKKI_WEBVIEW_MOCK_MEDIA")) kinds = 0;
    if (!kinds || !G.ready) { webkit_permission_request_deny(req); return TRUE; }
    last_media_kinds = kinds;
    { const char *uri = G.view ? webkit_web_view_get_uri(G.view) : NULL; g_strlcpy(last_media_origin, uri ? uri : "", sizeof last_media_origin); }
    if (!pending_perms) pending_perms = g_ptr_array_new();
    if (pending_perms->len >= MAX_PENDING_PERMISSIONS) { webkit_permission_request_deny(req); return TRUE; }
    PendingPerm *p = g_new0(PendingPerm, 1);
    p->id = perm_next++;
    p->req = g_object_ref(req);
    p->timer = g_timeout_add(PERMISSION_TIMEOUT_MS, perm_timeout, p);
    g_ptr_array_add(pending_perms, p);
    Buf b = {0};
    buf_u32(&b, p->id);
    buf_u8(&b, kinds);
    buf_str(&b, webkit_web_view_get_uri(v));   /* the top-level page the person sees, also for a request from a frame in it */
    send_msg(F_PERMISSION_REQUEST, &b);
    buf_free(&b);
    return TRUE;
}

/* the shell's "stop sharing" click: NONE ends the capture in the engine, which then sends the new state */
static void stop_capture(uint8_t kinds) {
    if (!G.view) return;
    if (kinds & MK_MICROPHONE) webkit_web_view_set_microphone_capture_state(G.view, WEBKIT_MEDIA_CAPTURE_STATE_NONE);
    if (kinds & MK_CAMERA) webkit_web_view_set_camera_capture_state(G.view, WEBKIT_MEDIA_CAPTURE_STATE_NONE);
    if (kinds & MK_SCREEN) webkit_web_view_set_display_capture_state(G.view, WEBKIT_MEDIA_CAPTURE_STATE_NONE);
}

static char *screen_path;   /* the file the shell writes its screen into while a screen share is on (read by the screen feeder, when there is one) */
static void screen_source(const char *path) {
    g_free(screen_path);
    screen_path = (path && *path) ? g_strdup(path) : NULL;
    logf_("screen source: %s", screen_path ? screen_path : "(none)");
}

static void on_capture_state(GObject *o, GParamSpec *ps, gpointer ud) {
    (void)o; (void)ps; (void)ud;
    uint8_t k = 0;
    if (webkit_web_view_get_microphone_capture_state(G.view) == WEBKIT_MEDIA_CAPTURE_STATE_ACTIVE) k |= MK_MICROPHONE;
    if (webkit_web_view_get_camera_capture_state(G.view) == WEBKIT_MEDIA_CAPTURE_STATE_ACTIVE) k |= MK_CAMERA;
    if (webkit_web_view_get_display_capture_state(G.view) == WEBKIT_MEDIA_CAPTURE_STATE_ACTIVE) k |= MK_SCREEN;
    Buf b = {0};
    buf_u8(&b, k);
    send_msg(F_CAPTURE_STATE, &b);
    buf_free(&b);
}

static gboolean on_script_dialog(WebKitWebView *v, WebKitScriptDialog *d, gpointer ud) {
    (void)v; (void)ud;
    /* no dialogs: alerts are acknowledged, confirms and prompts answered "no" */
    switch (webkit_script_dialog_get_dialog_type(d)) {
    case WEBKIT_SCRIPT_DIALOG_CONFIRM:
    case WEBKIT_SCRIPT_DIALOG_BEFORE_UNLOAD_CONFIRM: webkit_script_dialog_confirm_set_confirmed(d, FALSE); break;
    default: break;
    }
    return TRUE;
}

static void download_report(WebKitDownload *dl, const char *name) {
    if (g_object_get_data(G_OBJECT(dl), "reported")) return;
    g_object_set_data(G_OBJECT(dl), "reported", GINT_TO_POINTER(1));
    WebKitURIRequest *rq = webkit_download_get_request(dl);
    Buf b = {0};
    buf_str(&b, webkit_uri_request_get_uri(rq));
    buf_str(&b, name ? name : "");
    send_msg(F_DOWNLOAD, &b);
    buf_free(&b);
    webkit_download_cancel(dl);   /* the shell decides what to save, and where */
}

static gboolean on_decide_destination(WebKitDownload *dl, const char *suggested, gpointer ud) {
    (void)ud;
    download_report(dl, suggested);
    return TRUE;
}

static void on_download_started(WebKitNetworkSession *s, WebKitDownload *dl, gpointer ud) {
    (void)s; (void)ud;
    g_signal_connect(dl, "decide-destination", G_CALLBACK(on_decide_destination), NULL);
}

/* ---------------------------------------------------------------------------------------------- input */
static uint32_t now_ms(void) { return (uint32_t)(g_get_monotonic_time() / 1000) & 0x7fffffff; }

static uint32_t wpe_mods(uint32_t m) {
    uint32_t r = 0;
    if (m & 1) r |= wpe_input_keyboard_modifier_shift;
    if (m & 2) r |= wpe_input_keyboard_modifier_control;
    if (m & 4) r |= wpe_input_keyboard_modifier_alt;
    if (m & 8) r |= wpe_input_keyboard_modifier_meta;
    return r;
}

static uint32_t button_mods(uint32_t held) {
    uint32_t r = 0;
    if (held & 1) r |= wpe_input_pointer_modifier_button1;
    if (held & 2) r |= wpe_input_pointer_modifier_button3;   /* WPE numbers buttons like Wayland: 1 left, 2 right, 3 middle */
    if (held & 4) r |= wpe_input_pointer_modifier_button2;
    return r;
}

static int css(int v) { return (int)((int64_t)v * 100 / (int)G.scale); }

static void on_mouse(uint8_t kind, int x, int y, uint8_t button, uint8_t held, uint32_t mods) {
    struct wpe_input_pointer_event e = {0};
    e.time = now_ms();
    /* WPE pointer coordinates are device pixels (WebKit divides them by the device scale factor itself): no css() here */
    e.x = x; e.y = y;
    e.modifiers = wpe_mods(mods) | button_mods(held);
    if (kind == 0) {
        e.type = wpe_input_pointer_event_type_motion;
    } else {
        e.type = wpe_input_pointer_event_type_button;
        e.button = button == 2 ? 3 : button == 3 ? 2 : 1;
        e.state = kind == 1;
    }
    if (G.debug && kind != 0) logf_("mouse kind %d at %d,%d button %d state %u mods %#x", kind, e.x, e.y, e.button, e.state, e.modifiers);
    wpe_view_backend_dispatch_pointer_event(G.backend, &e);
}

static void on_wheel(int x, int y, int dx, int dy, uint32_t mods) {
    struct wpe_input_axis_2d_event e = {0};
    e.base.type = wpe_input_axis_event_type_mask_2d | wpe_input_axis_event_type_motion_smooth;
    e.base.time = now_ms();
    e.base.x = x; e.base.y = y;   /* device pixels, as for the pointer */
    e.base.modifiers = wpe_mods(mods);
    /* 120 = one notch = about 53 CSS px; WPE counts scroll-down as negative */
    e.x_axis = -(double)dx * 53.0 / 120.0;
    e.y_axis = -(double)dy * 53.0 / 120.0;
    wpe_view_backend_dispatch_axis_event(G.backend, &e.base);
}

static void report_selection_done(GObject *src, GAsyncResult *res, gpointer ud) {
    (void)ud;
    JSCValue *val = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(src), res, NULL);
    if (val) {
        if (jsc_value_is_string(val)) {
            char *s = jsc_value_to_string(val);
            if (s && *s) send_str_msg(F_CLIPBOARD, s);
            g_free(s);
        }
        g_object_unref(val);
    }
}

static gboolean report_selection(gpointer unused) {
    (void)unused;
    static const char js[] = "(function(){var a=document.activeElement;if(a&&a.selectionStart!=null&&a.selectionEnd>a.selectionStart){return a.value.substring(a.selectionStart,a.selectionEnd)}return String(window.getSelection())})()";
    webkit_web_view_evaluate_javascript(G.view, js, -1, NULL, NULL, NULL, report_selection_done, NULL);
    return G_SOURCE_REMOVE;
}

static void on_key(int down, uint32_t keysym, uint32_t mods) {
    if (!G.focused) return;
    struct wpe_input_keyboard_event e = {0};
    e.time = now_ms();
    e.key_code = keysym;
    e.hardware_key_code = 0;
    e.pressed = down;
    e.modifiers = wpe_mods(mods);
    wpe_view_backend_dispatch_keyboard_event(G.backend, &e);
    if (down && (mods & 2) && (keysym == 'c' || keysym == 'C' || keysym == 'x' || keysym == 'X')) g_timeout_add(150, report_selection, NULL);
}


/* ---------------------------------------------------------------------------------------------- clipboard bridge
 * WPE has no system clipboard here: a user script makes navigator.clipboard.writeText (a page's "Copy" button, e.g. a sign-in code)
 * and copy/cut events hand the text to the shell (F_CLIPBOARD), which keeps IYAKKI's one clipboard. Paste goes the other way:
 * the shell sends T_TEXT. */
static const char CLIP_JS[] =
    "(function(){try{var post=function(t){try{window.webkit.messageHandlers.iyakkiClip.postMessage(String(t))}catch(e){}};"
    "var sel=function(){var a=document.activeElement;if(a&&a.selectionStart!=null&&a.selectionEnd>a.selectionStart)return a.value.substring(a.selectionStart,a.selectionEnd);return String(window.getSelection())};"
    "var cb={writeText:function(t){post(t);return Promise.resolve()},readText:function(){return Promise.resolve(window.__iyakkiClip||'')},"
    "write:function(items){try{var it=items&&items[0];if(it&&it.getType){it.getType('text/plain').then(function(b){return b.text()}).then(post)}}catch(e){}return Promise.resolve()}};"
    "try{Object.defineProperty(navigator,'clipboard',{value:cb,configurable:true})}catch(e){}"
    "var f=function(){setTimeout(function(){var t=sel();if(t)post(t)},0)};document.addEventListener('copy',f,true);document.addEventListener('cut',f,true);}catch(e){}})();";

static void on_clip_message(WebKitUserContentManager *m, JSCValue *val, gpointer ud) {
    (void)m; (void)ud;
    if (!val || !jsc_value_is_string(val)) return;
    char *t = jsc_value_to_string(val);
    if (t && *t) send_str_msg(F_CLIPBOARD, t);
    g_free(t);
}

/* ---------------------------------------------------------------------------------------------- media probe
 * media-probe.js (a user script, built into media-probe.h by the Makefile) posts "video", "audio" or "both" to the iyakkiMedia message
 * handler when a page's media fails for want of an H.264 / AAC decoder and the engine's canPlayType agrees. Only those three words are
 * passed on, once per page load and word (the page is untrusted: nothing else it posts reaches the shell). */
static const char MEDIA_JS[] =
#include "media-probe.h"
;

static void on_media_message(WebKitUserContentManager *m, JSCValue *val, gpointer ud) {
    (void)m; (void)ud;
    if (!val || !jsc_value_is_string(val)) return;
    char *t = jsc_value_to_string(val);
    if (t && (!strcmp(t, "video") || !strcmp(t, "audio") || !strcmp(t, "both")) && strcmp(t, media_need_sent)) {
        snprintf(media_need_sent, sizeof media_need_sent, "%s", t);
        send_str_msg(F_MEDIA_UNSUPPORTED, t);
    }
    g_free(t);
}

/* ---------------------------------------------------------------------------------------------- developer-tools probe
 * dev-probe.js (a user script, built into dev-probe.h by the Makefile) posts one text line per failed request, slow resource, console
 * error/warning or page timing to the iyakkiDev message handler. The line is passed on as it is (the shell parses and scrubs it and
 * keeps it only when the person turned developer tools for the chat agent on), but the page is untrusted: only strings of at most
 * 1000 bytes with a known kind letter and U+001F separators are forwarded, and at most DEV_MAX_PER_PAGE per page load. */
#define DEV_MAX_PER_PAGE 400
static const char DEV_JS[] =
#include "dev-probe.h"
;

static void on_dev_message(WebKitUserContentManager *m, JSCValue *val, gpointer ud) {
    (void)m; (void)ud;
    if (!val || !jsc_value_is_string(val) || dev_sent >= DEV_MAX_PER_PAGE) return;
    char *t = jsc_value_to_string(val);
    if (t && t[0] && strlen(t) <= 1000 && strchr("FRCPH", t[0]) && t[1] == '\x1f') {
        dev_sent++;
        send_str_msg(F_DEV_EVENT, t);
    }
    g_free(t);
}

static void on_text(const char *text) {
    GString *js = g_string_new("document.execCommand('insertText',false,\"");
    for (const char *p = text; *p; p = g_utf8_next_char(p)) {
        gunichar c = g_utf8_get_char(p);
        if (c == '"' || c == '\\') g_string_append_printf(js, "\\%c", (char)c);
        else if (c < 0x20) g_string_append_printf(js, "\\u%04x", c);
        else if (c > 0xffff) { c -= 0x10000; g_string_append_printf(js, "\\u%04x\\u%04x", 0xd800 + (c >> 10), 0xdc00 + (c & 0x3ff)); }
        else if (c > 0x7e) g_string_append_printf(js, "\\u%04x", c);
        else g_string_append_c(js, (char)c);
    }
    g_string_append(js, "\")");
    webkit_web_view_evaluate_javascript(G.view, js->str, -1, NULL, NULL, NULL, NULL, NULL);
    g_string_free(js, TRUE);
}

/* Protocol 1.1 Script: evaluate JavaScript in the top-level page, answer ScriptResult{id, ok, json}. The value is turned into JSON text
 * with jsc_value_to_json (undefined and null become "null"). A timer answers "script timed out" if the page never finishes. */
typedef struct { uint32_t id; int done; int refs; } ScriptJob;

static void script_unref(ScriptJob *j) { if (--j->refs == 0) g_free(j); }

static void send_script_result(ScriptJob *j, int ok, const char *text) {
    if (j->done) return;
    j->done = 1;
    Buf b = {0};
    buf_u32(&b, j->id); buf_u8(&b, (uint8_t)ok); buf_str(&b, text);
    send_msg(F_SCRIPT_RESULT, &b);
    buf_free(&b);
}

static void script_done(GObject *src, GAsyncResult *res, gpointer ud) {
    ScriptJob *j = ud;
    GError *err = NULL;
    JSCValue *val = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(src), res, &err);
    if (!val) {
        send_script_result(j, 0, err && err->message ? err->message : "script failed");
    } else {
        if (jsc_value_is_undefined(val) || jsc_value_is_null(val)) {
            send_script_result(j, 1, "null");
        } else {
            char *json = jsc_value_to_json(val, 0);
            send_script_result(j, 1, json ? json : "null");
            g_free(json);
        }
        g_object_unref(val);
    }
    if (err) g_error_free(err);
    script_unref(j);   /* the timer holds the other reference */
}

static gboolean script_timeout(gpointer ud) {
    ScriptJob *j = ud;
    send_script_result(j, 0, "script timed out");
    script_unref(j);
    return G_SOURCE_REMOVE;
}

static void on_script(uint32_t id, const char *js, uint32_t timeout_ms) {
    ScriptJob *j = g_new0(ScriptJob, 1);
    j->id = id;
    j->refs = 2;
    if (timeout_ms < 100) timeout_ms = 10000;
    if (timeout_ms > 120000) timeout_ms = 120000;
    g_timeout_add(timeout_ms, script_timeout, j);   /* drops one reference */
    webkit_web_view_evaluate_javascript(G.view, js, -1, NULL, NULL, NULL, script_done, j);
}

/* DOM fullscreen (YouTube's button, F, double click). WPE asks the backend to "fullscreen the window" through this handler; with none
 * set WebKit treats the request as failed and leaves fullscreen again at once (the player jumps back to the mini player). The shell
 * owns the screen, so the handler only tells it (Fullscreen{on}); it then sends Resize for the whole page area. Refused while the
 * tab is hidden, so the page never ends up half in fullscreen. Leaving comes from the page (button, Esc in the page, navigation)
 * through this same handler with enable false, or from the shell (LeaveFullscreen). */
static void send_fullscreen(int on) { Buf b = {0}; buf_u8(&b, on ? 1 : 0); send_msg(F_FULLSCREEN, &b); buf_free(&b); }

/* Sound doctor (docs/WEBVIEW-PROTOCOL.md, AudioPlaying): the shell compares "the page plays audio" with the playback streams the engine's
 * processes hold open and reconnects the sound when the page is loud but nothing holds the card. Only reports changes. */
static void send_audio_playing(int on) { Buf b = {0}; buf_u8(&b, on ? 1 : 0); send_msg(F_AUDIO_PLAYING, &b); buf_free(&b); }
static void on_audio_playing(void) {
    static int last = -1;
    int on = webkit_web_view_is_playing_audio(G.view) ? 1 : 0;
    if (on == last) return;
    last = on;
    send_audio_playing(on);
}

static bool fullscreen_handler(void *ud, bool enable) {
    (void)ud;
    if (enable == (G.fullscreen != 0)) return true;
    if (enable && !G.visible) { logf_("fullscreen refused: the tab is hidden"); return false; }
    G.fullscreen = enable ? 1 : 0;
    send_fullscreen(G.fullscreen);
    return true;
}

static void leave_fullscreen(void) {
    if (!G.fullscreen) return;
    wpe_view_backend_dispatch_request_exit_fullscreen(G.backend);
    /* belt and braces for a page that did not take the request: the script path of the same thing */
    webkit_web_view_evaluate_javascript(G.view, "if(document.fullscreenElement&&document.exitFullscreen)document.exitFullscreen();", -1, NULL, NULL, NULL, NULL, NULL);
}

static void apply_zoom(void) { webkit_web_view_set_zoom_level(G.view, G.zoom / 100.0); }

static void set_activity(void) {
    uint32_t want = (G.visible ? wpe_view_activity_state_visible | wpe_view_activity_state_in_window : 0) | (G.focused && G.visible ? wpe_view_activity_state_focused : 0);
    uint32_t cur = wpe_view_backend_get_activity_state(G.backend);
    wpe_view_backend_add_activity_state(G.backend, want & ~cur);
    wpe_view_backend_remove_activity_state(G.backend, cur & ~want);
}

static void apply_size(void) {
    wpe_view_backend_dispatch_set_size(G.backend, (uint32_t)css((int)G.w), (uint32_t)css((int)G.h));
}

/* ---------------------------------------------------------------------------------------------- content blocking
 * The shell compiles the person's filter lists into WebKit's rule-list JSON (iyakki-web blocker_json) and names the file in
 * IYAKKI_WEBVIEW_FILTER, with a content hash in IYAKKI_WEBVIEW_FILTER_ID. The compiled form is kept by WebKit's filter store
 * under the profile, so only the first helper after a list change pays for compiling; the others load it by name. The first
 * page waits (at most 20 seconds) so nothing loads unfiltered. Any failure is logged and the engine runs without a filter:
 * blocking fails open, never closed. */
static void filter_done(void) {
    if (!G.filter_wait) return;
    G.filter_wait = 0;
    if (G.pending_nav && G.view) webkit_web_view_load_uri(G.view, G.pending_nav);
    g_free(G.pending_nav);
    G.pending_nav = NULL;
}

static gboolean filter_timeout(gpointer d) { logf_("content filter: gave up waiting"); filter_done(); return G_SOURCE_REMOVE; }

static void filter_use(WebKitUserContentFilter *f, const char *how) {
    webkit_user_content_manager_add_filter(G.ucm, f);
    webkit_user_content_filter_unref(f);
    logf_("content filter %s", how);
    filter_done();
}

static void filter_saved(GObject *src, GAsyncResult *res, gpointer ud) {
    GError *err = NULL;
    WebKitUserContentFilter *f = webkit_user_content_filter_store_save_from_file_finish(WEBKIT_USER_CONTENT_FILTER_STORE(src), res, &err);
    if (!f) { logf_("content filter not compiled: %s", err ? err->message : "unknown"); g_clear_error(&err); filter_done(); return; }
    filter_use(f, "compiled and loaded");
}

static void filter_loaded(GObject *src, GAsyncResult *res, gpointer ud) {
    GError *err = NULL;
    WebKitUserContentFilter *f = webkit_user_content_filter_store_load_finish(WEBKIT_USER_CONTENT_FILTER_STORE(src), res, &err);
    g_clear_error(&err);
    if (f) { filter_use(f, "loaded from the store"); return; }
    const char *path = getenv("IYAKKI_WEBVIEW_FILTER"), *id = getenv("IYAKKI_WEBVIEW_FILTER_ID");
    GFile *file = g_file_new_for_path(path);
    webkit_user_content_filter_store_save_from_file(WEBKIT_USER_CONTENT_FILTER_STORE(src), id, file, NULL, filter_saved, NULL);
    g_object_unref(file);
}

static void setup_filter(void) {
    const char *path = getenv("IYAKKI_WEBVIEW_FILTER"), *id = getenv("IYAKKI_WEBVIEW_FILTER_ID");
    if (!path || !*path || !id || !*id || !G.ucm || access(path, R_OK) != 0) return;
    for (const char *c = id; *c; c++) if (!g_ascii_isalnum(*c) && *c != '-' && *c != '_') { logf_("content filter: bad id"); return; }
    char *store_dir = g_build_filename(G.profile && *G.profile ? G.profile : g_get_tmp_dir(), "content-filter-store", NULL);
    WebKitUserContentFilterStore *store = webkit_user_content_filter_store_new(store_dir);
    g_free(store_dir);
    G.filter_wait = 1;
    g_timeout_add_seconds(20, filter_timeout, NULL);
    webkit_user_content_filter_store_load(store, id, NULL, filter_loaded, NULL);
    /* the store object stays alive until the callbacks have run (GIO holds a reference for the pending operation) */
}

/* ---------------------------------------------------------------------------------------------- messages */
static void handle(uint8_t ty, const uint8_t *p, size_t n) {
    Rd r = { .p = p, .n = n };
    if (!G.view) return;
    switch (ty) {
    case T_NAVIGATE: {
        char *u = rd_str(&r);
        if (!host_allowed(u)) logf_("refused to open %s", u);
        else if (G.filter_wait) { g_free(G.pending_nav); G.pending_nav = g_strdup(u); }   /* opened when the filter is in */
        else webkit_web_view_load_uri(G.view, u);
        g_free(u);
        break;
    }
    case T_RELOAD: if (rd_u8(&r)) webkit_web_view_reload_bypass_cache(G.view); else webkit_web_view_reload(G.view); break;
    case T_STOP: webkit_web_view_stop_loading(G.view); break;
    case T_BACK: if (webkit_web_view_can_go_back(G.view)) webkit_web_view_go_back(G.view); break;
    case T_FORWARD: if (webkit_web_view_can_go_forward(G.view)) webkit_web_view_go_forward(G.view); break;
    case T_RESIZE: {
        uint32_t w = rd_u32(&r), h = rd_u32(&r);
        G.w = w < 1 ? 1 : w > G.max_w ? G.max_w : w; if (G.w > MAX_SCREEN_W) G.w = MAX_SCREEN_W;
        G.h = h < 1 ? 1 : h > G.max_h ? G.max_h : h; if (G.h > MAX_SCREEN_H) G.h = MAX_SCREEN_H;
        apply_size();
        G.need_full = TRUE;
        break;
    }
    case T_MOUSE: {
        uint8_t kind = rd_u8(&r); int x = rd_i32(&r), y = rd_i32(&r); uint8_t b = rd_u8(&r), held = rd_u8(&r); uint32_t m = rd_u32(&r);
        on_mouse(kind, x, y, b, held, m);
        break;
    }
    case T_WHEEL: {
        int x = rd_i32(&r), y = rd_i32(&r), dx = rd_i32(&r), dy = rd_i32(&r); uint32_t m = rd_u32(&r);
        on_wheel(x, y, dx, dy, m);
        break;
    }
    case T_KEY: {
        uint8_t down = rd_u8(&r); uint32_t ks = rd_u32(&r), m = rd_u32(&r);
        on_key(down, ks, m);
        break;
    }
    case T_TEXT: { char *t = rd_str(&r); if (*t) on_text(t); g_free(t); break; }
    case T_ZOOM: { uint32_t z = rd_u32(&r); G.zoom = z < 25 ? 25 : z > 500 ? 500 : z; apply_zoom(); break; }
    case T_FOCUS: G.focused = rd_u8(&r) != 0; set_activity(); break;
    case T_VISIBLE: G.visible = rd_u8(&r) != 0; set_activity(); if (G.visible) mark_all(); else leave_fullscreen(); break;
    case T_ACK: {
        uint8_t s = rd_u8(&r);
        G.nacks++;
        if (s < 2) G.busy[s] = FALSE;
        if (G.back_new) schedule_flush();
        break;
    }
    case T_SCRIPT: { uint32_t id = rd_u32(&r), tmo = rd_u32(&r); char *js = rd_str(&r); if (!r.bad) on_script(id, js, tmo); g_free(js); break; }
    case T_LEAVE_FULLSCREEN: leave_fullscreen(); break;
    case T_STOP_CAPTURE: { uint8_t kinds = rd_u8(&r); if (!r.bad) stop_capture(kinds); break; }
    case T_SCREEN_SOURCE: { char *p = rd_str(&r); if (!r.bad) screen_source(p); g_free(p); break; }
    case T_PERMISSION_ANSWER: { uint32_t id = rd_u32(&r); uint8_t allow = rd_u8(&r); if (!r.bad) permission_answer(id, allow); break; }
    case T_QUIT: quit_loop(); break;
    default: break;   /* unknown: skipped */
    }
}

static void feed(const uint8_t *chunk, size_t n) {
    g_byte_array_append(G.in, chunk, (guint)n);
    while (G.in->len >= 4) {
        uint32_t len;
        memcpy(&len, G.in->data, 4);
        if (len == 0 || len > (4u << 20)) { fprintf(stderr, "wpe-helper: impossible message length %u\n", len); quit_loop(); return; }
        if (G.in->len < 4 + len) break;
        uint8_t ty = G.in->data[4];
        uint8_t *payload = g_memdup2(G.in->data + 4 + 1, len - 1 ? len - 1 : 1);
        g_byte_array_remove_range(G.in, 0, 4 + len);
        handle(ty, payload, len - 1);
        g_free(payload);
    }
}

static gboolean on_readable(gint fd, GIOCondition cond, gpointer ud) {
    (void)ud; (void)fd;
    uint8_t buf[65536];
    if (cond & G_IO_IN) {
        ssize_t r = read(G.sock, buf, sizeof buf);
        if (r > 0) { feed(buf, (size_t)r); return G_SOURCE_CONTINUE; }
        if (r < 0 && (errno == EAGAIN || errno == EINTR)) return G_SOURCE_CONTINUE;
    }
    quit_loop();     /* EOF, hang-up or error: the shell is gone */
    return G_SOURCE_REMOVE;
}

/* ---------------------------------------------------------------------------------------------- start */
static gboolean read_hello(void) {
    uint8_t buf[65536];
    for (;;) {
        if (G.in->len >= 4) {
            uint32_t len;
            memcpy(&len, G.in->data, 4);
            if (len == 0 || len > (4u << 20)) { fprintf(stderr, "wpe-helper: impossible message length %u\n", len); exit(2); }
            if (G.in->len >= 4 + len) {
                if (G.in->data[4] != T_HELLO) { fprintf(stderr, "wpe-helper: the first message was not Hello\n"); exit(2); }
                Rd r = { .p = G.in->data + 5, .n = len - 1 };
                uint32_t ver = rd_u32(&r);
                if (ver != PROTOCOL_VERSION) { fprintf(stderr, "wpe-helper: unknown protocol version %u\n", ver); exit(2); }
                G.w = rd_u32(&r); G.h = rd_u32(&r); G.scale = rd_u32(&r); G.max_w = rd_u32(&r); G.max_h = rd_u32(&r);
                G.ua = rd_str(&r); G.profile = rd_str(&r); G.locale = rd_str(&r);
                uint32_t n = rd_u32(&r);
                if (n > 4096) n = 4096;
                G.allow = g_new0(char *, n + 1);
                for (uint32_t i = 0; i < n && !r.bad; i++) G.allow[G.n_allow++] = rd_str(&r);
                g_byte_array_remove_range(G.in, 0, 4 + len);
                if (G.w < 1) G.w = 1;
                if (G.h < 1) G.h = 1;
                if (G.scale < 50) G.scale = 100;
                if (G.max_w < 1) G.max_w = 1;
                if (G.max_h < 1) G.max_h = 1;
                if (G.w > G.max_w) G.w = G.max_w;
                if (G.h > G.max_h) G.h = G.max_h;
                return TRUE;
            }
        }
        ssize_t r = read(G.sock, buf, sizeof buf);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return FALSE;
        g_byte_array_append(G.in, buf, (guint)r);
    }
}

static void setup_engine(void) {
    /* the web process finds the same backend library through the environment */
    setenv("WPE_BACKEND_LIBRARY", "libWPEBackend-fdo-1.0.so.1", 0);
    /* GStreamer errors and warnings, and what the audio sinks do when they open the card, reach the web process's stderr (the shell keeps
     * the tail and the agent log shows it): the evidence a silent session needs. The owner can set GST_DEBUG to change it. */
    setenv("GST_DEBUG", "*:1,alsasink:4,autoaudiosink:4", 0);
    setenv("GST_DEBUG_NO_COLOR", "1", 0);
    /* which sound configuration the engine was given (the agent sets ALSA_CONFIG_PATH on a T2): one line in the log, so a silent or
     * frozen page can be traced to the set-up it really used */
    {
        const char *alsa_conf = getenv("ALSA_CONFIG_PATH");
        fprintf(stderr, "wpe-helper: sound: ALSA_CONFIG_PATH=%s\n", alsa_conf && *alsa_conf ? alsa_conf : "(unset)");
    }
    /* Diagnosis switch: while the file /tmp/iyakki-dev/gst-debug exists (a paired developer computer puts it there), the camera, the
     * WebRTC and the media-stream pieces log in detail to /tmp/iyakki-dev/gst-debug.log (`iyakki dev files pull`) instead of the tail. */
    if (access("/tmp/iyakki-dev/gst-debug", F_OK) == 0) {
        setenv("GST_DEBUG", "*:2,alsasink:6,autoaudiosink:5,audiobasesink:5,v4l2*:5,v4l2src:6,webkit*:5,webrtc*:4,GST_CAPS:4,GST_STATES:3", 1);
        setenv("GST_DEBUG_FILE", "/tmp/iyakki-dev/gst-debug.log", 1);
    }
    /* The engine's media device monitor probes every GStreamer device provider. On the T2 MacBook the ALSA provider cannot open
     * hw:0,1 and the web process then dereferences a NULL GstObject (g_mutex_lock at 0x18) and crashes. Demote it to rank 0 so it is
     * not started (the camera comes from the v4l2 provider); IYAKKI_WEBVIEW_ALSA_DEVICES=1 keeps it. Appended to an existing value. */
    if (!getenv("IYAKKI_WEBVIEW_ALSA_DEVICES")) {
        const char *old = getenv("GST_PLUGIN_FEATURE_RANK");
        char *rk = g_strdup_printf("%s%salsadeviceprovider:0", old ? old : "", old && *old ? "," : "");
        setenv("GST_PLUGIN_FEATURE_RANK", rk, 1);
        g_free(rk);
    }
    /* One open per camera. The engine opens a v4l2src for every getUserMedia call and a USB camera cannot be opened twice (the second
     * call fails with "Device is busy" and the page waits for ever). dist/webview/wpe-helper/camshare.c is a GStreamer plugin the web
     * process loads: its device provider wraps the v4l2 provider and every page request is served from one open of the camera. The
     * raw v4l2 provider goes to rank 0 so the engine lists the wrappers only (it stays reachable by name for the plugin). Without
     * the plugin file (older image) nothing changes. IYAKKI_WEBVIEW_CAMSHARE=0 turns it off. */
    if (access(CAMSHARE_PLUGIN, R_OK) == 0 && !(getenv("IYAKKI_WEBVIEW_CAMSHARE") && !strcmp(getenv("IYAKKI_WEBVIEW_CAMSHARE"), "0"))) {
        const char *old = getenv("GST_PLUGIN_FEATURE_RANK");
        char *rk = g_strdup_printf("%s%sv4l2deviceprovider:0", old ? old : "", old && *old ? "," : "");
        setenv("GST_PLUGIN_FEATURE_RANK", rk, 1);
        g_free(rk);
        logf_("camera: one open per camera for all page requests (shared camera plugin)");
    }
    /* Microphones: dist/webview/wpe-helper/micshare.c is a GStreamer device provider that lists the capture streams of /proc/asound,
     * opens them through plughw (alsa-lib converts rate, format and channels, which a raw hw: device cannot) and shares one open between
     * page requests. The stock ALSA provider must not run next to it (it opens every hw: stream "for inspection", logs noise and lists
     * devices that fail with "Rate doesn't match"): rank 0, whatever IYAKKI_WEBVIEW_ALSA_DEVICES says. IYAKKI_WEBVIEW_MICPROVIDER=0
     * turns the plugin off (and leaves the stock provider as it was before). Without the plugin file nothing changes. */
    if (access(MIC_PLUGIN, R_OK) == 0) {
        const char *old = getenv("GST_PLUGIN_FEATURE_RANK");
        gboolean off = getenv("IYAKKI_WEBVIEW_MICPROVIDER") && !strcmp(getenv("IYAKKI_WEBVIEW_MICPROVIDER"), "0");
        char *rk = g_strdup_printf("%s%s%s", old ? old : "", old && *old ? "," : "", off ? "iyakkimic:0" : "iyakkimic:259,alsadeviceprovider:0");
        setenv("GST_PLUGIN_FEATURE_RANK", rk, 1);
        g_free(rk);
        mic_plugin_on = !off;
        logf_("microphone: %s", off ? "plugin present but switched off (IYAKKI_WEBVIEW_MICPROVIDER=0)" : "capture through plughw (microphone plugin)");
    }
    /* The engine's sound output. The usual `autoaudiosink` -> `alsasink` failed to set the T2's sound device up (the channel step of its
     * hardware parameters is refused through every ALSA chain tried), and a page that uses WebAudio then froze on the failed start; the
     * same failure can come from any device that is busy or refuses the engine's parameters. dist/webview/wpe-helper/websink.c is a sink
     * that cannot fail (ALSA the way aplay asks: 16-bit stereo at 48 kHz, silence when the device cannot be used), so it is the engine's
     * output on every machine. IYAKKI_WEBVIEW_AUDIOSINK=0 keeps the usual output. Without the plugin file nothing changes. */
    if (access(WEBSINK_PLUGIN, R_OK) == 0 && !(getenv("IYAKKI_WEBVIEW_AUDIOSINK") && !strcmp(getenv("IYAKKI_WEBVIEW_AUDIOSINK"), "0"))) {
        const char *old = getenv("GST_PLUGIN_FEATURE_RANK");
        char *rk = g_strdup_printf("%s%siyakkiwebaudiosink:300", old ? old : "", old && *old ? "," : "");
        setenv("GST_PLUGIN_FEATURE_RANK", rk, 1);
        g_free(rk);
        logf_("sound: the engine's output is the never-failing sink (web sound plugin)");
    }
    wpe_loader_init(getenv("WPE_BACKEND_LIBRARY"));
    if (hw_open()) {
        if (wpe_fdo_initialize_for_egl_display(G.edpy)) G.egl = 1;
        else { logf_("graphics: wpe_fdo_initialize_for_egl_display failed, using shared memory pictures"); hw_close(); }
    }
    if (!G.egl && !wpe_fdo_initialize_shm()) { fprintf(stderr, "wpe-helper: wpe_fdo_initialize_shm failed\n"); exit(3); }

    if (G.egl) G.exportable = wpe_view_backend_exportable_fdo_egl_create(&export_egl_client, NULL, G.w, G.h);
    else G.exportable = wpe_view_backend_exportable_fdo_create(&export_client, NULL, G.w, G.h);
    G.backend = wpe_view_backend_exportable_fdo_get_view_backend(G.exportable);
    wpe_view_backend_set_target_refresh_rate(G.backend, G.fps * 1000);
    wpe_view_backend_set_fullscreen_handler(G.backend, fullscreen_handler, NULL);
    WebKitWebViewBackend *wb = webkit_web_view_backend_new(G.backend, NULL, NULL);

    if (G.profile && *G.profile) {
        char *data = g_build_filename(G.profile, "data", NULL), *cache = g_build_filename(G.profile, "cache", NULL);
        g_mkdir_with_parents(data, 0700);
        g_mkdir_with_parents(cache, 0700);
        G.session = webkit_network_session_new(data, cache);
        WebKitCookieManager *cm = webkit_network_session_get_cookie_manager(G.session);
        char *cookies = g_build_filename(data, "cookies.sqlite", NULL);
        webkit_cookie_manager_set_persistent_storage(cm, cookies, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
        webkit_cookie_manager_set_accept_policy(cm, WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY);
        g_free(cookies); g_free(data); g_free(cache);
    } else {
        G.session = webkit_network_session_new_ephemeral();
    }
    g_signal_connect(G.session, "download-started", G_CALLBACK(on_download_started), NULL);

    /* Memory: the shell says how much this engine may use (IYAKKI_WEBVIEW_MEM_MB, from the machine's memory and the person's Speed and
       memory choice, webengine/browser_speed); WebKit then frees caches at a third and at half of it instead of waiting for the whole
       machine to run short (its default limits are a share of the machine for EVERY process, so several tabs could add up to all of it).
       The network process gets a third of that for its own caches. No kill threshold: a page is never killed for memory here. */
    WebKitWebContext *ctx = NULL;
    const char *mem_mb = getenv("IYAKKI_WEBVIEW_MEM_MB");
    long mem = mem_mb ? strtol(mem_mb, NULL, 10) : 0;
    if (mem >= 64 && mem <= 65536) {
        WebKitMemoryPressureSettings *mp = webkit_memory_pressure_settings_new();
        webkit_memory_pressure_settings_set_memory_limit(mp, (guint)mem);
        ctx = g_object_new(WEBKIT_TYPE_WEB_CONTEXT, "memory-pressure-settings", mp, NULL);
        WebKitMemoryPressureSettings *np = webkit_memory_pressure_settings_new();
        webkit_memory_pressure_settings_set_memory_limit(np, (guint)(mem / 3 < 64 ? 64 : mem / 3));
        webkit_network_session_set_memory_pressure_settings(np);
        webkit_memory_pressure_settings_free(mp);
        logf_("memory: the web process may use %ld MB, the network process %ld MB (caches are freed at a third and at half of it)", mem, mem / 3 < 64 ? 64L : mem / 3);
    } else {
        ctx = webkit_web_context_new();
    }
    /* IYAKKI_WEBVIEW_CACHE_MODEL: "document" keeps smaller in-memory caches (a machine with little memory, or the person's Small
       choice), "viewer" the smallest (no back-forward cache: for a kiosk page), anything else WebKit's full browser model. */
    const char *cm = getenv("IYAKKI_WEBVIEW_CACHE_MODEL");
    if (cm && !strcmp(cm, "document")) webkit_web_context_set_cache_model(ctx, WEBKIT_CACHE_MODEL_DOCUMENT_BROWSER);
    else if (cm && !strcmp(cm, "viewer")) webkit_web_context_set_cache_model(ctx, WEBKIT_CACHE_MODEL_DOCUMENT_VIEWER);
    if (cm && *cm) logf_("memory: cache model %s", cm);
    if (G.locale && *G.locale) {
        const char *langs[] = { G.locale, NULL };
        webkit_web_context_set_preferred_languages(ctx, langs);
    }

    WebKitSettings *st = webkit_settings_new();
    webkit_settings_set_enable_javascript(st, TRUE);
    webkit_settings_set_enable_fullscreen(st, TRUE);
    webkit_settings_set_enable_media(st, TRUE);
    webkit_settings_set_enable_mediasource(st, TRUE);
    webkit_settings_set_enable_webaudio(st, TRUE);
    webkit_settings_set_enable_webgl(st, TRUE);
    /* getUserMedia / enumerateDevices exist in the page only when the engine was built with ENABLE_MEDIA_STREAM (Buildroot option
     * BR2_PACKAGE_WPEWEBKIT_MEDIA_STREAM; harmless otherwise). Every request is still decided by the person in the shell (on_permission).
     * IYAKKI_WEBVIEW_MEDIA=0 turns the setting off. Mock devices are for tests only (IYAKKI_WEBVIEW_MOCK_MEDIA=1). */
    const char *media_env = getenv("IYAKKI_WEBVIEW_MEDIA");
    webkit_settings_set_enable_media_stream(st, !(media_env && !strcmp(media_env, "0")));
    if (getenv("IYAKKI_WEBVIEW_MOCK_MEDIA")) webkit_settings_set_enable_mock_capture_devices(st, TRUE);
    webkit_settings_set_enable_encrypted_media(st, FALSE);
    webkit_settings_set_enable_smooth_scrolling(st, TRUE);
    webkit_settings_set_media_playback_requires_user_gesture(st, FALSE);
    webkit_settings_set_javascript_can_open_windows_automatically(st, FALSE);
    if (G.ua && *G.ua) {
        char *name = g_strdup(G.ua), *ver = strchr(name, '/');
        if (ver) *ver++ = 0;
        webkit_settings_set_user_agent_with_application_details(st, name, ver && *ver ? ver : NULL);
        g_free(name);
    }

    WebKitUserContentManager *ucm = webkit_user_content_manager_new();
    G.ucm = ucm;
    webkit_user_content_manager_register_script_message_handler(ucm, "iyakkiClip", NULL);
    g_signal_connect(ucm, "script-message-received::iyakkiClip", G_CALLBACK(on_clip_message), NULL);
    WebKitUserScript *us = webkit_user_script_new(CLIP_JS, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(ucm, us);
    webkit_user_script_unref(us);
    webkit_user_content_manager_register_script_message_handler(ucm, "iyakkiMedia", NULL);
    g_signal_connect(ucm, "script-message-received::iyakkiMedia", G_CALLBACK(on_media_message), NULL);
    WebKitUserScript *mu = webkit_user_script_new(MEDIA_JS, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(ucm, mu);
    webkit_user_script_unref(mu);
    webkit_user_content_manager_register_script_message_handler(ucm, "iyakkiDev", NULL);
    g_signal_connect(ucm, "script-message-received::iyakkiDev", G_CALLBACK(on_dev_message), NULL);
    WebKitUserScript *du = webkit_user_script_new(DEV_JS, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(ucm, du);
    webkit_user_script_unref(du);
    /* The browser tile is a person's browser, not a kiosk of unattended pages: sound and video may start without a gesture, as in
     * window mode. Without this WebKit answers play() with NotAllowedError for any video that has sound. */
    /* IYAKKI_WEBVIEW_AUTOPLAY: allow (default), muted (only media without sound starts by itself) or deny (a click starts everything).
     * A sound device that cannot be shared (the T2 speaker has no mixer) is held by the first media element that opens it, and
     * pages such as YouTube open several (muted previews), so the player that is meant to be heard stays silent. */
    const char *ap_env = getenv("IYAKKI_WEBVIEW_AUTOPLAY");
    if (!ap_env) ap_env = IYAKKI_AUTOPLAY_DEFAULT;
    WebKitAutoplayPolicy ap = !strcmp(ap_env, "deny") ? WEBKIT_AUTOPLAY_DENY : !strcmp(ap_env, "muted") ? WEBKIT_AUTOPLAY_ALLOW_WITHOUT_SOUND : WEBKIT_AUTOPLAY_ALLOW;
    WebKitWebsitePolicies *wpol = webkit_website_policies_new_with_policies("autoplay", ap, NULL);
    G.view = g_object_new(WEBKIT_TYPE_WEB_VIEW, "backend", wb, "user-content-manager", ucm, "web-context", ctx, "network-session", G.session, "settings", st, "website-policies", wpol, NULL);
    g_object_unref(wpol);
    g_object_unref(st);
    G.visible = TRUE;
    G.focused = TRUE;
    G.zoom = 100;
    wpe_view_backend_dispatch_set_device_scale_factor(G.backend, G.scale / 100.0f);
    apply_size();
    set_activity();

    WebKitWebView *v = G.view;
    g_signal_connect(v, "notify::title", G_CALLBACK(on_title), NULL);
    g_signal_connect(v, "notify::uri", G_CALLBACK(on_uri), NULL);
    g_signal_connect(v, "notify::is-playing-audio", G_CALLBACK(on_audio_playing), NULL);
    g_signal_connect(v, "notify::estimated-load-progress", G_CALLBACK(on_progress), NULL);
    g_signal_connect(v, "load-changed", G_CALLBACK(on_load_changed), NULL);
    g_signal_connect(v, "load-failed", G_CALLBACK(on_load_failed), NULL);
    g_signal_connect(v, "load-failed-with-tls-errors", G_CALLBACK(on_load_failed_tls), NULL);
    g_signal_connect(v, "decide-policy", G_CALLBACK(on_policy), NULL);
    g_signal_connect(v, "create", G_CALLBACK(on_create), NULL);
    g_signal_connect(v, "mouse-target-changed", G_CALLBACK(on_target), NULL);
    g_signal_connect(v, "permission-request", G_CALLBACK(on_permission), NULL);
    g_signal_connect(v, "notify::camera-capture-state", G_CALLBACK(on_capture_state), NULL);
    g_signal_connect(v, "notify::microphone-capture-state", G_CALLBACK(on_capture_state), NULL);
    g_signal_connect(v, "notify::display-capture-state", G_CALLBACK(on_capture_state), NULL);
    g_signal_connect(v, "script-dialog", G_CALLBACK(on_script_dialog), NULL);
    g_signal_connect(v, "web-process-terminated", G_CALLBACK(on_crash), NULL);
    g_signal_connect_swapped(webkit_web_view_get_back_forward_list(v), "changed", G_CALLBACK(send_history), NULL);
    apply_zoom();
    setup_filter();

    Buf b = {0};
    buf_u32(&b, PROTOCOL_VERSION);
    buf_str(&b, "WPE WebKit");
    char ver[64];
    snprintf(ver, sizeof ver, "%u.%u.%u", webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
    buf_str(&b, ver);
    send_msg(F_READY, &b);
    buf_free(&b);
    G.ready = 1;
}

static gboolean debug_stats(gpointer unused) {
    logf_("stats(%s): %u frames sent, %u pictures from the engine, %u acks, %u blocked, %u unchanged in the last 5 s", G.egl ? "gpu" : "shm", G.nframes, G.nexports, G.nacks, G.nblocked, G.nsame);
    G.nframes = G.nexports = G.nacks = G.nblocked = G.nsame = 0;
    return G_SOURCE_CONTINUE;
}

static void on_term(int sig) { (void)sig; _exit(0); }

int main(int argc, char **argv) {
    int fd = 0;
    const char *shm = NULL;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--socket-fd") && i + 1 < argc) fd = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--shm") && i + 1 < argc) shm = argv[++i];
    }
    if (!shm) { fprintf(stderr, "usage: webview-helper --socket-fd N --shm PATH\n"); return 2; }
    prctl(PR_SET_PDEATHSIG, SIGTERM);
    /* One line on stderr (the system log) every time a helper starts, so a restart of the engine is visible even when the person
     * did not close anything (the shell starts a new helper for a new view; a crash is logged below in on_crash). */
    fprintf(stderr, "wpe-helper: started pid=%d shm=%s\n", (int)getpid(), shm);
    fflush(stderr);
    /* Opt in (IYAKKI_WEBVIEW_NICE): the engine's processes (web, network, GPU) are started from this one and inherit its priority.
     * A page that scrolls or plays a video keeps all cores busy; at the shell's priority they would take turns with the shell, whose drawing and input then wait
     * (the cost is on the shell's side of every frame). Linux keeps a nice value per thread: this is the main thread, before any
     * other thread or child exists. Raising it again is not allowed to an unprivileged user, which is fine: it is only lowered. */
    {
        const char *nice_env = getenv("IYAKKI_WEBVIEW_NICE");
        int nice_v = nice_env ? atoi(nice_env) : 0;
        if (nice_v > 0 && nice_v <= 19 && setpriority(PRIO_PROCESS, 0, nice_v) != 0)
            fprintf(stderr, "wpe-helper: could not lower the engine's priority: %s\n", strerror(errno));
    }
    signal(SIGTERM, on_term);
    signal(SIGPIPE, SIG_IGN);
    G.sock = fd;
    G.in = g_byte_array_new();
    G.debug = getenv("IYAKKI_WEBVIEW_DEBUG") != NULL;
    const char *fps = getenv("IYAKKI_WEBVIEW_FPS");
    G.fps = fps && atoi(fps) > 0 ? (guint)atoi(fps) : 30;
    G.cursor = -1;
    if (!read_hello()) return 0;
    G.shm_fd = open(shm, O_RDWR);
    if (G.shm_fd < 0) { fprintf(stderr, "wpe-helper: cannot open %s: %s\n", shm, strerror(errno)); return 3; }
    if (G.profile && *G.profile && chdir(G.profile) != 0) { /* the profile folder is the working directory when it exists */ }
    /* sockets for reading are non-blocking from here on (writes wait with poll) */
    fcntl(G.sock, F_SETFL, fcntl(G.sock, F_GETFL) | O_NONBLOCK);
    G.loop = g_main_loop_new(NULL, FALSE);
    setup_engine();
    if (G.in->len) { uint8_t *rest = g_memdup2(G.in->data, G.in->len); size_t n = G.in->len; g_byte_array_set_size(G.in, 0); feed(rest, n); g_free(rest); }
    g_unix_fd_add(G.sock, G_IO_IN | G_IO_HUP | G_IO_ERR, on_readable, NULL);
    if (G.debug) g_timeout_add_seconds(5, debug_stats, NULL);
    g_main_loop_run(G.loop);
    alarm(2);          /* a stuck engine shutdown must not keep the shell waiting */
    if (G.view) g_object_unref(G.view);
    _exit(0);
}
