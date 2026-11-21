/* iyakki-screenportal: the in-house ScreenCast portal for the full engine (F-BROW-71, docs/development/BROWSER-MEDIA-DEVICES.md section 5).
 *
 * The engine (WPE WebKit) reads a shared screen only through the desktop portal `org.freedesktop.portal.ScreenCast` and a PipeWire node it
 * gets from it. This program is that portal, small and private to one helper:
 *   - it starts a private session bus (dbus-daemon) and a private PipeWire daemon in its own folder (mode 0700) and prints their addresses to
 *     its standard output (`bus ADDRESS`, `rtdir DIR`, `ready`), which the helper puts in the engine's environment;
 *   - it owns `org.freedesktop.portal.Desktop` on that bus and answers the four ScreenCast methods the engine calls (CreateSession,
 *     SelectSources, Start, OpenPipeWireRemote) with the `Request::Response` signals the specification wants;
 *   - the helper tells it, on its standard input, where the shell writes its screen (`feed PATH`; `feed` alone ends it; `quit`). While there
 *     is a feed, a pipeline appsrc ! videoconvert ! pipewiresink publishes the pictures of the feed file (screenfeed.h) as a PipeWire video node.
 * The person's allow is the shell's: Start answers "ended" (code 2) unless the shell has handed over a feed, so a page cannot get a picture the
 * person did not agree to even if it reaches the portal. Nothing here is a sound or a camera. Not run in this tree (no PipeWire in the build
 * host's sysroot); the D-Bus and GStreamer calls compile against the image's headers, the names and the feed reader are tested on their own. */
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <gst/app/gstappsrc.h>
#include <gst/gst.h>

#include "portalpure.h"
#include "screenfeed.h"

static const char XML[] =
    "<node><interface name='org.freedesktop.portal.ScreenCast'>"
    "<method name='CreateSession'><arg type='a{sv}' name='options' direction='in'/><arg type='o' name='handle' direction='out'/></method>"
    "<method name='SelectSources'><arg type='o' name='session_handle' direction='in'/><arg type='a{sv}' name='options' direction='in'/><arg type='o' name='handle' direction='out'/></method>"
    "<method name='Start'><arg type='o' name='session_handle' direction='in'/><arg type='s' name='parent_window' direction='in'/><arg type='a{sv}' name='options' direction='in'/><arg type='o' name='handle' direction='out'/></method>"
    "<method name='OpenPipeWireRemote'><arg type='o' name='session_handle' direction='in'/><arg type='a{sv}' name='options' direction='in'/><arg type='h' name='fd' direction='out'/></method>"
    "<property name='AvailableSourceTypes' type='u' access='read'/><property name='AvailableCursorModes' type='u' access='read'/><property name='version' type='u' access='read'/>"
    "</interface></node>";

static GMainLoop *loop;
static GDBusConnection *bus;
static GDBusNodeInfo *info;
static char *rt_dir, *feed_path, *session_path;
static GPid dbus_pid, pw_pid;
static GstElement *pipe_, *src_, *sink_;
static uint64_t last_seq;
static uint32_t cur_w, cur_h, node_id;
static guint feed_timer;

static void say(const char *what, const char *v) { printf("%s%s%s\n", what, v ? " " : "", v ? v : ""); fflush(stdout); }

/* ---- the pipeline: the feed file as a PipeWire video node -------------------------------------------------------------------- */

static gboolean push_feed(gpointer ud) {
    (void)ud;
    SfFrame f;
    if (!feed_path || !src_ || !sf_read(feed_path, last_seq, &f)) return G_SOURCE_CONTINUE;
    last_seq = f.seq;
    if (f.w != cur_w || f.h != cur_h) {
        GstCaps *caps = gst_caps_new_simple("video/x-raw", "format", G_TYPE_STRING, "RGBA", "width", G_TYPE_INT, (int)f.w, "height", G_TYPE_INT, (int)f.h, "framerate", GST_TYPE_FRACTION, 4, 1, NULL);
        gst_app_src_set_caps(GST_APP_SRC(src_), caps);
        gst_caps_unref(caps);
        cur_w = f.w; cur_h = f.h;
    }
    gsize n = (gsize)f.w * f.h * 4;
    gst_app_src_push_buffer(GST_APP_SRC(src_), gst_buffer_new_wrapped(f.rgba, n));   /* the buffer owns the bytes and frees them */
    return G_SOURCE_CONTINUE;
}

static void stop_pipeline(void) {
    if (feed_timer) { g_source_remove(feed_timer); feed_timer = 0; }
    if (pipe_) { gst_element_set_state(pipe_, GST_STATE_NULL); gst_object_unref(pipe_); }
    pipe_ = src_ = sink_ = NULL;
    cur_w = cur_h = node_id = 0;
    last_seq = 0;
}

/* the PipeWire node id of our sink: the element's own property when this plug-in has one, else the node named iyakki-screen in `pw-dump` */
static uint32_t find_node_id(void) {
    if (sink_ && g_object_class_find_property(G_OBJECT_GET_CLASS(sink_), "node-id")) {
        guint id = 0;
        g_object_get(sink_, "node-id", &id, NULL);
        if (id && id != G_MAXUINT) return id;
    }
    char *out = NULL;
    char *argv[] = { (char *)"pw-dump", NULL };
    gchar *envp[] = { NULL, NULL };
    envp[0] = g_strdup_printf("XDG_RUNTIME_DIR=%s", rt_dir);
    gint status = 0;
    uint32_t id = 0;
    if (g_spawn_sync(NULL, argv, envp, G_SPAWN_SEARCH_PATH | G_SPAWN_STDERR_TO_DEV_NULL, NULL, NULL, &out, NULL, &status, NULL) && out) {
        const char *hit = strstr(out, "\"node.name\": \"iyakki-screen\"");
        if (hit) {
            const char *p = hit;
            while (p > out && !(p[0] == '"' && !strncmp(p, "\"id\":", 5))) p--;
            if (p > out) id = (uint32_t)strtoul(p + 5, NULL, 10);
        }
    }
    g_free(out);
    g_free(envp[0]);
    return id;
}

static int start_pipeline(void) {
    if (pipe_) return 0;
    GError *err = NULL;
    pipe_ = gst_parse_launch("appsrc name=src is-live=true format=time do-timestamp=true ! videoconvert ! pipewiresink name=sink mode=provide stream-properties=\"props,media.class=Video/Source,node.name=iyakki-screen\"", &err);
    if (!pipe_) { fprintf(stderr, "screenportal: pipeline: %s\n", err ? err->message : "?"); g_clear_error(&err); return -1; }
    src_ = gst_bin_get_by_name(GST_BIN(pipe_), "src");
    sink_ = gst_bin_get_by_name(GST_BIN(pipe_), "sink");
    gst_object_unref(src_);   /* the bin keeps both alive; the pointers stay valid until the pipeline is dropped */
    gst_object_unref(sink_);
    gst_app_src_set_stream_type(GST_APP_SRC(src_), GST_APP_STREAM_TYPE_STREAM);
    gst_element_set_state(pipe_, GST_STATE_PLAYING);
    feed_timer = g_timeout_add(250, push_feed, NULL);
    return 0;
}

/* ---- a Response signal for a request, after the method call has returned -------------------------------------------------------- */

typedef struct { char *dest, *request, *token; guint code, tries; gboolean want_stream; } Pending;

static void respond(Pending *p, GVariant *results) {
    g_dbus_connection_emit_signal(bus, p->dest, p->request, "org.freedesktop.portal.Request", "Response", g_variant_new("(u@a{sv})", p->code, results), NULL);
}

static gboolean pending_step(gpointer ud) {
    Pending *p = ud;
    GVariantBuilder res;
    g_variant_builder_init(&res, G_VARIANT_TYPE("a{sv}"));
    if (p->want_stream) {
        uint32_t id = pipe_ ? find_node_id() : 0;
        if (!id && ++p->tries < 40) { g_variant_builder_clear(&res); return G_SOURCE_CONTINUE; }   /* the node shows up a moment after PLAYING */
        if (id) {
            node_id = id;
            GVariantBuilder props, streams;
            g_variant_builder_init(&props, G_VARIANT_TYPE("a{sv}"));
            g_variant_builder_add(&props, "{sv}", "source_type", g_variant_new_uint32(1));
            g_variant_builder_init(&streams, G_VARIANT_TYPE("a(ua{sv})"));
            g_variant_builder_add(&streams, "(ua{sv})", id, &props);
            g_variant_builder_add(&res, "{sv}", "streams", g_variant_builder_end(&streams));
            p->code = 0;
        } else {
            fprintf(stderr, "screenportal: no PipeWire node after 4 seconds\n");
            p->code = 2;
        }
    }
    respond(p, g_variant_builder_end(&res));
    g_free(p->dest); g_free(p->request); g_free(p->token); g_free(p);
    return G_SOURCE_REMOVE;
}

static gboolean pending_first(gpointer ud) { return pending_step(ud) == G_SOURCE_CONTINUE ? (g_timeout_add(100, pending_step, ud), G_SOURCE_REMOVE) : G_SOURCE_REMOVE; }

static void queue_response(const char *sender, const char *request, guint code, gboolean want_stream) {
    Pending *p = g_new0(Pending, 1);
    p->dest = g_strdup(sender); p->request = g_strdup(request); p->code = code; p->want_stream = want_stream;
    g_idle_add(pending_first, p);
}

/* ---- the methods --------------------------------------------------------------------------------------------------------------- */

static int token_of(GVariant *opts, const char *key, char *out, size_t cap) {
    const char *t = NULL;
    out[0] = 0;
    if (!g_variant_lookup(opts, key, "&s", &t) || !pp_token_ok(t) || strlen(t) + 1 > cap) return 0;
    strcpy(out, t);
    return 1;
}

static void on_method(GDBusConnection *c, const char *sender, const char *obj, const char *iface, const char *method, GVariant *params, GDBusMethodInvocation *inv, gpointer ud) {
    (void)c; (void)obj; (void)iface; (void)ud;
    char tok[80], req[256], ses[256];
    GVariant *opts = NULL;
    const char *session = NULL, *parent = NULL;
    if (!strcmp(method, "CreateSession")) g_variant_get(params, "(@a{sv})", &opts);
    else if (!strcmp(method, "Start")) g_variant_get(params, "(&s&s@a{sv})", &session, &parent, &opts);
    else g_variant_get(params, "(&s@a{sv})", &session, &opts);
    int ok = token_of(opts, "handle_token", tok, sizeof tok) && pp_path("request", sender, tok, req, sizeof req);
    if (!ok) { g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.DBus.Error.InvalidArgs", "bad handle_token"); goto out; }
    if (!strcmp(method, "CreateSession")) {
        char st[80];
        if (!token_of(opts, "session_handle_token", st, sizeof st) || !pp_path("session", sender, st, ses, sizeof ses)) { g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.DBus.Error.InvalidArgs", "bad session_handle_token"); goto out; }
        g_free(session_path);
        session_path = g_strdup(ses);   /* one session at a time: a new one replaces the old */
        g_dbus_method_invocation_return_value(inv, g_variant_new("(o)", req));
        GVariantBuilder r;
        g_variant_builder_init(&r, G_VARIANT_TYPE("a{sv}"));
        g_variant_builder_add(&r, "{sv}", "session_handle", g_variant_new_string(ses));
        Pending *p = g_new0(Pending, 1);
        p->dest = g_strdup(sender); p->request = g_strdup(req); p->code = 0;
        respond(p, g_variant_builder_end(&r));
        g_free(p->dest); g_free(p->request); g_free(p);
    } else if (!session_path || strcmp(session, session_path)) {
        g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.DBus.Error.InvalidArgs", "no such session");
    } else if (!strcmp(method, "SelectSources")) {
        g_dbus_method_invocation_return_value(inv, g_variant_new("(o)", req));
        queue_response(sender, req, 0, FALSE);
    } else if (!strcmp(method, "Start")) {
        g_dbus_method_invocation_return_value(inv, g_variant_new("(o)", req));
        if (!pp_may_start(feed_path) || start_pipeline() != 0) queue_response(sender, req, 2, FALSE);   /* ended: no screen was handed over */
        else queue_response(sender, req, 0, TRUE);
    } else if (!strcmp(method, "OpenPipeWireRemote")) {
        char path[256];
        snprintf(path, sizeof path, "%s/pipewire-0", rt_dir);
        struct sockaddr_un a = { .sun_family = AF_UNIX };
        int fd = pp_may_start(feed_path) && strlen(path) < sizeof a.sun_path ? socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0) : -1;
        if (fd >= 0) { strcpy(a.sun_path, path); if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) { close(fd); fd = -1; } }
        if (fd < 0) { g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.DBus.Error.Failed", "no PipeWire"); goto out; }
        GUnixFDList *l = g_unix_fd_list_new_from_array(&fd, 1);
        g_dbus_method_invocation_return_value_with_unix_fd_list(inv, g_variant_new("(h)", 0), l);
        g_object_unref(l);
    } else {
        g_dbus_method_invocation_return_dbus_error(inv, "org.freedesktop.DBus.Error.UnknownMethod", "unknown");
    }
out:
    if (opts) g_variant_unref(opts);
}

static GVariant *on_prop(GDBusConnection *c, const char *sender, const char *obj, const char *iface, const char *prop, GError **err, gpointer ud) {
    (void)c; (void)sender; (void)obj; (void)iface; (void)ud;
    if (!strcmp(prop, "version")) return g_variant_new_uint32(4);
    if (!strcmp(prop, "AvailableSourceTypes")) return g_variant_new_uint32(1);    /* the whole screen only: no window picker */
    if (!strcmp(prop, "AvailableCursorModes")) return g_variant_new_uint32(1);    /* the cursor is not drawn */
    g_set_error(err, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_PROPERTY, "unknown property");
    return NULL;
}

/* ---- the helper's commands on standard input ------------------------------------------------------------------------------------ */

static void quit_all(void) {
    stop_pipeline();
    if (pw_pid) kill(pw_pid, SIGTERM);
    if (dbus_pid) kill(dbus_pid, SIGTERM);
    if (rt_dir) { char *sock = g_strdup_printf("%s/pipewire-0", rt_dir); unlink(sock); g_free(sock); sock = g_strdup_printf("%s/pipewire-0.lock", rt_dir); unlink(sock); g_free(sock); rmdir(rt_dir); }
    if (loop) g_main_loop_quit(loop);
}

static gboolean on_term(gpointer ud) { (void)ud; quit_all(); return G_SOURCE_REMOVE; }

static gboolean on_stdin(GIOChannel *ch, GIOCondition cond, gpointer ud) {
    (void)ud;
    gchar *line = NULL;
    gsize n = 0;
    if ((cond & (G_IO_HUP | G_IO_ERR)) && !(cond & G_IO_IN)) { quit_all(); return G_SOURCE_REMOVE; }
    GIOStatus st = g_io_channel_read_line(ch, &line, &n, NULL, NULL);
    if (st == G_IO_STATUS_EOF || st == G_IO_STATUS_ERROR) { quit_all(); return G_SOURCE_REMOVE; }
    if (line) {
        g_strchomp(line);
        if (!strcmp(line, "quit")) { g_free(line); quit_all(); return G_SOURCE_REMOVE; }
        if (!strncmp(line, "feed", 4) && (line[4] == 0 || line[4] == ' ')) {
            const char *p = line[4] ? line + 5 : "";
            g_free(feed_path);
            feed_path = *p ? g_strdup(p) : NULL;
            if (!feed_path) { stop_pipeline(); g_free(session_path); session_path = NULL; }   /* the share ended: the node goes with it */
            last_seq = 0;
        }
        g_free(line);
    }
    return G_SOURCE_CONTINUE;
}

/* ---- start ------------------------------------------------------------------------------------------------------------------------ */

static char *start_bus(void) {
    char *argv[] = { (char *)"dbus-daemon", (char *)"--session", (char *)"--nofork", (char *)"--print-address=1", NULL };
    gint out_fd = -1;
    if (!g_spawn_async_with_pipes(NULL, argv, NULL, G_SPAWN_SEARCH_PATH | G_SPAWN_DO_NOT_REAP_CHILD, NULL, NULL, &dbus_pid, NULL, &out_fd, NULL, NULL)) return NULL;
    GIOChannel *ch = g_io_channel_unix_new(out_fd);
    gchar *addr = NULL;
    if (g_io_channel_read_line(ch, &addr, NULL, NULL, NULL) != G_IO_STATUS_NORMAL) addr = NULL;
    g_io_channel_unref(ch);
    if (addr) g_strchomp(addr);
    return addr && *addr ? addr : NULL;
}

static int start_pipewire(void) {
    char *argv[] = { (char *)"pipewire", NULL };
    gchar *envp[] = { NULL, NULL, NULL };
    envp[0] = g_strdup_printf("XDG_RUNTIME_DIR=%s", rt_dir);
    envp[1] = g_strdup("PATH=/usr/bin:/bin");
    gboolean ok = g_spawn_async(NULL, argv, envp, G_SPAWN_SEARCH_PATH | G_SPAWN_DO_NOT_REAP_CHILD | G_SPAWN_STDOUT_TO_DEV_NULL, NULL, NULL, &pw_pid, NULL);
    g_free(envp[0]); g_free(envp[1]);
    if (!ok) return -1;
    char *sock = g_strdup_printf("%s/pipewire-0", rt_dir);
    for (int i = 0; i < 50 && access(sock, F_OK) != 0; i++) g_usleep(100 * 1000);   /* the daemon makes its socket within a moment */
    int up = access(sock, F_OK) == 0;
    g_free(sock);
    return up ? 0 : -1;
}

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    gst_init(NULL, NULL);
    signal(SIGPIPE, SIG_IGN);
    GError *err = NULL;
    rt_dir = g_dir_make_tmp("iyakki-screen-XXXXXX", &err);
    if (!rt_dir) { fprintf(stderr, "screenportal: %s\n", err ? err->message : "no folder"); return 1; }
    char *addr = start_bus();
    if (!addr) { fprintf(stderr, "screenportal: no session bus\n"); quit_all(); return 1; }
    if (start_pipewire() != 0) { fprintf(stderr, "screenportal: no PipeWire daemon\n"); quit_all(); return 1; }
    bus = g_dbus_connection_new_for_address_sync(addr, G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT | G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION, NULL, NULL, &err);
    if (!bus) { fprintf(stderr, "screenportal: bus: %s\n", err ? err->message : "?"); quit_all(); return 1; }
    info = g_dbus_node_info_new_for_xml(XML, &err);
    static const GDBusInterfaceVTable vt = { on_method, on_prop, NULL, { 0 } };
    if (!info || !g_dbus_connection_register_object(bus, PP_PREFIX, info->interfaces[0], &vt, NULL, NULL, &err)) { fprintf(stderr, "screenportal: object: %s\n", err ? err->message : "?"); quit_all(); return 1; }
    g_bus_own_name_on_connection(bus, "org.freedesktop.portal.Desktop", G_BUS_NAME_OWNER_FLAGS_NONE, NULL, NULL, NULL, NULL);
    say("bus", addr);
    say("rtdir", rt_dir);
    say("ready", NULL);
    GIOChannel *in = g_io_channel_unix_new(0);
    g_io_add_watch(in, G_IO_IN | G_IO_HUP | G_IO_ERR, on_stdin, NULL);
    g_unix_signal_add(SIGTERM, on_term, NULL);
    loop = g_main_loop_new(NULL, FALSE);
    g_main_loop_run(loop);
    return 0;
}
