/*
 * IYAKKI shared camera: a GStreamer plugin loaded by the WebKit web process (SPDX-License-Identifier: Apache-2.0).
 *
 * Problem: WebKit's GStreamer capture opens one v4l2src per getUserMedia call. A UVC camera node can be opened and format-set by one
 * streamer only, so the second call fails ("Device is busy", "S_FMT failed") and the page waits for ever (webcamtests.com asks twice).
 *
 * This plugin registers a device provider ("iyakkicamshare", rank primary) that wraps the device provider of v4l2 (the helper puts the
 * raw v4l2 provider on rank 0 so the engine's device monitor lists only these wrappers). A wrapper device offers ONE fixed mode and
 * its create_element() gives each caller a small appsrc. A process-wide hub per camera owns the only real source
 * (v4l2src ! capsfilter ! appsink) and copies every frame to every live appsrc (leaky, so a slow page never stalls the others).
 * The camera is opened when the first consumer starts, and closed 2 s after the last one stopped. Frames are passed on as the camera
 * made them (MJPEG stays MJPEG): the engine's own decodebin3 and videoscale/videorate after the source do the rest, for every
 * resolution or frame rate a page asks for.
 *
 * Environment: IYAKKI_CAMSHARE_FAKE=1 (or "jpeg") lists one test camera made of videotestsrc that, like a UVC node, refuses a second
 * open: the self check (camshare-check.c) uses it. Only GStreamer (LGPL, used dynamically) is linked.
 */
#ifndef PACKAGE
#define PACKAGE "iyakki"
#endif
#include <math.h>
#include <string.h>
#include <gst/gst.h>
#include <gst/app/gstappsink.h>
#include <gst/app/gstappsrc.h>
#include "devlist.h"

GST_DEBUG_CATEGORY_STATIC(camshare_debug);
#define GST_CAT_DEFAULT camshare_debug

/* The mode the hub asks of a camera: the smallest one that is at least this size at 24 frames/s or more (30 preferred). */
#define TARGET_W 1920
#define TARGET_H 1080
#define CLOSE_DELAY_SECS 2

/* ------------------------------------------------------------------ counters (read by the self check) */
static gint stat_opens, stat_busy, stat_frames, stat_open_now;
void iyakki_camshare_stats(guint *opens, guint *busy, guint *frames) {
    *opens = (guint)g_atomic_int_get(&stat_opens);
    *busy = (guint)g_atomic_int_get(&stat_busy);
    *frames = (guint)g_atomic_int_get(&stat_frames);
}

/* ------------------------------------------------------------------ mode choice */
typedef struct { const char *mime; char format[24]; gint w, h, fn, fd; } Mode;

static gint fixed_values(const GValue *v, gint target, gint *out, gint max) {
    gint n = 0;
    if (!v) return 0;
    if (G_VALUE_HOLDS_INT(v)) { out[n++] = g_value_get_int(v); }
    else if (GST_VALUE_HOLDS_LIST(v) || GST_VALUE_HOLDS_ARRAY(v)) {
        guint sz = GST_VALUE_HOLDS_LIST(v) ? gst_value_list_get_size(v) : gst_value_array_get_size(v);
        for (guint i = 0; i < sz && n < max; i++) {
            const GValue *e = GST_VALUE_HOLDS_LIST(v) ? gst_value_list_get_value(v, i) : gst_value_array_get_value(v, i);
            if (G_VALUE_HOLDS_INT(e)) out[n++] = g_value_get_int(e);
        }
    } else if (GST_VALUE_HOLDS_INT_RANGE(v)) {
        gint lo = gst_value_get_int_range_min(v), hi = gst_value_get_int_range_max(v), st = gst_value_get_int_range_step(v);
        gint t = target < lo ? lo : target > hi ? hi : target;
        if (st > 1) t = lo + ((t - lo) / st) * st;
        out[n++] = t;
    }
    return n;
}

static gint fixed_fractions(const GValue *v, gint (*out)[2], gint max) {
    gint n = 0;
    if (!v) return 0;
    if (GST_VALUE_HOLDS_FRACTION(v)) {
        out[n][0] = gst_value_get_fraction_numerator(v); out[n][1] = gst_value_get_fraction_denominator(v); n++;
    } else if (GST_VALUE_HOLDS_LIST(v) || GST_VALUE_HOLDS_ARRAY(v)) {
        guint sz = GST_VALUE_HOLDS_LIST(v) ? gst_value_list_get_size(v) : gst_value_array_get_size(v);
        for (guint i = 0; i < sz && n < max; i++) {
            const GValue *e = GST_VALUE_HOLDS_LIST(v) ? gst_value_list_get_value(v, i) : gst_value_array_get_value(v, i);
            if (GST_VALUE_HOLDS_FRACTION(e)) { out[n][0] = gst_value_get_fraction_numerator(e); out[n][1] = gst_value_get_fraction_denominator(e); n++; }
        }
    } else if (GST_VALUE_HOLDS_FRACTION_RANGE(v)) {
        const GValue *lo = gst_value_get_fraction_range_min(v), *hi = gst_value_get_fraction_range_max(v);
        gint c = 30, d = 1;
        if (gst_util_fraction_compare(c, d, gst_value_get_fraction_numerator(lo), gst_value_get_fraction_denominator(lo)) < 0) {
            c = gst_value_get_fraction_numerator(lo); d = gst_value_get_fraction_denominator(lo);
        } else if (gst_util_fraction_compare(c, d, gst_value_get_fraction_numerator(hi), gst_value_get_fraction_denominator(hi)) > 0) {
            c = gst_value_get_fraction_numerator(hi); d = gst_value_get_fraction_denominator(hi);
        }
        out[n][0] = c; out[n][1] = d; n++;
    }
    return n;
}

static double fps_of(const Mode *m) { return m->fd ? (double)m->fn / m->fd : 0; }

/* is a better than b? (both complete) */
static gboolean mode_better(const Mode *a, const Mode *b) {
    double fa = fps_of(a), fb = fps_of(b);
    gboolean oka = fa >= 23.9, okb = fb >= 23.9;
    if (oka != okb) return oka;
    if (!oka && fa != fb) return fa > fb;
    gint64 aa = (gint64)a->w * a->h, ab = (gint64)b->w * b->h, want = (gint64)TARGET_W * TARGET_H;
    gboolean ba = aa >= want, bb = ab >= want;
    if (ba != bb) return ba;
    if (aa != ab) return ba ? aa < ab : aa > ab;
    gboolean ja = strcmp(a->mime, "image/jpeg") == 0, jb = strcmp(b->mime, "image/jpeg") == 0;
    if (ja != jb) return ba ? ja : !ja;   /* big pictures over USB are MJPEG; small raw ones need no decoding */
    return fabs(fa - 30.0) < fabs(fb - 30.0);
}

/* The one fixed mode of a camera from everything its caps offer; NULL when it offers nothing usable (only raw and MJPEG are used). */
GstCaps *iyakki_camshare_choose(const GstCaps *caps) {
    Mode best = {0}; gboolean have = FALSE;
    if (!caps) return NULL;
    for (guint i = 0; i < gst_caps_get_size(caps); i++) {
        const GstStructure *s = gst_caps_get_structure(caps, i);
        const char *name = gst_structure_get_name(s);
        gboolean raw = strcmp(name, "video/x-raw") == 0;
        if (!raw && strcmp(name, "image/jpeg") != 0) continue;
        const char *fmt = NULL;
        if (raw) {
            const GValue *f = gst_structure_get_value(s, "format");
            if (f && G_VALUE_HOLDS_STRING(f)) fmt = g_value_get_string(f);
            else if (f && GST_VALUE_HOLDS_LIST(f) && gst_value_list_get_size(f) > 0 && G_VALUE_HOLDS_STRING(gst_value_list_get_value(f, 0)))
                fmt = g_value_get_string(gst_value_list_get_value(f, 0));
            if (!fmt) continue;
        }
        gint ws[16], hs[16], fr[16][2];
        gint nw = fixed_values(gst_structure_get_value(s, "width"), TARGET_W, ws, 16);
        gint nh = fixed_values(gst_structure_get_value(s, "height"), TARGET_H, hs, 16);
        gint nf = fixed_fractions(gst_structure_get_value(s, "framerate"), fr, 16);
        for (gint a = 0; a < nw; a++) for (gint b = 0; b < nh; b++) for (gint c = 0; c < nf; c++) {
            if (ws[a] <= 0 || hs[b] <= 0 || fr[c][0] <= 0 || fr[c][1] <= 0) continue;
            Mode m = { raw ? "video/x-raw" : "image/jpeg", "", ws[a], hs[b], fr[c][0], fr[c][1] };
            if (fmt) g_strlcpy(m.format, fmt, sizeof m.format);
            if (!have || mode_better(&m, &best)) { best = m; have = TRUE; }
        }
    }
    if (!have) return NULL;
    GstStructure *out = gst_structure_new(best.mime, "width", G_TYPE_INT, best.w, "height", G_TYPE_INT, best.h,
                                          "framerate", GST_TYPE_FRACTION, best.fn, best.fd,
                                          "pixel-aspect-ratio", GST_TYPE_FRACTION, 1, 1, NULL);
    if (best.format[0]) gst_structure_set(out, "format", G_TYPE_STRING, best.format, NULL);
    else gst_structure_set(out, "parsed", G_TYPE_BOOLEAN, TRUE, NULL);
    GstCaps *c = gst_caps_new_empty();
    gst_caps_append_structure(c, out);
    return c;
}

/* ------------------------------------------------------------------ hub: the only real source of one camera */
typedef struct { GWeakRef ref; gboolean active; } Entry;

typedef struct Hub {
    gchar *key;
    gboolean fake;
    GMutex lock;            /* consumers, close timer */
    GMutex ctl;             /* the real source's state changes (taken before lock) */
    GList *entries;
    guint close_id;
    GstDevice *inner;       /* the v4l2 device (strong ref) */
    GstCaps *caps;          /* the one mode */
    GstElement *pipe;       /* the only open of the camera, or NULL */
    gboolean failed;
} Hub;

static GHashTable *hubs;
static GMutex hubs_lock;

static Hub *hub_get(const char *key, gboolean fake, GstDevice *inner, const GstCaps *caps) {
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
    if (inner) { gst_object_replace((GstObject **)&h->inner, GST_OBJECT(inner)); }
    gst_caps_replace(&h->caps, (GstCaps *)caps);
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
        GError *err = g_error_new_literal(GST_RESOURCE_ERROR, GST_RESOURCE_ERROR_FAILED, text);
        gst_element_post_message(s, gst_message_new_error(GST_OBJECT(s), err, "iyakkicamshare"));
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
    GST_INFO("camera %s closed", h->key);
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
    /* strong references are taken under the lock and the frames are pushed outside it (a push may call back into a consumer) */
    GPtrArray *live = g_ptr_array_new();   /* Entry*, GstElement* pairs */
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
        GstBuffer *copy = gst_buffer_copy(buf);   /* shares the memory; the timestamps come from each consumer's own do-timestamp */
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
        GST_WARNING("camera %s stopped: %s", h->key, err ? err->message : "end of stream");
        if (!h->failed) {
            h->failed = TRUE;
            hub_post_error(h, err ? err->message : "the camera stopped");
            g_idle_add(hub_failed_cb, h);
        }
        g_clear_error(&err); g_free(dbg);
    }
    gst_message_unref(msg);
    return GST_BUS_DROP;
}

static GstElement *fake_source(const GstCaps *caps) {
    const char *mode = g_getenv("IYAKKI_CAMSHARE_FAKE");
    gboolean jpeg = mode && strcmp(mode, "jpeg") == 0;
    gchar *desc = g_strdup_printf("videotestsrc is-live=true pattern=ball ! video/x-raw,format=I420,width=%d,height=%d,framerate=30/1 ! %s",
                                  jpeg ? 1280 : 640, jpeg ? 720 : 480, jpeg ? "jpegenc" : "identity");
    GError *err = NULL;
    GstElement *b = gst_parse_bin_from_description(desc, TRUE, &err);
    g_free(desc); g_clear_error(&err);
    (void)caps;
    return b;
}

/* Open the camera if it is not open (the only place that opens it). Returns whether it is open. */
static gboolean hub_start(Hub *h) {
    gboolean ok = FALSE;
    g_mutex_lock(&h->ctl);
    if (h->pipe && h->failed) hub_stop_locked_ctl(h);
    if (h->pipe) { g_mutex_unlock(&h->ctl); return TRUE; }
    g_mutex_lock(&h->lock);
    GstDevice *inner = h->inner ? gst_object_ref(h->inner) : NULL;
    GstCaps *caps = h->caps ? gst_caps_ref(h->caps) : NULL;
    g_mutex_unlock(&h->lock);
    if (h->fake && g_atomic_int_add(&stat_open_now, 1) > 0) {   /* a UVC node refuses a second streamer: so does the fake one */
        g_atomic_int_add(&stat_open_now, -1);
        g_atomic_int_inc(&stat_busy);
        goto out;
    }
    GstElement *src = h->fake ? fake_source(caps) : inner ? gst_device_create_element(inner, NULL) : NULL;
    GstElement *filter = gst_element_factory_make("capsfilter", NULL);
    GstElement *sink = gst_element_factory_make("appsink", NULL);
    GstElement *pipe = gst_pipeline_new(NULL);
    if (!src || !filter || !sink || !pipe) {
        if (src) gst_object_unref(src);
        if (filter) gst_object_unref(filter);
        if (sink) gst_object_unref(sink);
        if (pipe) gst_object_unref(pipe);
        if (h->fake) g_atomic_int_add(&stat_open_now, -1);
        goto out;
    }
    g_object_set(filter, "caps", caps, NULL);
    g_object_set(sink, "sync", FALSE, "max-buffers", 2, "drop", TRUE, "emit-signals", FALSE, NULL);
    GstAppSinkCallbacks cb = { 0 }; cb.new_sample = hub_new_sample;
    gst_app_sink_set_callbacks(GST_APP_SINK(sink), &cb, h, NULL);
    gst_bin_add_many(GST_BIN(pipe), src, filter, sink, NULL);
    if (!gst_element_link_many(src, filter, sink, NULL)) {
        gst_object_unref(pipe);
        if (h->fake) g_atomic_int_add(&stat_open_now, -1);
        goto out;
    }
    GstBus *bus = gst_pipeline_get_bus(GST_PIPELINE(pipe));
    gst_bus_set_sync_handler(bus, hub_bus, h, NULL);
    gst_object_unref(bus);
    h->failed = FALSE;
    g_atomic_int_inc(&stat_opens);
    if (gst_element_set_state(pipe, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
        gst_element_set_state(pipe, GST_STATE_NULL);
        gst_object_unref(pipe);
        if (h->fake) g_atomic_int_add(&stat_open_now, -1);
        goto out;
    }
    h->pipe = pipe;
    GST_INFO("camera %s opened once for every page request", h->key);
    ok = TRUE;
out:
    g_mutex_unlock(&h->ctl);
    if (inner) gst_object_unref(inner);
    if (caps) gst_caps_unref(caps);
    if (!ok) hub_post_error(h, "the camera could not be opened");
    return ok;
}

/* A consumer's appsrc wants data: it is live (again). */
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
    g_object_set(src, "is-live", TRUE, "format", GST_FORMAT_TIME, "caps", h->caps, "max-buffers", (guint64)2, "max-bytes", (guint64)0,
                 "leaky-type", GST_APP_LEAKY_TYPE_DOWNSTREAM, "block", FALSE, "emit-signals", TRUE, "min-latency", (gint64)0, NULL);
    Entry *e = g_new0(Entry, 1);
    g_weak_ref_init(&e->ref, src);
    h->entries = g_list_append(h->entries, e);
    hub_live_count(h, NULL);
    g_mutex_unlock(&h->lock);
    g_signal_connect(src, "need-data", G_CALLBACK(consumer_need_data), h);
    return src;
}

/* ------------------------------------------------------------------ device */
typedef struct { GstDevice parent; Hub *hub; } IyakkiCamDevice;
typedef struct { GstDeviceClass parent_class; } IyakkiCamDeviceClass;
G_DEFINE_TYPE(IyakkiCamDevice, iyakki_cam_device, GST_TYPE_DEVICE)

static GstElement *cam_device_create_element(GstDevice *dev, const gchar *name) {
    return hub_attach(((IyakkiCamDevice *)dev)->hub, name);
}
static void iyakki_cam_device_class_init(IyakkiCamDeviceClass *k) { GST_DEVICE_CLASS(k)->create_element = cam_device_create_element; }
static void iyakki_cam_device_init(IyakkiCamDevice *d) { (void)d; }

static GstDevice *wrap_device(GstDevice *inner, const char *fake_name, GstCaps *fake_caps) {
    gchar *name = inner ? gst_device_get_display_name(inner) : g_strdup(fake_name);
    GstStructure *props = inner ? gst_device_get_properties(inner) : gst_structure_new_empty("props");
    GstCaps *caps = inner ? gst_device_get_caps(inner) : gst_caps_ref(fake_caps);
    const char *path = props ? gst_structure_get_string(props, "device.path") : NULL;
    if (inner) {   /* a camera the person marked "don't use" is not offered to any page */
        GHashTable *hidden = devlist_read(DEVLIST_CAM_HIDDEN);
        gboolean skip = devlist_has(hidden, name);
        g_hash_table_unref(hidden);
        if (skip) {
            GST_INFO("camera %s is hidden by the person", name);
            if (caps) gst_caps_unref(caps);
            if (props) gst_structure_free(props);
            g_free(name);
            return NULL;
        }
    }
    gchar *key = g_strdup_printf("%s%s", inner ? "v4l2:" : "fake:", path ? path : name);
    GstCaps *mode = NULL;
    GstDevice *dev = NULL;
    if (caps && !gst_caps_is_empty(caps)) mode = iyakki_camshare_choose(caps);
    if (!mode) {   /* the camera was busy while it was inspected: the mode seen the last time is still right */
        g_mutex_lock(&hubs_lock);
        Hub *old = hubs ? g_hash_table_lookup(hubs, key) : NULL;
        if (old && old->caps) mode = gst_caps_ref(old->caps);
        g_mutex_unlock(&hubs_lock);
    }
    if (mode) {
        Hub *h = hub_get(key, inner == NULL, inner, mode);
        IyakkiCamDevice *d = g_object_new(iyakki_cam_device_get_type(), "display-name", name, "caps", mode, "device-class", "Video/Source",
                                          "properties", props, NULL);
        d->hub = h;
        dev = GST_DEVICE(d);
        gst_caps_unref(mode);
    } else {
        GST_WARNING("camera %s has no usable mode", name);
    }
    if (caps) gst_caps_unref(caps);
    if (props) gst_structure_free(props);
    g_free(name); g_free(key);
    return dev;
}

/* ------------------------------------------------------------------ provider */
typedef struct { GstDeviceProvider parent; GstDeviceProvider *inner; GHashTable *map; GMutex lock; guint timer; gchar *sig; } IyakkiCamProvider;
typedef struct { GstDeviceProviderClass parent_class; } IyakkiCamProviderClass;
G_DEFINE_TYPE(IyakkiCamProvider, iyakki_cam_provider, GST_TYPE_DEVICE_PROVIDER)

static GstCaps *fake_caps(void) {
    const char *m = g_getenv("IYAKKI_CAMSHARE_FAKE");
    if (m && strcmp(m, "jpeg") == 0) return gst_caps_from_string("image/jpeg, parsed=true, width=1280, height=720, framerate=30/1");
    return gst_caps_from_string("video/x-raw, format=I420, width=640, height=480, framerate=30/1");
}

static GstDeviceProvider *inner_provider(void) { return gst_device_provider_factory_get_by_name("v4l2deviceprovider"); }

/* The camera the person chose goes first: the engine takes the first camera of the list as the default one. */
static GList *cam_put_chosen_first(GList *out) {
    gchar *choice = devlist_value(DEVLIST_CAM_CHOICE);
    if (choice) {
        for (GList *l = out; l; l = l->next) {
            gchar *n = gst_device_get_display_name(l->data);
            gboolean hit = n && !g_strcmp0(g_strstrip(n), choice);
            g_free(n);
            if (hit) {
                out = g_list_remove_link(out, l);
                out = g_list_concat(l, out);
                break;
            }
        }
        g_free(choice);
    }
    return out;
}

/* What the person decided, as one string: the camera chosen and the hidden ones. A change is applied when no camera is open. */
static gchar *cam_signature(void) {
    GHashTable *h = devlist_read(DEVLIST_CAM_HIDDEN);
    gchar *c = devlist_value(DEVLIST_CAM_CHOICE);
    GList *keys = g_list_sort(g_hash_table_get_keys(h), (GCompareFunc)g_strcmp0);
    GString *s = g_string_new(c ? c : "");
    for (GList *k = keys; k; k = k->next) g_string_append_printf(s, "\n%s", (char *)k->data);
    g_list_free(keys);
    g_hash_table_unref(h);
    g_free(c);
    return g_string_free(s, FALSE);
}

static gboolean cams_in_use(void) {
    gboolean busy = FALSE;
    g_mutex_lock(&hubs_lock);
    if (hubs) {
        GHashTableIter it;
        gpointer k, v;
        g_hash_table_iter_init(&it, hubs);
        while (g_hash_table_iter_next(&it, &k, &v)) if (((Hub *)v)->pipe) busy = TRUE;
    }
    g_mutex_unlock(&hubs_lock);
    return busy;
}

static GList *cam_provider_probe(GstDeviceProvider *p) {
    GList *out = NULL;
    if (g_getenv("IYAKKI_CAMSHARE_FAKE")) {
        GstCaps *c = fake_caps();
        GstDevice *d = wrap_device(NULL, "IYAKKI test camera", c);
        gst_caps_unref(c);
        return d ? g_list_append(NULL, d) : NULL;
    }
    GstDeviceProvider *in = inner_provider();
    if (!in) return NULL;
    GList *devs = gst_device_provider_get_devices(in);
    for (GList *l = devs; l; l = l->next) {
        if (!gst_device_has_classes(l->data, "Video/Source")) continue;
        GstDevice *w = wrap_device(l->data, NULL, NULL);
        if (w) out = g_list_append(out, w);
    }
    g_list_free_full(devs, gst_object_unref);
    gst_object_unref(in);
    return cam_put_chosen_first(out);
}

static GstBusSyncReply inner_bus(GstBus *bus, GstMessage *msg, gpointer data) {
    IyakkiCamProvider *self = data;
    GstDevice *dev = NULL, *old = NULL;
    switch (GST_MESSAGE_TYPE(msg)) {
    case GST_MESSAGE_DEVICE_ADDED:
        gst_message_parse_device_added(msg, &dev);
        if (gst_device_has_classes(dev, "Video/Source")) {
            GstDevice *w = wrap_device(dev, NULL, NULL);
            if (w) {
                g_mutex_lock(&self->lock);
                g_hash_table_replace(self->map, dev, gst_object_ref(w));
                g_mutex_unlock(&self->lock);
                gst_device_provider_device_add(GST_DEVICE_PROVIDER(self), w);
            }
        }
        break;
    case GST_MESSAGE_DEVICE_REMOVED: {
        gst_message_parse_device_removed(msg, &dev);
        g_mutex_lock(&self->lock);
        GstDevice *w = g_hash_table_lookup(self->map, dev);
        if (w) { gst_object_ref(w); g_hash_table_remove(self->map, dev); }
        g_mutex_unlock(&self->lock);
        if (w) { gst_device_provider_device_remove(GST_DEVICE_PROVIDER(self), w); gst_object_unref(w); }
        break;
    }
    case GST_MESSAGE_DEVICE_CHANGED:
        gst_message_parse_device_changed(msg, &dev, &old);   /* the mode of a camera does not change while it is plugged in */
        break;
    default: break;
    }
    if (dev) gst_object_unref(dev);
    if (old) gst_object_unref(old);
    gst_message_unref(msg);
    return GST_BUS_DROP;
}

/* Every 2 s: did the person hide or choose a camera? Rebuild the list (the chosen one first, hidden ones gone) when no camera is open. */
static gboolean cam_refresh(gpointer data) {
    IyakkiCamProvider *self = data;
    if (!self->inner) return TRUE;
    gchar *sig = cam_signature();
    if (self->sig && !strcmp(self->sig, sig)) { g_free(sig); return TRUE; }
    if (cams_in_use()) { g_free(sig); return TRUE; }   /* applied as soon as the camera is free */
    GST_INFO("the camera choices changed: rebuilding the list");
    g_free(self->sig);
    self->sig = sig;
    g_mutex_lock(&self->lock);
    GList *old = g_hash_table_get_values(self->map);
    for (GList *l = old; l; l = l->next) gst_object_ref(l->data);
    g_hash_table_remove_all(self->map);
    g_mutex_unlock(&self->lock);
    for (GList *l = old; l; l = l->next) gst_device_provider_device_remove(GST_DEVICE_PROVIDER(self), l->data);
    g_list_free_full(old, gst_object_unref);
    GList *devs = gst_device_provider_get_devices(self->inner), *fresh = NULL;
    for (GList *l = devs; l; l = l->next) {
        if (!gst_device_has_classes(l->data, "Video/Source")) continue;
        GstDevice *w = wrap_device(l->data, NULL, NULL);
        if (!w) continue;
        g_mutex_lock(&self->lock);
        g_hash_table_replace(self->map, l->data, gst_object_ref(w));
        g_mutex_unlock(&self->lock);
        fresh = g_list_append(fresh, w);
    }
    fresh = cam_put_chosen_first(fresh);
    for (GList *l = fresh; l; l = l->next) gst_device_provider_device_add(GST_DEVICE_PROVIDER(self), l->data);
    g_list_free(fresh);
    g_list_free_full(devs, gst_object_unref);
    return TRUE;
}

static gboolean cam_provider_start(GstDeviceProvider *p) {
    IyakkiCamProvider *self = (IyakkiCamProvider *)p;
    if (g_getenv("IYAKKI_CAMSHARE_FAKE")) {
        GList *l = cam_provider_probe(p);
        for (GList *i = l; i; i = i->next) gst_device_provider_device_add(p, i->data);
        g_list_free(l);
        return TRUE;
    }
    self->inner = inner_provider();
    if (!self->inner) { GST_WARNING("no v4l2 device provider: no cameras"); return TRUE; }
    GstBus *bus = gst_device_provider_get_bus(self->inner);
    gst_bus_set_sync_handler(bus, inner_bus, self, NULL);
    gst_object_unref(bus);
    if (!gst_device_provider_start(self->inner)) { gst_object_unref(self->inner); self->inner = NULL; GST_WARNING("the v4l2 provider did not start"); }
    else {
        g_free(self->sig);
        self->sig = cam_signature();   /* the list that was just built used these files */
        if (!self->timer) self->timer = g_timeout_add_seconds(2, cam_refresh, self);
    }
    return TRUE;
}

static void cam_provider_stop(GstDeviceProvider *p) {
    IyakkiCamProvider *self = (IyakkiCamProvider *)p;
    if (self->timer) { g_source_remove(self->timer); self->timer = 0; }
    g_free(self->sig); self->sig = NULL;
    if (self->inner) {
        gst_device_provider_stop(self->inner);
        GstBus *bus = gst_device_provider_get_bus(self->inner);
        gst_bus_set_sync_handler(bus, NULL, NULL, NULL);
        gst_object_unref(bus);
        gst_object_unref(self->inner); self->inner = NULL;
    }
    g_mutex_lock(&self->lock);
    g_hash_table_remove_all(self->map);
    g_mutex_unlock(&self->lock);
}

static void iyakki_cam_provider_class_init(IyakkiCamProviderClass *k) {
    GstDeviceProviderClass *pc = GST_DEVICE_PROVIDER_CLASS(k);
    pc->probe = cam_provider_probe; pc->start = cam_provider_start; pc->stop = cam_provider_stop;
    gst_device_provider_class_set_static_metadata(pc, "IYAKKI shared camera", "Video/Source",
        "Cameras that every page request of the web engine shares through one open", "IYAKKI");
}
static void iyakki_cam_provider_init(IyakkiCamProvider *p) {
    p->map = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, gst_object_unref);
    g_mutex_init(&p->lock);
}

/* ------------------------------------------------------------------ plugin */
static gboolean camshare_plugin_init(GstPlugin *plugin) {
    GST_DEBUG_CATEGORY_INIT(camshare_debug, "iyakkicamshare", 0, "IYAKKI shared camera");
    return gst_device_provider_register(plugin, "iyakkicamshare", GST_RANK_PRIMARY + 1, iyakki_cam_provider_get_type());
}

#ifndef CAMSHARE_NO_PLUGIN_DEFINE
GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, iyakkicamshare, "IYAKKI shared camera: one open of a camera for every page request",
                  camshare_plugin_init, "1", "unknown", "IYAKKI", "iyakki")
#endif
