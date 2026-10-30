/*
 * Self check of camshare.c (SPDX-License-Identifier: Apache-2.0): `make check` in this folder.
 * Needs GStreamer with videotestsrc, videoconvert, jpegenc and jpegdec; no camera. The test camera refuses a second open, like a UVC node.
 */
#define CAMSHARE_NO_PLUGIN_DEFINE
#include "camshare.c"

static int failures;
#define CHECK(c, ...) do { if (!(c)) { failures++; g_printerr("FAIL %s:%d: ", __FILE__, __LINE__); g_printerr(__VA_ARGS__); g_printerr("\n"); } } while (0)

static void pump(int ms) {
    gint64 end = g_get_monotonic_time() + (gint64)ms * 1000;
    while (g_get_monotonic_time() < end) { while (g_main_context_iteration(NULL, FALSE)); g_usleep(5000); }
}

static void mode_is(const char *offer, const char *want) {
    GstCaps *in = gst_caps_from_string(offer), *got = iyakki_camshare_choose(in), *w = gst_caps_from_string(want);
    gchar *gs = got ? gst_caps_to_string(got) : g_strdup("(none)");
    CHECK(got && gst_caps_is_equal(got, w), "offer [%s] gave [%s], wanted [%s]", offer, gs, want);
    g_free(gs);
    if (got) gst_caps_unref(got);
    gst_caps_unref(in); gst_caps_unref(w);
}

static void modes(void) {
    /* a 4K UVC camera: 4K, 1080p and 720p MJPEG, 640x480 raw */
    mode_is("image/jpeg, width=3840, height=2160, framerate=30/1; image/jpeg, width=1920, height=1080, framerate=30/1;"
            "image/jpeg, width=1280, height=720, framerate=30/1; video/x-raw, format=YUY2, width=640, height=480, framerate=30/1",
            "image/jpeg, parsed=true, width=1920, height=1080, framerate=30/1, pixel-aspect-ratio=1/1");
    /* nothing above 720p: take the biggest */
    mode_is("video/x-raw, format=YUY2, width=640, height=480, framerate={30/1, 15/1}; video/x-raw, format=YUY2, width=1280, height=720, framerate=10/1",
            "video/x-raw, format=YUY2, width=640, height=480, framerate=30/1, pixel-aspect-ratio=1/1");
    /* only 4K */
    mode_is("image/jpeg, width=3840, height=2160, framerate=30/1", "image/jpeg, parsed=true, width=3840, height=2160, framerate=30/1, pixel-aspect-ratio=1/1");
    /* stepwise sizes and a frame rate range */
    mode_is("video/x-raw, format=NV12, width=[160,4096], height=[120,2160], framerate=[1/1,60/1]",
            "video/x-raw, format=NV12, width=1920, height=1080, framerate=30/1, pixel-aspect-ratio=1/1");
    /* below 1080p a raw mode needs no decoding and beats MJPEG; H.264 is never taken */
    mode_is("video/x-h264, width=1920, height=1080, framerate=30/1; image/jpeg, width=1280, height=720, framerate=30/1; video/x-raw, format=YUY2, width=1280, height=720, framerate=30/1",
            "video/x-raw, format=YUY2, width=1280, height=720, framerate=30/1, pixel-aspect-ratio=1/1");
    GstCaps *none = gst_caps_from_string("video/x-h264, width=640, height=480, framerate=30/1");
    CHECK(iyakki_camshare_choose(none) == NULL, "H.264 only has no mode");
    gst_caps_unref(none);
}

typedef struct { GstElement *pipe; GstElement *sink; gint frames; gint errors; } Page;

static GstFlowReturn count_frame(GstAppSink *s, gpointer d) {
    GstSample *sm = gst_app_sink_pull_sample(s);
    if (sm) { g_atomic_int_inc(&((Page *)d)->frames); gst_sample_unref(sm); }
    return GST_FLOW_OK;
}

static void link_pad(GstPad *sinkpad, GstPad *pad) { if (!gst_pad_is_linked(sinkpad)) gst_pad_link(pad, sinkpad); }

/* a page as WebKit builds it: device element ! converter ! capsfilter (its own size and rate) ! appsink */
static Page *open_page(GstDevice *dev, int w, int h, int fps) {
    Page *p = g_new0(Page, 1);
    GstElement *src = gst_device_create_element(dev, NULL);
    GstElement *dec = gst_element_factory_make("decodebin3", NULL);
    GstElement *conv = gst_element_factory_make("videoconvert", NULL), *scale = gst_element_factory_make("videoscale", NULL);
    GstElement *rate = gst_element_factory_make("videorate", NULL), *cf = gst_element_factory_make("capsfilter", NULL);
    p->sink = gst_element_factory_make("appsink", NULL); p->pipe = gst_pipeline_new(NULL);
    gchar *c = g_strdup_printf("video/x-raw,width=%d,height=%d,framerate=%d/1", w, h, fps);
    GstCaps *caps = gst_caps_from_string(c);
    g_object_set(cf, "caps", caps, NULL); g_object_set(src, "do-timestamp", TRUE, NULL);
    g_object_set(p->sink, "sync", FALSE, "emit-signals", FALSE, NULL);
    gst_caps_unref(caps); g_free(c);
    GstAppSinkCallbacks cb = { 0 }; cb.new_sample = count_frame; gst_app_sink_set_callbacks(GST_APP_SINK(p->sink), &cb, p, NULL);
    gst_bin_add_many(GST_BIN(p->pipe), src, dec, conv, scale, rate, cf, p->sink, NULL);
    gst_element_link(src, dec);
    gst_element_link_many(conv, scale, rate, cf, p->sink, NULL);
    GstPad *sinkpad = gst_element_get_static_pad(conv, "sink");
    g_signal_connect_swapped(dec, "pad-added", G_CALLBACK(link_pad), sinkpad);
    gst_element_set_state(p->pipe, GST_STATE_PLAYING);
    return p;
}

static void close_page(Page *p) { gst_element_set_state(p->pipe, GST_STATE_NULL); gst_object_unref(p->pipe); g_free(p); }

static gboolean all_have(Page **ps, int n, int frames, int ms) {
    for (int t = 0; t < ms; t += 20) {
        gboolean ok = TRUE;
        for (int i = 0; i < n; i++) if (g_atomic_int_get(&ps[i]->frames) < frames) ok = FALSE;
        if (ok) return TRUE;
        pump(20);
    }
    return FALSE;
}

static void share(const char *fake) {
    g_setenv("IYAKKI_CAMSHARE_FAKE", fake, TRUE);
    guint o0, b0, f0; iyakki_camshare_stats(&o0, &b0, &f0);

    GstDeviceMonitor *mon = gst_device_monitor_new();
    gst_device_monitor_add_filter(mon, "Video/Source", NULL);
    CHECK(gst_device_monitor_start(mon), "monitor start");
    GList *devs = gst_device_monitor_get_devices(mon);
    CHECK(g_list_length(devs) == 1, "exactly one camera listed, got %u", g_list_length(devs));
    if (!devs) { gst_device_monitor_stop(mon); return; }
    GstDevice *dev = devs->data;
    gchar *name = gst_device_get_display_name(dev);
    CHECK(strcmp(name, "IYAKKI test camera") == 0, "name %s", name);
    g_free(name);
    GstCaps *dc = gst_device_get_caps(dev);
    CHECK(dc && gst_caps_get_size(dc) == 1 && gst_caps_is_fixed(dc), "one fixed mode");
    if (dc) gst_caps_unref(dc);

    /* three pages at once, with different sizes, like the presets of webcamtests.com: one open, all get frames */
    Page *ps[3] = { open_page(dev, 640, 480, 30), open_page(dev, 320, 240, 15), open_page(dev, 1280, 720, 30) };
    CHECK(all_have(ps, 3, 8, 8000), "every page gets frames (%d %d %d)", ps[0]->frames, ps[1]->frames, ps[2]->frames);
    guint o, b, f; iyakki_camshare_stats(&o, &b, &f);
    CHECK(o - o0 == 1, "the camera was opened %u times for three pages", o - o0);
    CHECK(b == b0, "the camera was found busy %u times", b - b0);

    /* a page that comes later joins the open camera */
    Page *late = open_page(dev, 640, 360, 30);
    Page *lp[1] = { late };
    CHECK(all_have(lp, 1, 5, 5000), "a late page gets frames");
    iyakki_camshare_stats(&o, &b, &f);
    CHECK(o - o0 == 1 && b == b0, "still one open (%u) and no busy (%u)", o - o0, b - b0);

    /* one page leaving does not disturb the others */
    close_page(ps[1]);
    gint before = g_atomic_int_get(&ps[0]->frames);
    pump(700);
    CHECK(g_atomic_int_get(&ps[0]->frames) > before + 5, "the others keep running");

    /* all leave: the camera closes after the delay, a new page opens it again (second open overall, never two at once) */
    close_page(ps[0]); close_page(ps[2]); close_page(late);
    pump((CLOSE_DELAY_SECS + 2) * 1000);
    CHECK(g_atomic_int_get(&stat_open_now) == 0, "the camera was closed after the last page left");
    Page *again = open_page(dev, 640, 480, 30);
    Page *ap[1] = { again };
    CHECK(all_have(ap, 1, 8, 8000), "a page after the camera closed gets frames");
    iyakki_camshare_stats(&o, &b, &f);
    CHECK(o - o0 == 2 && b == b0, "opened %u times in all, busy %u", o - o0, b - b0);
    close_page(again);
    pump((CLOSE_DELAY_SECS + 2) * 1000);
    CHECK(g_atomic_int_get(&stat_open_now) == 0, "closed at the end");

    g_list_free_full(devs, gst_object_unref);
    gst_device_monitor_stop(mon);
    gst_object_unref(mon);
}

int main(int argc, char **argv) {
    gst_init(&argc, &argv);
    CHECK(gst_plugin_register_static(GST_VERSION_MAJOR, GST_VERSION_MINOR, "iyakkicamshare", "t", camshare_plugin_init, "1", "unknown", "t", "t", "t"),
          "plugin registers");
    modes();
    /* the second sanity check: without the plugin's help the fake would refuse a second open */
    share("1");
    share("jpeg");
    if (failures) { g_printerr("camshare-check: %d failure(s)\n", failures); return 1; }
    g_print("camshare-check: ok\n");
    return 0;
}
