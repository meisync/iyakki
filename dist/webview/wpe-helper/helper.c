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
 * Environment: IYAKKI_WEBVIEW_FPS (default 30), IYAKKI_WEBVIEW_DEBUG=1 (more log lines).
 */
#define _GNU_SOURCE
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
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include <glib.h>
#include <glib-unix.h>
#include <wayland-server.h>
#include <wpe/webkit.h>
#include <wpe/fdo.h>
#include <wpe/unstable/fdo-shm.h>

#include "proto.h"

#define PROTOCOL_VERSION 1
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
    uint32_t zoom;
    int visible, focused;
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
    wpe_view_backend_exportable_fdo_dispatch_release_shm_exported_buffer(G.exportable, eb);
    wpe_view_backend_exportable_fdo_dispatch_frame_complete(G.exportable);
    schedule_flush();
}

static const struct wpe_view_backend_exportable_fdo_client export_client = { .export_shm_buffer = export_shm_buffer };

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

static void on_load_changed(WebKitWebView *v, WebKitLoadEvent e, gpointer ud) {
    (void)v; (void)ud;
    if (e == WEBKIT_LOAD_FINISHED) send_bb(F_LOADING, 0, 100);
    else if (e == WEBKIT_LOAD_STARTED) send_bb(F_LOADING, 1, 0);
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

static void on_crash(WebKitWebView *v, WebKitWebProcessTerminationReason r, gpointer ud) {
    (void)ud;
    G.crashes++;
    send_str_msg(F_CRASHED, r == WEBKIT_WEB_PROCESS_EXCEEDED_MEMORY_LIMIT ? "the web process used too much memory" : "the web process stopped");
    if (G.crashes < 5) g_timeout_add(500, reload_later, v);
    else { logf_("the web process keeps crashing; giving up"); quit_loop(); }
}

static gboolean on_permission(WebKitWebView *v, WebKitPermissionRequest *req, gpointer ud) {
    (void)v; (void)ud;
    webkit_permission_request_deny(req);
    return TRUE;
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
    e.x = css(x); e.y = css(y);
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
    e.base.x = css(x); e.base.y = css(y);
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

/* ---------------------------------------------------------------------------------------------- messages */
static void handle(uint8_t ty, const uint8_t *p, size_t n) {
    Rd r = { .p = p, .n = n };
    if (!G.view) return;
    switch (ty) {
    case T_NAVIGATE: {
        char *u = rd_str(&r);
        if (host_allowed(u)) webkit_web_view_load_uri(G.view, u); else logf_("refused to open %s", u);
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
    case T_VISIBLE: G.visible = rd_u8(&r) != 0; set_activity(); if (G.visible) mark_all(); break;
    case T_ACK: {
        uint8_t s = rd_u8(&r);
        G.nacks++;
        if (s < 2) G.busy[s] = FALSE;
        if (G.back_new) schedule_flush();
        break;
    }
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
    wpe_loader_init(getenv("WPE_BACKEND_LIBRARY"));
    if (!wpe_fdo_initialize_shm()) { fprintf(stderr, "wpe-helper: wpe_fdo_initialize_shm failed\n"); exit(3); }

    G.exportable = wpe_view_backend_exportable_fdo_create(&export_client, NULL, G.w, G.h);
    G.backend = wpe_view_backend_exportable_fdo_get_view_backend(G.exportable);
    wpe_view_backend_set_target_refresh_rate(G.backend, G.fps * 1000);
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

    WebKitWebContext *ctx = webkit_web_context_new();
    if (G.locale && *G.locale) {
        const char *langs[] = { G.locale, NULL };
        webkit_web_context_set_preferred_languages(ctx, langs);
    }

    WebKitSettings *st = webkit_settings_new();
    webkit_settings_set_enable_javascript(st, TRUE);
    webkit_settings_set_enable_media(st, TRUE);
    webkit_settings_set_enable_mediasource(st, TRUE);
    webkit_settings_set_enable_webaudio(st, TRUE);
    webkit_settings_set_enable_webgl(st, TRUE);
    webkit_settings_set_enable_media_stream(st, FALSE);
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

    G.view = g_object_new(WEBKIT_TYPE_WEB_VIEW, "backend", wb, "web-context", ctx, "network-session", G.session, "settings", st, NULL);
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
    g_signal_connect(v, "notify::estimated-load-progress", G_CALLBACK(on_progress), NULL);
    g_signal_connect(v, "load-changed", G_CALLBACK(on_load_changed), NULL);
    g_signal_connect(v, "load-failed", G_CALLBACK(on_load_failed), NULL);
    g_signal_connect(v, "load-failed-with-tls-errors", G_CALLBACK(on_load_failed_tls), NULL);
    g_signal_connect(v, "decide-policy", G_CALLBACK(on_policy), NULL);
    g_signal_connect(v, "create", G_CALLBACK(on_create), NULL);
    g_signal_connect(v, "mouse-target-changed", G_CALLBACK(on_target), NULL);
    g_signal_connect(v, "permission-request", G_CALLBACK(on_permission), NULL);
    g_signal_connect(v, "script-dialog", G_CALLBACK(on_script_dialog), NULL);
    g_signal_connect(v, "web-process-terminated", G_CALLBACK(on_crash), NULL);
    g_signal_connect_swapped(webkit_web_view_get_back_forward_list(v), "changed", G_CALLBACK(send_history), NULL);
    apply_zoom();

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
    logf_("stats: %u frames sent, %u pictures from the engine, %u acks, %u blocked, %u unchanged in the last 5 s", G.nframes, G.nexports, G.nacks, G.nblocked, G.nsame);
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
