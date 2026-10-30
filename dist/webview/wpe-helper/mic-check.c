/*
 * Self check of micscan.h and micshare.c (SPDX-License-Identifier: Apache-2.0): `make check` in this folder.
 * Part 1 feeds fake /proc/asound trees to the scan (a T2-like layout, a PC with digital outputs, USB, nothing at all). Part 2 runs the
 * device provider with a test microphone made of audiotestsrc that refuses a second open like a hw capture device: three pages that
 * ask for different rates and channel counts share one open, the card closes after the last one, a later page opens it again.
 * Needs GStreamer with audiotestsrc, audioconvert, audioresample. No sound card.
 */
#define MICSHARE_NO_PLUGIN_DEFINE
#include "micshare.c"

static int failures;
#define CHECK(c, ...) do { if (!(c)) { failures++; g_printerr("FAIL %s:%d: ", __FILE__, __LINE__); g_printerr(__VA_ARGS__); g_printerr("\n"); } } while (0)

static void pump(int ms) {
    gint64 end = g_get_monotonic_time() + (gint64)ms * 1000;
    while (g_get_monotonic_time() < end) { while (g_main_context_iteration(NULL, FALSE)); g_usleep(5000); }
}

/* ------------------------------------------------------------------ part 1: the scan */
static void put(const char *root, const char *rel, const char *text) {
    gchar *p = g_build_filename(root, rel, NULL), *d = g_path_get_dirname(p);
    g_mkdir_with_parents(d, 0755);
    g_file_set_contents(p, text, -1, NULL);
    g_free(p); g_free(d);
}

static void pcm(const char *root, int card, int dev, char dir, const char *name) {
    gchar *rel = g_strdup_printf("asound/card%d/pcm%d%c/info", card, dev, dir);
    gchar *t = g_strdup_printf("card: %d\ndevice: %d\nsubdevice: 0\nstream: %s\nid: %s\nname: %s\nsubname: subdevice #0\nclass: 0\nsubclass: 0\nsubdevices_count: 1\nsubdevices_avail: 1\n",
                               card, dev, dir == 'c' ? "CAPTURE" : "PLAYBACK", name, name);
    put(root, rel, t);
    g_free(rel); g_free(t);
}

static IyakkiMicScan *scan_tree(const char *root) {
    gchar *proc = g_build_filename(root, "asound", NULL), *sys = g_build_filename(root, "sys", NULL);
    IyakkiMicScan *s = micscan_run(proc, sys, FALSE);
    g_free(proc); g_free(sys);
    return s;
}

static const char *alsa_at(IyakkiMicScan *s, guint i) { return i < s->mics->len ? ((IyakkiMic *)s->mics->pdata[i])->alsa : "(none)"; }
static const char *label_at(IyakkiMicScan *s, guint i) { return i < s->mics->len ? ((IyakkiMic *)s->mics->pdata[i])->label : "(none)"; }

static gboolean skipped_has(IyakkiMicScan *s, const char *needle) {
    for (guint i = 0; i < s->skipped->len; i++) if (strstr(s->skipped->pdata[i], needle)) return TRUE;
    return FALSE;
}

static void scans(void) {
    /* the 2020 MacBook Pro (T2): the sound card first, the webcam's USB microphone, the HDMI of the GPU */
    gchar *root = g_dir_make_tmp("mic-check-XXXXXX", NULL);
    put(root, "asound/cards", " 0 [AppleT2x4      ]: AppleT2x4 - Apple T2 Audio\n                      Apple T2 Audio\n"
        " 1 [Nova           ]: USB-Audio - EMEET SmartCam Nova 4K\n                      EMEET SmartCam Nova 4K at usb-0000:00:14.0-2, high speed\n"
        " 2 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x600c110000 irq 140\n");
    pcm(root, 0, 0, 'p', "Speaker"); pcm(root, 0, 1, 'c', "Digital Mic"); pcm(root, 0, 2, 'p', "Headphones");
    pcm(root, 0, 3, 'c', "Codec Input"); pcm(root, 0, 4, 'p', "Bridge Loopback"); pcm(root, 0, 4, 'c', "Bridge Loopback");
    pcm(root, 1, 0, 'c', "USB Audio"); pcm(root, 1, 0, 'p', "USB Audio");
    pcm(root, 2, 3, 'p', "HDMI 0"); pcm(root, 2, 7, 'p', "HDMI 1");
    put(root, "sys/module/apple_bce/parameters/capture", "N\n");
    IyakkiMicScan *s = scan_tree(root);
    CHECK(s->mics->len == 1 && !strcmp(alsa_at(s, 0), "plughw:1,0"), "T2 capture off: only the USB microphone, got %u first %s", s->mics->len, alsa_at(s, 0));
    CHECK(!strcmp(label_at(s, 0), "EMEET SmartCam Nova 4K (USB microphone)"), "usb label: %s", label_at(s, 0));
    CHECK(skipped_has(s, "0,1 'Digital Mic' (T2 capture switched off") && skipped_has(s, "Bridge Loopback' (loopback)"), "reasons are told");
    gchar *rep = micscan_report(s, "t");
    CHECK(strstr(rep, "capture devices (1)") && strstr(rep, "chosen plughw:1,0"), "report: %s", rep);
    g_free(rep);
    micscan_free(s);

    put(root, "sys/module/apple_bce/parameters/capture", "Y\n");
    s = scan_tree(root);
    CHECK(s->mics->len == 3, "T2 capture on: Digital Mic, USB, Codec Input; got %u", s->mics->len);
    CHECK(!strcmp(alsa_at(s, 0), "plughw:0,1") && !strcmp(alsa_at(s, 1), "plughw:1,0") && !strcmp(alsa_at(s, 2), "plughw:0,3"),
          "order %s %s %s", alsa_at(s, 0), alsa_at(s, 1), alsa_at(s, 2));
    CHECK(!strcmp(label_at(s, 0), "Built-in microphone (Apple T2 Audio)"), "T2 label: %s", label_at(s, 0));
    CHECK(!strcmp(label_at(s, 2), "Codec Input (Apple T2 Audio)"), "codec label: %s", label_at(s, 2));
    CHECK(skipped_has(s, "Bridge Loopback"), "the loopback is skipped");
    CHECK(((IyakkiMic *)s->mics->pdata[0])->t2 && !((IyakkiMic *)s->mics->pdata[0])->probed && micscan_channels(s->mics->pdata[0]) == 1, "T2 is never probed and is mono");
    gchar *rep2 = micscan_report(s, "t");
    CHECK(strstr(rep2, "chosen plughw:0,1") && strstr(rep2, "S16LE 48000 Hz 1 ch") && strstr(rep2, "not opened (T2"), "report: %s", rep2);
    g_free(rep2);
    micscan_free(s);

    /* the person's choice (the Voice tile and the Browser write it to the shared folder) goes first; an absent one changes nothing */
    gchar *shared = g_dir_make_tmp("mic-check-XXXXXX", NULL);
    g_setenv("IYAKKI_MIC_DIR", shared, TRUE);
    put(shared, MICSCAN_CHOICE_FILE, "plughw:0,3\n");
    s = scan_tree(root);
    CHECK(s->mics->len == 3 && !strcmp(alsa_at(s, 0), "plughw:0,3") && !strcmp(alsa_at(s, 1), "plughw:0,1") && s->choice_found,
          "the choice is first: %s %s", alsa_at(s, 0), alsa_at(s, 1));
    gchar *rep4 = micscan_report(s, "t");
    CHECK(strstr(rep4, "the person's choice plughw:0,3 is first") && strstr(rep4, "chosen plughw:0,3"), "report: %s", rep4);
    g_free(rep4); micscan_free(s);
    put(shared, MICSCAN_CHOICE_FILE, "plughw:7,7\n");
    s = scan_tree(root);
    CHECK(!strcmp(alsa_at(s, 0), "plughw:0,1") && !s->choice_found, "an absent choice leaves the automatic order: %s", alsa_at(s, 0));
    gchar *rep5 = micscan_report(s, "t");
    CHECK(strstr(rep5, "not present (automatic order used)"), "report: %s", rep5);
    g_free(rep5); micscan_free(s);
    /* a microphone the person marked "don't use" (a defective one) is not listed and not opened, and is named among the skipped; with
     * the file gone it is back; the shared folder falls back to the temp folder when /dev/shm is missing (devlist_dir) */
    put(shared, MICSCAN_CHOICE_FILE, "plughw:0,3\n");
    put(shared, MICSCAN_HIDDEN_FILE, "plughw:0,3\n  \nplughw:9,9\n");
    s = scan_tree(root);
    CHECK(s->mics->len == 2 && skipped_has(s, "hidden by the person"), "the hidden microphone is skipped: %u", s->mics->len);
    for (guint i = 0; i < s->mics->len; i++) CHECK(strcmp(alsa_at(s, i), "plughw:0,3"), "the hidden microphone is not listed");
    CHECK(!s->choice_found, "a hidden microphone cannot be the person's choice");
    gchar *rep6 = micscan_report(s, "t");
    CHECK(strstr(rep6, "hidden by the person"), "report: %s", rep6);
    g_free(rep6); micscan_free(s);
    gchar *hp = g_build_filename(shared, MICSCAN_HIDDEN_FILE, NULL); g_remove(hp); g_free(hp);
    s = scan_tree(root);
    CHECK(s->mics->len == 3, "without the file the microphone is back: %u", s->mics->len);
    micscan_free(s);
    g_unsetenv("IYAKKI_MIC_DIR");
    CHECK(!strcmp(devlist_dir(), g_file_test("/dev/shm", G_FILE_TEST_IS_DIR) ? "/dev/shm" : g_get_tmp_dir()), "the shared folder rule matches the shell's: %s", devlist_dir());
    { gchar *cmd = g_strdup_printf("rm -rf '%s'", shared); if (system(cmd)) {} g_free(cmd); g_free(shared); }

    /* a PC: analog capture on the codec, a digital S/PDIF capture, HDMI playback only, a loopback card */
    gchar *pc = g_dir_make_tmp("mic-check-XXXXXX", NULL);
    put(pc, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7e10000 irq 33\n"
        " 1 [Loopback       ]: Loopback - Loopback\n                      Loopback 1\n");
    pcm(pc, 0, 0, 'c', "ALC3246 Analog"); pcm(pc, 0, 0, 'p', "ALC3246 Analog"); pcm(pc, 0, 1, 'c', "ALC3246 Digital (IEC958)");
    pcm(pc, 0, 3, 'p', "HDMI 0"); pcm(pc, 1, 0, 'c', "Loopback PCM");
    s = scan_tree(pc);
    CHECK(s->mics->len == 1 && !strcmp(alsa_at(s, 0), "plughw:0,0"), "PC: only the analog input, got %u %s", s->mics->len, alsa_at(s, 0));
    CHECK(!strcmp(label_at(s, 0), "ALC3246 Analog (HDA Intel PCH)"), "pc label: %s", label_at(s, 0));
    CHECK(skipped_has(s, "iec958") && skipped_has(s, "loopback"), "digital and loopback are skipped");
    micscan_free(s);

    /* nothing at all, and a card with playback only */
    gchar *none = g_dir_make_tmp("mic-check-XXXXXX", NULL);
    s = scan_tree(none);
    CHECK(s->mics->len == 0, "no sound system, no microphone");
    gchar *rep3 = micscan_report(s, "t");
    CHECK(strstr(rep3, "none") && strstr(rep3, "chosen none"), "report: %s", rep3);
    g_free(rep3); micscan_free(s);
    put(none, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
    pcm(none, 0, 3, 'p', "HDMI 0");
    s = scan_tree(none);
    CHECK(s->mics->len == 0, "playback only, no microphone");
    micscan_free(s);
    const char *roots[] = { root, pc, none };
    for (int i = 0; i < 3; i++) { gchar *cmd = g_strdup_printf("rm -rf '%s'", roots[i]); if (system(cmd)) {} g_free(cmd); }
    g_free(root); g_free(pc); g_free(none);
}

/* ------------------------------------------------------------------ part 2: the provider and the hub */
typedef struct { GstElement *pipe; gint bufs; gint rate, ch; } Page;

static GstFlowReturn count_buf(GstAppSink *s, gpointer d) {
    GstSample *sm = gst_app_sink_pull_sample(s);
    Page *p = d;
    if (sm) {
        GstStructure *st = gst_caps_get_structure(gst_sample_get_caps(sm), 0);
        gint r = 0, c = 0; gst_structure_get_int(st, "rate", &r); gst_structure_get_int(st, "channels", &c);
        g_atomic_int_set(&p->rate, r); g_atomic_int_set(&p->ch, c);
        g_atomic_int_inc(&p->bufs); gst_sample_unref(sm);
    }
    return GST_FLOW_OK;
}

/* a page as WebKit builds it: device element ! audioconvert ! audioresample ! capsfilter (its own rate and channels) ! appsink */
static Page *open_page(GstDevice *dev, int rate, int ch) {
    Page *p = g_new0(Page, 1);
    GstElement *src = gst_device_create_element(dev, NULL), *conv = gst_element_factory_make("audioconvert", NULL);
    GstElement *res = gst_element_factory_make("audioresample", NULL), *cf = gst_element_factory_make("capsfilter", NULL);
    GstElement *sink = gst_element_factory_make("appsink", NULL);
    p->pipe = gst_pipeline_new(NULL);
    GstCaps *caps = gst_caps_new_simple("audio/x-raw", "rate", G_TYPE_INT, rate, "channels", G_TYPE_INT, ch, NULL);
    g_object_set(cf, "caps", caps, NULL); g_object_set(sink, "sync", FALSE, "emit-signals", FALSE, NULL);
    gst_caps_unref(caps);
    GstAppSinkCallbacks cb = { 0 }; cb.new_sample = count_buf; gst_app_sink_set_callbacks(GST_APP_SINK(sink), &cb, p, NULL);
    gst_bin_add_many(GST_BIN(p->pipe), src, conv, res, cf, sink, NULL);
    gst_element_link_many(src, conv, res, cf, sink, NULL);
    gst_element_set_state(p->pipe, GST_STATE_PLAYING);
    return p;
}
static void close_page(Page *p) { gst_element_set_state(p->pipe, GST_STATE_NULL); gst_object_unref(p->pipe); g_free(p); }

static gboolean all_have(Page **ps, int n, int bufs, int ms) {
    for (int t = 0; t < ms; t += 20) {
        gboolean ok = TRUE;
        for (int i = 0; i < n; i++) if (g_atomic_int_get(&ps[i]->bufs) < bufs) ok = FALSE;
        if (ok) return TRUE;
        pump(20);
    }
    return FALSE;
}

static void provider(void) {
    g_setenv("IYAKKI_MIC_FAKE", "1", TRUE);
    guint o0, b0, f0; iyakki_micshare_stats(&o0, &b0, &f0);
    GstDeviceMonitor *mon = gst_device_monitor_new();
    gst_device_monitor_add_filter(mon, "Audio/Source", NULL);
    CHECK(gst_device_monitor_start(mon), "monitor start");
    GList *devs = gst_device_monitor_get_devices(mon);
    GstDevice *dev = NULL;   /* the host may have other providers (PipeWire): ours is the one with the test name */
    guint ours = 0;
    for (GList *l = devs; l; l = l->next) {
        gchar *n = gst_device_get_display_name(l->data);
        if (!strcmp(n, "IYAKKI test microphone")) { dev = l->data; ours++; }
        g_free(n);
    }
    CHECK(ours == 1, "exactly one test microphone listed, got %u of %u devices", ours, g_list_length(devs));
    if (!dev) { g_list_free_full(devs, gst_object_unref); gst_device_monitor_stop(mon); return; }
    gchar *name = gst_device_get_display_name(dev);
    CHECK(!strcmp(name, "IYAKKI test microphone"), "name %s", name);
    g_free(name);
    GstCaps *dc = gst_device_get_caps(dev);
    CHECK(dc && gst_caps_get_size(dc) == 1 && gst_caps_is_fixed(dc), "one fixed mode");
    if (dc) gst_caps_unref(dc);

    /* three pages: 44100 Hz stereo, 16000 Hz mono (the dictation's), 48000 Hz stereo: one open, every page gets its own format */
    Page *ps[3] = { open_page(dev, 44100, 2), open_page(dev, 16000, 1), open_page(dev, 48000, 2) };
    CHECK(all_have(ps, 3, 8, 8000), "every page gets audio (%d %d %d)", ps[0]->bufs, ps[1]->bufs, ps[2]->bufs);
    CHECK(ps[0]->rate == 44100 && ps[0]->ch == 2 && ps[1]->rate == 16000 && ps[1]->ch == 1 && ps[2]->rate == 48000 && ps[2]->ch == 2,
          "conversions: %d/%d %d/%d %d/%d", ps[0]->rate, ps[0]->ch, ps[1]->rate, ps[1]->ch, ps[2]->rate, ps[2]->ch);
    guint o, b, f; iyakki_micshare_stats(&o, &b, &f);
    CHECK(o - o0 == 1 && b == b0, "one open for three pages (%u opens, %u busy)", o - o0, b - b0);

    close_page(ps[1]);
    gint before = g_atomic_int_get(&ps[0]->bufs);
    pump(600);
    CHECK(g_atomic_int_get(&ps[0]->bufs) > before + 5, "the others keep running");

    close_page(ps[0]); close_page(ps[2]);
    pump((CLOSE_DELAY_SECS + 2) * 1000);
    CHECK(g_atomic_int_get(&stat_open_now) == 0, "the microphone was closed after the last page left");
    Page *again = open_page(dev, 44100, 1);
    Page *ap[1] = { again };
    CHECK(all_have(ap, 1, 8, 8000), "a page after the close gets audio");
    iyakki_micshare_stats(&o, &b, &f);
    CHECK(o - o0 == 2 && b == b0, "two opens in all, never two at once (%u, busy %u)", o - o0, b - b0);
    close_page(again);
    pump((CLOSE_DELAY_SECS + 2) * 1000);
    CHECK(g_atomic_int_get(&stat_open_now) == 0, "closed at the end");
    g_list_free_full(devs, gst_object_unref);
    gst_device_monitor_stop(mon);
    gst_object_unref(mon);
}

/* a microphone that fails: the page hears an error and the shell finds the device name and the reason in the status file */
static void failure(void) {
    gchar *shared = g_dir_make_tmp("mic-check-XXXXXX", NULL);
    g_setenv("IYAKKI_MIC_DIR", shared, TRUE);
    g_setenv("IYAKKI_MIC_FAKE", "fail", TRUE);
    GstDeviceMonitor *mon = gst_device_monitor_new();
    gst_device_monitor_add_filter(mon, "Audio/Source", NULL);
    gst_device_monitor_start(mon);
    GList *devs = gst_device_monitor_get_devices(mon);
    GstDevice *dev = NULL;
    for (GList *l = devs; l; l = l->next) { gchar *n = gst_device_get_display_name(l->data); if (!strcmp(n, "IYAKKI test microphone")) dev = l->data; g_free(n); }
    CHECK(dev != NULL, "the failing test microphone is listed");
    if (dev) {
        Page *p = open_page(dev, 44100, 1);
        gchar *f = g_build_filename(shared, MICSCAN_STATUS_FILE, NULL), *t = NULL;
        for (int i = 0; i < 100 && !g_file_get_contents(f, &t, NULL, NULL); i++) { t = NULL; pump(50); }
        CHECK(t && g_str_has_prefix(t, "IYAKKI test microphone\t") && strlen(t) > 30, "status names the device and a reason: [%s]", t ? t : "(none)");
        g_free(t); g_free(f);
        close_page(p);
    }
    g_list_free_full(devs, gst_object_unref);
    gst_device_monitor_stop(mon);
    gst_object_unref(mon);
    g_unsetenv("IYAKKI_MIC_DIR");
    { gchar *cmd = g_strdup_printf("rm -rf '%s'", shared); if (system(cmd)) {} g_free(cmd); g_free(shared); }
}

int main(int argc, char **argv) {
    gst_init(&argc, &argv);
    scans();
    CHECK(gst_plugin_register_static(GST_VERSION_MAJOR, GST_VERSION_MINOR, "iyakkimic", "t", micshare_plugin_init, "1", "unknown", "t", "t", "t"),
          "plugin registers");
    provider();
    pump((CLOSE_DELAY_SECS + 2) * 1000);
    failure();
    if (failures) { g_printerr("mic-check: %d failure(s)\n", failures); return 1; }
    g_print("mic-check: ok\n");
    return 0;
}
