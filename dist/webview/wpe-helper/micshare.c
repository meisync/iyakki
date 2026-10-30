/*
 * IYAKKI microphones for the web engine: a GStreamer device provider plugin (SPDX-License-Identifier: Apache-2.0).
 *
 * Problem (T2 MacBook, 2026-10): the engine's microphone path made an alsasrc on a raw hw: device. The T2 sound card offers one fixed
 * rate and sample format, so asking it for the 44100 Hz a page wants failed ("Rate doesn't match (requested 44100Hz, get 0Hz)",
 * not-negotiated) and the page recorded nothing; the stock ALSA device provider also opened every hw: stream "for inspection" and
 * logged noise. IYAKKI's own dictation works on the same machine because it records through plughw:C,D, where alsa-lib converts.
 *
 * This plugin registers a device provider ("iyakkimic", rank primary) that finds the capture streams itself (micscan.h: reads
 * /proc/asound, skips HDMI/IEC958/loopbacks, ranks a "mic" first, then USB, probes the hardware without keeping it open) and lists
 * each as an Audio/Source device whose element is a small appsrc. A process-wide hub per device owns the only real source
 * (alsasrc device=plughw:C,D ! audioconvert ! audioresample ! S16LE 48000 Hz ! appsink) and copies every buffer to every live
 * appsrc, so two page requests share one open of a card that cannot be opened twice. The card is opened when the first consumer
 * starts and closed 2 s after the last one stopped; nothing holds it while idle, and nothing here ever opens a playback stream, so
 * IYAKKI's mixer / softvol output is not touched. The helper puts the stock alsadeviceprovider on rank 0.
 *
 * Environment: IYAKKI_MIC_FAKE=1 lists one test microphone made of audiotestsrc that, like a hw device, refuses a second open (the
 * self check, mic-check.c, uses it). IYAKKI_MIC_PROC / IYAKKI_MIC_SYS replace /proc/asound and /sys (tests). Only GStreamer
 * (LGPL, used dynamically) and alsa-lib (LGPL, dynamically) are linked.
 */
#ifndef PACKAGE
#define PACKAGE "iyakki"
#endif
#define IYAKKI_MIC_WITH_ALSA 1
#include <gst/gst.h>
#include <gst/app/gstappsink.h>
#include <gst/app/gstappsrc.h>
#include "micscan.h"

GST_DEBUG_CATEGORY_STATIC(micshare_debug);
#define GST_CAT_DEFAULT micshare_debug

#define CLOSE_DELAY_SECS 2
#define POLL_SECS 3

static gint stat_opens, stat_busy, stat_frames, stat_open_now;
void iyakki_micshare_stats(guint *opens, guint *busy, guint *frames) {
    *opens = (guint)g_atomic_int_get(&stat_opens);
    *busy = (guint)g_atomic_int_get(&stat_busy);
    *frames = (guint)g_atomic_int_get(&stat_frames);
}

/* One line on stderr (the web process's stderr reaches the system log); GST_DEBUG levels do not apply to it. */
static void say(const char *fmt, ...) G_GNUC_PRINTF(1, 2);
static void say(const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    gchar *t = g_strdup_vprintf(fmt, ap);
    va_end(ap);
    g_printerr("iyakki-mic: %s\n", t);
    g_free(t);
}

/* ------------------------------------------------------------------ hub: the only real source of one microphone */
typedef struct { GWeakRef ref; gboolean active; } Entry;

typedef struct Hub {
    gchar *key;             /* plughw:C,D */
    gchar *label;           /* for the person */
    gboolean fake;
    int channels;
    GMutex lock;            /* consumers, close timer */
    GMutex ctl;             /* the real source's state changes (taken before lock) */
    GList *entries;
    guint close_id;
    GstCaps *caps;
    GstElement *pipe;
    gboolean failed;
    gint reported;          /* the first buffer of this open was seen */
} Hub;

static GHashTable *hubs;
static GMutex hubs_lock;

static GstCaps *mode_caps(int channels) {
    return gst_caps_new_simple("audio/x-raw", "format", G_TYPE_STRING, "S16LE", "layout", G_TYPE_STRING, "interleaved",
                               "rate", G_TYPE_INT, 48000, "channels", G_TYPE_INT, channels, NULL);
}

static Hub *hub_get(const char *key, const char *label, gboolean fake, int channels) {
    g_mutex_lock(&hubs_lock);
    if (!hubs) hubs = g_hash_table_new(g_str_hash, g_str_equal);
    Hub *h = g_hash_table_lookup(hubs, key);
    if (!h) {
        h = g_new0(Hub, 1);
        h->key = g_strdup(key); h->fake = fake;
        g_mutex_init(&h->lock); g_mutex_init(&h->ctl);
        g_hash_table_insert(hubs, h->key, h);
    }
    g_mutex_lock(&h->lock);
    h->channels = channels;
    g_free(h->label); h->label = g_strdup(label);
    gst_caps_replace(&h->caps, mode_caps(channels));
    g_mutex_unlock(&h->lock);
    g_mutex_unlock(&hubs_lock);
    return h;
}

static gint hub_live_count(Hub *h, gint *active) {   /* h->lock held; drops dead entries */
    gint live = 0, act = 0;
    for (GList *l = h->entries; l;) {
        Entry *e = l->data; GList *next = l->next;
        GstElement *s = g_weak_ref_get(&e->ref);
        if (!s) { g_weak_ref_clear(&e->ref); g_free(e); h->entries = g_list_delete_link(h->entries, l); }
        else { live++; if (e->active) act++; gst_object_unref(s); }
        l = next;
    }
    if (active) *active = act;
    return live;
}

static void hub_post_error(Hub *h, const char *text) {
    g_mutex_lock(&h->lock);
    for (GList *l = h->entries; l; l = l->next) {
        Entry *e = l->data; GstElement *s = g_weak_ref_get(&e->ref);
        if (!s) continue;
        e->active = FALSE;
        GError *err = g_error_new_literal(GST_RESOURCE_ERROR, GST_RESOURCE_ERROR_OPEN_READ, text);
        gst_element_post_message(s, gst_message_new_error(GST_OBJECT(s), err, "iyakkimic"));
        g_error_free(err);
        gst_object_unref(s);
    }
    g_mutex_unlock(&h->lock);
}

static void hub_stop_locked_ctl(Hub *h) {   /* h->ctl held */
    if (!h->pipe) return;
    GstElement *p = h->pipe; h->pipe = NULL; h->failed = FALSE;
    gst_element_set_state(p, GST_STATE_NULL);
    gst_object_unref(p);
    if (h->fake) g_atomic_int_add(&stat_open_now, -1);
    say("closed %s (no page is listening)", h->key);
}

static gboolean hub_close_cb(gpointer data) {
    Hub *h = data;
    g_mutex_lock(&h->ctl);
    g_mutex_lock(&h->lock);
    h->close_id = 0;
    gint act = 0; hub_live_count(h, &act);
    g_mutex_unlock(&h->lock);
    if (act == 0 || h->failed) hub_stop_locked_ctl(h);
    g_mutex_unlock(&h->ctl);
    return G_SOURCE_REMOVE;
}

static void hub_schedule_close(Hub *h) {   /* h->lock held */
    if (!h->close_id) h->close_id = g_timeout_add_seconds(CLOSE_DELAY_SECS, hub_close_cb, h);
}

static GstFlowReturn hub_new_sample(GstAppSink *sink, gpointer data) {
    Hub *h = data;
    GstSample *sample = gst_app_sink_pull_sample(sink);
    if (!sample) return GST_FLOW_OK;
    GstBuffer *buf = gst_sample_get_buffer(sample);
    g_atomic_int_inc(&stat_frames);
    if (g_atomic_int_compare_and_exchange(&h->reported, 0, 1)) micscan_write_status(NULL, NULL);   /* audio arrives: nothing to report */
    GPtrArray *live = g_ptr_array_new();   /* Entry*, GstElement* pairs; strong refs taken under the lock, pushed outside it */
    g_mutex_lock(&h->lock);
    for (GList *l = h->entries; l; l = l->next) {
        Entry *e = l->data; GstElement *s = g_weak_ref_get(&e->ref);
        if (!s) continue;
        g_ptr_array_add(live, e); g_ptr_array_add(live, s);
    }
    g_mutex_unlock(&h->lock);
    gboolean flushed[64] = { 0 };
    for (guint i = 0; i + 1 < live->len; i += 2) {
        GstElement *s = live->pdata[i + 1];
        GstBuffer *copy = gst_buffer_copy(buf);   /* shares the memory; each consumer timestamps with its own do-timestamp */
        GST_BUFFER_PTS(copy) = GST_CLOCK_TIME_NONE; GST_BUFFER_DTS(copy) = GST_CLOCK_TIME_NONE;
        GstFlowReturn r = gst_app_src_push_buffer(GST_APP_SRC(s), copy);
        if ((r == GST_FLOW_FLUSHING || r == GST_FLOW_EOS) && i / 2 < 64) flushed[i / 2] = TRUE;
    }
    g_mutex_lock(&h->lock);
    for (guint i = 0; i + 1 < live->len; i += 2) if (i / 2 < 64 && flushed[i / 2]) ((Entry *)live->pdata[i])->active = FALSE;
    gint act = 0; hub_live_count(h, &act);
    if (act == 0) hub_schedule_close(h);
    g_mutex_unlock(&h->lock);
    for (guint i = 1; i < live->len; i += 2) gst_object_unref(live->pdata[i]);
    g_ptr_array_free(live, TRUE);
    gst_sample_unref(sample);
    return GST_FLOW_OK;
}

static gboolean hub_failed_cb(gpointer data) {
    Hub *h = data;
    g_mutex_lock(&h->ctl);
    if (h->failed) hub_stop_locked_ctl(h);
    g_mutex_unlock(&h->ctl);
    return G_SOURCE_REMOVE;
}

static GstBusSyncReply hub_bus(GstBus *bus, GstMessage *msg, gpointer data) {
    Hub *h = data;
    if (GST_MESSAGE_TYPE(msg) == GST_MESSAGE_ERROR || GST_MESSAGE_TYPE(msg) == GST_MESSAGE_EOS) {
        GError *err = NULL; gchar *dbg = NULL;
        if (GST_MESSAGE_TYPE(msg) == GST_MESSAGE_ERROR) gst_message_parse_error(msg, &err, &dbg);
        say("%s stopped: %s%s%s", h->key, err ? err->message : "end of stream", dbg ? " / " : "", dbg ? dbg : "");
        if (!h->failed) {
            h->failed = TRUE;
            { gchar *first = dbg ? g_strndup(dbg, strcspn(dbg, "\n")) : NULL;
              gchar *why = g_strdup_printf("%s%s%s", err ? err->message : "the stream ended", first ? ": " : "", first ? first : "");
              micscan_write_status(h->label ? h->label : h->key, why); g_free(why); g_free(first); }
            hub_post_error(h, err ? err->message : "the microphone stopped");
            g_idle_add(hub_failed_cb, h);
        }
        g_clear_error(&err); g_free(dbg);
    }
    gst_message_unref(msg);
    return GST_BUS_DROP;
}

/* Open the microphone if it is not open (the only place that opens it). Returns whether it is open. */
static gboolean hub_start(Hub *h) {
    gboolean ok = FALSE;
    g_mutex_lock(&h->ctl);
    if (h->pipe && h->failed) hub_stop_locked_ctl(h);
    if (h->pipe) { g_mutex_unlock(&h->ctl); return TRUE; }
    if (h->fake && g_atomic_int_add(&stat_open_now, 1) > 0) {   /* a hw capture device refuses a second open: so does the fake one */
        g_atomic_int_add(&stat_open_now, -1);
        g_atomic_int_inc(&stat_busy);
        goto out;
    }
    g_mutex_lock(&h->lock);
    gchar *caps = h->caps ? gst_caps_to_string(h->caps) : g_strdup("audio/x-raw");
    g_mutex_unlock(&h->lock);
    gchar *desc = g_strdup_printf("%s ! audioconvert ! audioresample ! capsfilter caps=\"%s\" ! appsink name=out",
                                  h->fake ? (g_strcmp0(g_getenv("IYAKKI_MIC_FAKE"), "fail") ? "audiotestsrc is-live=true wave=sine freq=440 samplesperbuffer=480"
                                                                  : "audiotestsrc is-live=true samplesperbuffer=480 ! identity error-after=3")
                                  : "alsasrc name=mic", caps);
    g_free(caps);
    GError *err = NULL;
    GstElement *pipe = gst_parse_launch(desc, &err);
    g_free(desc);
    if (!pipe) {
        say("%s: cannot build the capture pipeline: %s", h->key, err ? err->message : "?");
        g_clear_error(&err);
        if (h->fake) g_atomic_int_add(&stat_open_now, -1);
        goto out;
    }
    g_clear_error(&err);
    if (!h->fake) {
        GstElement *mic = gst_bin_get_by_name(GST_BIN(pipe), "mic");
        g_object_set(mic, "device", h->key, "buffer-time", (gint64)100000, "latency-time", (gint64)20000, NULL);
        gst_object_unref(mic);
    }
    GstElement *sink = gst_bin_get_by_name(GST_BIN(pipe), "out");
    g_object_set(sink, "sync", FALSE, "max-buffers", 8, "drop", TRUE, "emit-signals", FALSE, NULL);
    GstAppSinkCallbacks cb = { 0 }; cb.new_sample = hub_new_sample;
    gst_app_sink_set_callbacks(GST_APP_SINK(sink), &cb, h, NULL);
    gst_object_unref(sink);
    GstBus *bus = gst_pipeline_get_bus(GST_PIPELINE(pipe));
    gst_bus_set_sync_handler(bus, hub_bus, h, NULL);
    gst_object_unref(bus);
    h->failed = FALSE;
    g_atomic_int_set(&h->reported, 0);
    g_atomic_int_inc(&stat_opens);
    say("opening %s as S16LE 48000 Hz %d ch for a page request", h->key, h->channels);
    if (gst_element_set_state(pipe, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
        /* the bus handler has already said why (an ALSA open error arrives there); make sure the person's page hears it */
        say("%s: the microphone did not start", h->key);
        gst_element_set_state(pipe, GST_STATE_NULL);
        gst_object_unref(pipe);
        if (h->fake) g_atomic_int_add(&stat_open_now, -1);
        goto out;
    }
    h->pipe = pipe;
    ok = TRUE;
out:
    g_mutex_unlock(&h->ctl);
    if (!ok) { micscan_write_status(h->label ? h->label : h->key, "the microphone could not be started"); hub_post_error(h, "the microphone could not be opened"); }
    return ok;
}

static void consumer_need_data(GstAppSrc *src, guint length, gpointer data) {
    Hub *h = data;
    g_mutex_lock(&h->lock);
    for (GList *l = h->entries; l; l = l->next) {
        Entry *e = l->data; GstElement *s = g_weak_ref_get(&e->ref);
        if (s == GST_ELEMENT(src)) e->active = TRUE;
        if (s) gst_object_unref(s);
    }
    g_mutex_unlock(&h->lock);
    hub_start(h);
}

static GstElement *hub_attach(Hub *h, const char *name) {
    GstElement *src = gst_element_factory_make("appsrc", name);
    if (!src) return NULL;
    g_mutex_lock(&h->lock);
    g_object_set(src, "is-live", TRUE, "format", GST_FORMAT_TIME, "do-timestamp", TRUE, "caps", h->caps, "max-buffers", (guint64)8,
                 "max-bytes", (guint64)0, "leaky-type", GST_APP_LEAKY_TYPE_DOWNSTREAM, "block", FALSE, "emit-signals", TRUE,
                 "min-latency", (gint64)0, NULL);
    Entry *e = g_new0(Entry, 1);
    g_weak_ref_init(&e->ref, src);
    h->entries = g_list_append(h->entries, e);
    hub_live_count(h, NULL);
    g_mutex_unlock(&h->lock);
    g_signal_connect(src, "need-data", G_CALLBACK(consumer_need_data), h);
    return src;
}

/* ------------------------------------------------------------------ device */
typedef struct { GstDevice parent; Hub *hub; } IyakkiMicDevice;
typedef struct { GstDeviceClass parent_class; } IyakkiMicDeviceClass;
G_DEFINE_TYPE(IyakkiMicDevice, iyakki_mic_device, GST_TYPE_DEVICE)

static GstElement *mic_device_create_element(GstDevice *dev, const gchar *name) { return hub_attach(((IyakkiMicDevice *)dev)->hub, name); }
static void iyakki_mic_device_class_init(IyakkiMicDeviceClass *k) { GST_DEVICE_CLASS(k)->create_element = mic_device_create_element; }
static void iyakki_mic_device_init(IyakkiMicDevice *d) { (void)d; }

static GstDevice *make_device(const char *key, const char *label, int channels, gboolean fake, gboolean best) {
    Hub *h = hub_get(key, label, fake, channels);
    GstCaps *caps = mode_caps(channels);
    GstStructure *props = gst_structure_new("properties", "device.api", G_TYPE_STRING, "alsa", "device.path", G_TYPE_STRING, key,
                                            "is-default", G_TYPE_BOOLEAN, best, NULL);
    IyakkiMicDevice *d = g_object_new(iyakki_mic_device_get_type(), "display-name", label, "caps", caps, "device-class", "Audio/Source",
                                      "properties", props, NULL);
    d->hub = h;
    gst_structure_free(props);
    gst_caps_unref(caps);
    return gst_object_ref_sink(GST_DEVICE(d));   /* the caller owns one reference */
}

/* ------------------------------------------------------------------ provider */
typedef struct { GstDeviceProvider parent; GHashTable *known; GMutex lock; guint poll_id; } IyakkiMicProvider;
typedef struct { GstDeviceProviderClass parent_class; } IyakkiMicProviderClass;
G_DEFINE_TYPE(IyakkiMicProvider, iyakki_mic_provider, GST_TYPE_DEVICE_PROVIDER)

static const char *proc_root(void) { const char *e = g_getenv("IYAKKI_MIC_PROC"); return e && *e ? e : "/proc/asound"; }
static const char *sys_root(void) { const char *e = g_getenv("IYAKKI_MIC_SYS"); return e && *e ? e : "/sys"; }

static GList *scan_devices(gboolean log, GList **keys, gboolean probe) {
    GList *out = NULL;
    if (g_getenv("IYAKKI_MIC_FAKE")) {
        out = g_list_append(out, make_device("fake:mic", "IYAKKI test microphone", 1, TRUE, TRUE));
        if (keys) *keys = g_list_append(*keys, g_strdup("fake:mic"));
        return out;
    }
    IyakkiMicScan *s = micscan_run(proc_root(), sys_root(), probe);
    if (log) { gchar *r = micscan_report(s, "provider scan"); say("%s", r); g_free(r); }
    for (guint i = 0; i < s->mics->len; i++) {
        IyakkiMic *m = s->mics->pdata[i];
        out = g_list_append(out, make_device(m->alsa, m->label, micscan_channels(m), FALSE, i == 0));
        if (keys) *keys = g_list_append(*keys, g_strdup_printf("%s|%s|%d", m->alsa, m->label, i == 0));
    }
    micscan_free(s);
    return out;
}

static GList *mic_provider_probe(GstDeviceProvider *p) { (void)p; return scan_devices(FALSE, NULL, TRUE); }

static void mic_sync(IyakkiMicProvider *self, gboolean first) {
    /* the periodic look probes too (a non-blocking open that is closed at once; a busy card counts as present) */
    GList *keys = NULL, *devs = scan_devices(first, &keys, TRUE);
    GHashTable *now = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
    GList *k = keys, *d = devs;
    for (; k && d; k = k->next, d = d->next) {
        gchar *key = k->data;
        g_mutex_lock(&self->lock);
        gboolean have = g_hash_table_contains(self->known, key);
        if (!have) g_hash_table_insert(self->known, g_strdup(key), gst_object_ref(d->data));
        g_mutex_unlock(&self->lock);
        if (!have) gst_device_provider_device_add(GST_DEVICE_PROVIDER(self), d->data);
        g_hash_table_add(now, g_strdup(key));
    }
    GList *gone = NULL;
    g_mutex_lock(&self->lock);
    GHashTableIter it; gpointer kk, vv;
    g_hash_table_iter_init(&it, self->known);
    while (g_hash_table_iter_next(&it, &kk, &vv)) if (!g_hash_table_contains(now, kk)) gone = g_list_append(gone, g_strdup(kk));
    g_mutex_unlock(&self->lock);
    for (GList *g = gone; g; g = g->next) {
        g_mutex_lock(&self->lock);
        GstDevice *dev = g_hash_table_lookup(self->known, g->data);
        if (dev) gst_object_ref(dev);
        g_hash_table_remove(self->known, g->data);
        g_mutex_unlock(&self->lock);
        if (dev) { gst_device_provider_device_remove(GST_DEVICE_PROVIDER(self), dev); gst_object_unref(dev); }
    }
    g_list_free_full(gone, g_free);
    g_list_free_full(keys, g_free);
    g_list_free_full(devs, gst_object_unref);
    g_hash_table_destroy(now);
}

static gboolean mic_poll(gpointer data) { mic_sync(data, FALSE); return G_SOURCE_CONTINUE; }

static gboolean mic_provider_start(GstDeviceProvider *p) {
    IyakkiMicProvider *self = (IyakkiMicProvider *)p;
    mic_sync(self, TRUE);
    if (!g_getenv("IYAKKI_MIC_FAKE") && !self->poll_id) self->poll_id = g_timeout_add_seconds(POLL_SECS, mic_poll, self);   /* a USB microphone plugged in later */
    return TRUE;
}

static void mic_provider_stop(GstDeviceProvider *p) {
    IyakkiMicProvider *self = (IyakkiMicProvider *)p;
    if (self->poll_id) { g_source_remove(self->poll_id); self->poll_id = 0; }
    g_mutex_lock(&self->lock);
    g_hash_table_remove_all(self->known);
    g_mutex_unlock(&self->lock);
}

static void iyakki_mic_provider_class_init(IyakkiMicProviderClass *k) {
    GstDeviceProviderClass *pc = GST_DEVICE_PROVIDER_CLASS(k);
    pc->probe = mic_provider_probe; pc->start = mic_provider_start; pc->stop = mic_provider_stop;
    gst_device_provider_class_set_static_metadata(pc, "IYAKKI microphones", "Audio/Source",
        "Capture streams found in /proc/asound, opened through plughw and shared between page requests", "IYAKKI");
}
static void iyakki_mic_provider_init(IyakkiMicProvider *p) {
    p->known = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, gst_object_unref);
    g_mutex_init(&p->lock);
}

/* ------------------------------------------------------------------ plugin */
static gboolean micshare_plugin_init(GstPlugin *plugin) {
    GST_DEBUG_CATEGORY_INIT(micshare_debug, "iyakkimic", 0, "IYAKKI microphones");
    return gst_device_provider_register(plugin, "iyakkimic", GST_RANK_PRIMARY + 1, iyakki_mic_provider_get_type());
}

#ifndef MICSHARE_NO_PLUGIN_DEFINE
GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, iyakkimic, "IYAKKI microphones: capture through plughw, one open for every page request",
                  micshare_plugin_init, "1", "unknown", "IYAKKI", "iyakki")
#endif
