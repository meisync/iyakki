/*
 * Self check of websink.c (SPDX-License-Identifier: Apache-2.0): `make check` in this folder.
 * Creates the IyakkiWebSink bin (the engine's sound output) and checks: `ts-offset` is a property (as on GstBaseSink: int64, default 0,
 * full range), is set and read back through g_object_set / g_object_get without any GLib warning, reaches the inner sink, and is kept when
 * there is no inner sink; audio runs through audiotestsrc ! IyakkiWebSink to the end of the stream; the lines the sink writes for the
 * system log appear (started, first buffer, stopped), an error and a warning of an inner element are logged with the element name, the
 * message and the debug text, and a burst of errors makes ONE line per second.
 * Needs GStreamer with audiotestsrc, audioconvert, audioresample and ALSA's built-in `null` device (set below, so the real sound card is
 * never opened). Not covered: a real sound card, the engine (WebKit) setting the property.
 */
#define WEBSINK_NO_PLUGIN_DEFINE
#include "websink.c"

static int failures;
#define CHECK(c, ...) do { if (!(c)) { failures++; g_printerr("FAIL %s:%d: ", __FILE__, __LINE__); g_printerr(__VA_ARGS__); g_printerr("\n"); } } while (0)

static GPtrArray *lines;
static int warnings; /* GLib CRITICAL / WARNING messages */

static void hook(const char *line) { g_ptr_array_add(lines, g_strdup(line)); }

static void count_log(const gchar *domain, GLogLevelFlags level, const gchar *msg, gpointer unused) {
    if (level & (G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING)) { warnings++; g_printerr("GLib: %s\n", msg); }
}

static guint count(const char *needle) {
    guint n = 0;
    for (guint i = 0; i < lines->len; i++) if (strstr(lines->pdata[i], needle)) n++;
    return n;
}

static gint64 get_ts(gpointer o) { gint64 v = -1; g_object_get(o, "ts-offset", &v, NULL); return v; }

int main(void) {
    g_setenv("IYAKKI_WEBSINK_DEVICE", "null", TRUE);
    gst_init(NULL, NULL);
    GST_DEBUG_CATEGORY_INIT(websink_debug, "iyakkiwebsink", 0, "check");
    lines = g_ptr_array_new_with_free_func(g_free);
    log_hook = hook;
    g_log_set_default_handler(count_log, NULL);

    /* the property */
    GParamSpec *ps = g_object_class_find_property(g_type_class_ref(IYAKKI_TYPE_WEB_SINK), "ts-offset");
    CHECK(ps && G_IS_PARAM_SPEC_INT64(ps), "ts-offset is not an int64 property");
    if (ps) {
        GParamSpecInt64 *p = G_PARAM_SPEC_INT64(ps);
        CHECK(p->minimum == G_MININT64 && p->maximum == G_MAXINT64 && p->default_value == 0, "range or default differs from GstBaseSink");
    }
    GstElement *sink = g_object_new(IYAKKI_TYPE_WEB_SINK, NULL);
    IyakkiWebSink *self = (IyakkiWebSink *)sink;
    CHECK(self->sink != NULL, "no inner sink");
    CHECK(get_ts(sink) == 0, "default is not 0");
    g_object_set(sink, "ts-offset", (gint64)123456789, NULL);
    CHECK(get_ts(sink) == 123456789, "read back %" G_GINT64_FORMAT, get_ts(sink));
    CHECK(get_ts(self->sink) == 123456789, "not forwarded to the inner sink");
    g_object_set(sink, "ts-offset", (gint64)-5000000, NULL);
    CHECK(get_ts(sink) == -5000000 && get_ts(self->sink) == -5000000, "negative value");
    /* no inner sink (it could not be made, or not yet): the value is kept and applied to it once it is there */
    GstElement *inner = self->sink;
    self->sink = NULL;
    g_object_set(sink, "ts-offset", (gint64)777, NULL);
    CHECK(get_ts(sink) == 777, "kept without an inner sink");
    self->sink = inner;
    g_object_set(sink, "ts-offset", (gint64)888, NULL);
    CHECK(get_ts(inner) == 888, "applied once the inner sink is there");
    g_object_set(sink, "ts-offset", (gint64)0, NULL);

    /* audio through a pipeline */
    GstElement *pipe = gst_pipeline_new("p");
    GstElement *src = gst_element_factory_make("audiotestsrc", NULL);
    CHECK(src != NULL, "audiotestsrc is missing");
    g_object_set(src, "num-buffers", 30, "samplesperbuffer", 960, NULL);
    gst_bin_add_many(GST_BIN(pipe), src, sink, NULL);
    CHECK(gst_element_link(src, sink), "link");
    GstBus *bus = gst_element_get_bus(pipe);
    CHECK(gst_element_set_state(pipe, GST_STATE_PLAYING) != GST_STATE_CHANGE_FAILURE, "cannot play");
    GstMessage *m = gst_bus_timed_pop_filtered(bus, 10 * GST_SECOND, GST_MESSAGE_EOS | GST_MESSAGE_ERROR);
    CHECK(m && GST_MESSAGE_TYPE(m) == GST_MESSAGE_EOS, "the stream did not reach its end");
    if (m) gst_message_unref(m);
    CHECK(count("sink started") == 1, "no 'sink started' line");
    CHECK(count("first buffer") == 1, "no 'first buffer' line");

    /* an error and a warning of an inner element reach the log; a burst of errors makes one line */
    GST_ELEMENT_WARNING(inner, STREAM, FAILED, ("odd\nthing"), ("warn debug"));
    for (int i = 0; i < 5; i++) GST_ELEMENT_ERROR(inner, STREAM, FAILED, ("device exploded"), ("error debug text"));
    CHECK(count("WARNING from") == 1 && count("odd thing") == 1 && count("warn debug") == 1, "warning line");
    CHECK(count("ERROR from") == 1, "%u error lines, wanted one", count("ERROR from"));
    CHECK(count("device exploded") == 1 && count("error debug text") == 1 && count(GST_OBJECT_NAME(inner)) >= 1, "error line lacks name, message or debug");
    g_usleep(1100000);
    GST_ELEMENT_ERROR(inner, STREAM, FAILED, ("again"), ("d"));
    CHECK(count("ERROR from") == 2 && count("(+4 similar)") == 1, "the next second logs again and counts the dropped ones");
    /* the error still goes on up to the pipeline's bus */
    m = gst_bus_timed_pop_filtered(bus, 2 * GST_SECOND, GST_MESSAGE_ERROR);
    CHECK(m != NULL, "the error did not reach the pipeline bus");
    if (m) gst_message_unref(m);

    gst_element_set_state(pipe, GST_STATE_NULL);
    CHECK(count("sink stopped") >= 1, "no 'sink stopped' line");
    gst_object_unref(bus);
    gst_object_unref(pipe);
    CHECK(warnings == 0, "%d GLib warnings or criticals", warnings);
    for (guint i = 0; i < lines->len; i++) g_print("  log: %s\n", (char *)lines->pdata[i]);
    if (failures) { g_printerr("websink-check: %d failure(s)\n", failures); return 1; }
    g_print("websink-check: ok\n");
    return 0;
}
