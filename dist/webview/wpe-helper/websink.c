/*
 * IYAKKI sound output for the web engine: a GStreamer sink that cannot fail (SPDX-License-Identifier: Apache-2.0).
 *
 * Why: on a T2 MacBook the engine's own output (`autoaudiosink` -> `alsasink`) cannot be set up on the sound device: the channel step
 * of its hardware parameters is refused (`Error -22 ... snd_pcm_hw_params_set_channels`) through every ALSA chain tried (plain
 * default, softvol over hw, plug over hw). A page that uses WebAudio (a recorder's level meter, Google's voice search) then freezes:
 * WebKit starts that output synchronously on the page's own thread and the failed pipeline never reports back. `aplay` plays on the
 * same device, so this element asks ALSA the way `aplay` does (`snd_pcm_set_params`: one call, soft resampling on, plain 16-bit
 * stereo at 48 kHz) and, whatever happens, never reports an error: when the device cannot be opened or set up (busy, refused), the
 * samples are dropped at real-time speed, so the page goes on without sound instead of freezing. The reason is written to stderr
 * (the system log shows it: `iyakki-websink: ...`).
 *
 * Several outputs of one engine share ONE device handle and one mixer thread (the T2 speaker is single-use and the engine starts a new
 * output while the previous one still holds the device).
 *
 * `iyakkiwebaudiosink` is a bin: audioconvert ! audioresample ! capsfilter(S16LE, 48000, 2) ! iyakkiwebpcmsink. Rank none: the web
 * helper raises it (GST_PLUGIN_FEATURE_RANK) on every machine, `IYAKKI_WEBVIEW_AUDIOSINK=0` keeps the engine's usual output. Limits:
 * always 16-bit stereo at 48 kHz (surround is mixed down, no AC3/DTS pass-through). The device is `default` (the engine's ALSA configuration decides what
 * that is), or `IYAKKI_WEBSINK_DEVICE` from the engine's environment (never a file: an ALSA name can write files).
 */
#ifndef PACKAGE
#define PACKAGE "iyakki"
#endif
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#include <gst/gst.h>
#include <gst/audio/audio.h>
#include <gst/audio/gstaudiosink.h>
#include <alsa/asoundlib.h>

GST_DEBUG_CATEGORY_STATIC(websink_debug);
#define GST_CAT_DEFAULT websink_debug

#define RATE 48000
#define CHANNELS 2
#define FRAME_BYTES (CHANNELS * 2)

/* ---- one shared output per process, the sinks are voices of it ------------------------------------------------------------------- */

/* The T2's speaker (and many other devices without dmix) can be opened by ONE program at a time, and the engine starts a new output
 * pipeline whenever a page resumes its audio or starts a media element, while the previous one still holds the device. So the device
 * is opened by ONE mixer thread of this process; every sink is a voice with its own ring buffer; the thread adds the voices and writes
 * the sum in 20 ms pieces, and lets the device go after 1.5 s without a voice. Other processes (another engine, the IYAKKI players)
 * still exclude each other: the thread keeps trying to open the device while voices wait, and drops their samples at real-time speed. */

#define VOICE_FRAMES 9600       /* 200 ms per voice */
#define MIX_FRAMES (RATE / 50)  /* 20 ms per write */
#define IDLE_CLOSE_US 1500000
#define BUSY_TRIES 10           /* x 100 ms while the device is busy */

typedef struct {
    gint16 buf[VOICE_FRAMES * CHANNELS];
    guint rd, fill;       /* first frame, frames waiting */
    guint gen;            /* grows at every reset: a write that sees it change drops its data */
    gboolean registered;
} Voice;

static GMutex mu;
static GCond cv_space, cv_data;
static GSList *voices = NULL;
static GThread *mixer = NULL;
static snd_pcm_t *dev = NULL;      /* touched only by the mixer thread */
static gint64 idle_since_us = 0;
static gint64 next_open_us = 0;
static gboolean told_failure = FALSE;

static const char *device_name(void) {
    const char *d = getenv("IYAKKI_WEBSINK_DEVICE");
    return d && *d ? d : "default";
}

static void say(const char *what, int err) {
    fprintf(stderr, "iyakki-websink: %s on %s: %s (the page goes on without sound)\n", what, device_name(), err < 0 ? snd_strerror(err) : "no further detail");
    fflush(stderr);
}

/* Open and set the device up the way aplay does. Called by the mixer thread WITHOUT the lock. */
static void device_open(void) {
    int err = 0;
    snd_pcm_t *p = NULL;
    for (int tries = 0;; tries++) {
        err = snd_pcm_open(&p, device_name(), SND_PCM_STREAM_PLAYBACK, 0);
        if (err != -EBUSY || tries >= BUSY_TRIES) {
            break;
        }
        g_usleep(100000);
    }
    if (err >= 0) {
        err = snd_pcm_set_params(p, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, CHANNELS, RATE, 1, 200000);
        if (err < 0) {
            snd_pcm_close(p);
            p = NULL;
        }
    } else {
        p = NULL;
    }
    if (p) {
        dev = p;
        told_failure = FALSE;
        fprintf(stderr, "iyakki-websink: playing through %s (S16LE, %d Hz, %d channels)\n", device_name(), RATE, CHANNELS);
        fflush(stderr);
    } else {
        if (!told_failure) {
            say(err == -EBUSY ? "snd_pcm_open failed" : "the device could not be set up", err);
            told_failure = TRUE;
        }
        next_open_us = g_get_monotonic_time() + 1000000; /* try again in a second while voices wait */
    }
}

static void device_close(const char *why) {
    if (dev) {
        snd_pcm_drop(dev);
        snd_pcm_close(dev);
        dev = NULL;
        if (why) {
            fprintf(stderr, "iyakki-websink: %s\n", why);
            fflush(stderr);
        }
    }
}

static gpointer mixer_main(gpointer unused) {
    gint32 *acc = g_new0(gint32, MIX_FRAMES * CHANNELS);
    gint16 *out = g_new0(gint16, MIX_FRAMES * CHANNELS);
    (void)unused;
    g_mutex_lock(&mu);
    for (;;) {
        gint64 now = g_get_monotonic_time();
        if (!voices) {
            if (dev && now - idle_since_us > IDLE_CLOSE_US) {
                g_mutex_unlock(&mu);
                device_close(NULL);
                g_mutex_lock(&mu);
            }
            g_cond_wait_until(&cv_data, &mu, g_get_monotonic_time() + 200000);
            continue;
        }
        idle_since_us = now;
        if (!dev && now >= next_open_us) {
            g_mutex_unlock(&mu);
            device_open();
            g_mutex_lock(&mu);
        }
        memset(acc, 0, sizeof(gint32) * MIX_FRAMES * CHANNELS);
        for (GSList *l = voices; l; l = l->next) {
            Voice *v = l->data;
            guint take = v->fill < MIX_FRAMES ? v->fill : MIX_FRAMES;
            for (guint f = 0; f < take; f++) {
                guint at = ((v->rd + f) % VOICE_FRAMES) * CHANNELS;
                acc[f * CHANNELS] += v->buf[at];
                acc[f * CHANNELS + 1] += v->buf[at + 1];
            }
            v->rd = (v->rd + take) % VOICE_FRAMES;
            v->fill -= take;
        }
        g_cond_broadcast(&cv_space);
        g_mutex_unlock(&mu);
        for (guint i = 0; i < MIX_FRAMES * CHANNELS; i++) {
            gint32 x = acc[i];
            out[i] = (gint16)(x > 32767 ? 32767 : x < -32768 ? -32768 : x);
        }
        if (dev) {
            guint done = 0;
            while (done < MIX_FRAMES) {
                snd_pcm_sframes_t n = snd_pcm_writei(dev, out + (gsize)done * CHANNELS, MIX_FRAMES - done);
                if (n == -EPIPE || n == -ESTRPIPE || n == -EINTR) {
                    n = snd_pcm_recover(dev, (int)n, 1);
                }
                if (n < 0) {
                    /* a hard error (the T2 speaker answers "I/O error" now and then): close it, the next round opens it again */
                    char msg[160];
                    snprintf(msg, sizeof msg, "snd_pcm_writei failed on %s: %s; opening it again", device_name(), snd_strerror((int)n));
                    device_close(msg);
                    next_open_us = g_get_monotonic_time() + 100000;
                    break;
                }
                done += (guint)n;
            }
        } else {
            g_usleep(20000); /* no device: the voices still run at real-time speed */
        }
        g_mutex_lock(&mu);
    }
    return NULL;
}

/* ---- the sink: one voice ---------------------------------------------------------------------------------------------------------- */

#define IYAKKI_TYPE_PCM_SINK (iyakki_pcm_sink_get_type())
#define IYAKKI_PCM_SINK(obj) (G_TYPE_CHECK_INSTANCE_CAST((obj), IYAKKI_TYPE_PCM_SINK, IyakkiPcmSink))

typedef struct {
    GstAudioSink parent;
    Voice *voice;
} IyakkiPcmSink;

typedef struct {
    GstAudioSinkClass parent_class;
} IyakkiPcmSinkClass;

G_DEFINE_TYPE(IyakkiPcmSink, iyakki_pcm_sink, GST_TYPE_AUDIO_SINK)

static GstStaticPadTemplate pcm_sink_template = GST_STATIC_PAD_TEMPLATE("sink", GST_PAD_SINK, GST_PAD_ALWAYS,
    GST_STATIC_CAPS("audio/x-raw, format = (string) " GST_AUDIO_NE(S16) ", layout = (string) interleaved, rate = (int) 48000, channels = (int) 2"));

static gboolean pcm_open(GstAudioSink *sink) {
    (void)sink;
    return TRUE; /* never reports a failure */
}

static gboolean pcm_close(GstAudioSink *sink) {
    (void)sink;
    return TRUE;
}

static gboolean pcm_prepare(GstAudioSink *sink, GstAudioRingBufferSpec *spec) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(sink);
    Voice *v = s->voice;
    g_mutex_lock(&mu);
    v->rd = v->fill = 0;
    if (!v->registered) {
        v->registered = TRUE;
        voices = g_slist_prepend(voices, v);
    }
    if (!mixer) {
        mixer = g_thread_new("iyakki-websink", mixer_main, NULL);
    }
    g_cond_broadcast(&cv_data);
    g_mutex_unlock(&mu);
    /* 20 ms segments, 10 of them */
    spec->segsize = MIX_FRAMES * FRAME_BYTES;
    spec->segtotal = 10;
    return TRUE;
}

static gboolean pcm_unprepare(GstAudioSink *sink) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(sink);
    Voice *v = s->voice;
    g_mutex_lock(&mu);
    if (v->registered) {
        v->registered = FALSE;
        voices = g_slist_remove(voices, v);
    }
    v->fill = 0;
    v->gen++;
    g_cond_broadcast(&cv_space);
    g_mutex_unlock(&mu);
    return TRUE;
}

static gint pcm_write(GstAudioSink *sink, gpointer data, guint length) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(sink);
    Voice *v = s->voice;
    guint frames = length / FRAME_BYTES;
    const gint16 *in = data;
    guint done = 0;
    g_mutex_lock(&mu);
    guint gen = v->gen;
    while (done < frames && v->registered && v->gen == gen) {
        guint space = VOICE_FRAMES - v->fill;
        if (space == 0) {
            g_cond_wait_until(&cv_space, &mu, g_get_monotonic_time() + 100000);
            continue;
        }
        guint n = frames - done < space ? frames - done : space;
        for (guint f = 0; f < n; f++) {
            guint at = ((v->rd + v->fill + f) % VOICE_FRAMES) * CHANNELS;
            v->buf[at] = in[(done + f) * CHANNELS];
            v->buf[at + 1] = in[(done + f) * CHANNELS + 1];
        }
        v->fill += n;
        done += n;
        g_cond_broadcast(&cv_data);
    }
    g_mutex_unlock(&mu);
    return length;
}

static guint pcm_delay(GstAudioSink *sink) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(sink);
    g_mutex_lock(&mu);
    guint d = s->voice->fill;
    g_mutex_unlock(&mu);
    return d;
}

static void pcm_reset(GstAudioSink *sink) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(sink);
    g_mutex_lock(&mu);
    s->voice->fill = 0;
    s->voice->gen++;
    g_cond_broadcast(&cv_space);
    g_mutex_unlock(&mu);
}

static void pcm_finalize(GObject *obj) {
    IyakkiPcmSink *s = IYAKKI_PCM_SINK(obj);
    g_mutex_lock(&mu);
    if (s->voice && s->voice->registered) {
        s->voice->registered = FALSE;
        voices = g_slist_remove(voices, s->voice);
    }
    g_mutex_unlock(&mu);
    g_free(s->voice);
    s->voice = NULL;
    G_OBJECT_CLASS(iyakki_pcm_sink_parent_class)->finalize(obj);
}

static void iyakki_pcm_sink_class_init(IyakkiPcmSinkClass *klass) {
    GstElementClass *ec = GST_ELEMENT_CLASS(klass);
    GstAudioSinkClass *sc = GST_AUDIO_SINK_CLASS(klass);
    G_OBJECT_CLASS(klass)->finalize = pcm_finalize;
    gst_element_class_set_static_metadata(ec, "IYAKKI PCM sink", "Sink/Audio", "One voice of the process's shared output: mixed with the others, written to ALSA, or dropped when the device cannot be used", "IYAKKI");
    gst_element_class_add_static_pad_template(ec, &pcm_sink_template);
    sc->open = pcm_open;
    sc->close = pcm_close;
    sc->prepare = pcm_prepare;
    sc->unprepare = pcm_unprepare;
    sc->write = pcm_write;
    sc->delay = pcm_delay;
    sc->reset = pcm_reset;
    GST_DEBUG_CATEGORY_INIT(websink_debug, "iyakkiwebsink", 0, "IYAKKI web engine sound output");
}

static void iyakki_pcm_sink_init(IyakkiPcmSink *s) {
    s->voice = g_new0(Voice, 1);
}

/* ---- the bin the engine's autoaudiosink picks ------------------------------------------------------------------------------------ */

#define IYAKKI_TYPE_WEB_SINK (iyakki_web_sink_get_type())

typedef struct {
    GstBin parent;
} IyakkiWebSink;

typedef struct {
    GstBinClass parent_class;
} IyakkiWebSinkClass;

G_DEFINE_TYPE(IyakkiWebSink, iyakki_web_sink, GST_TYPE_BIN)

static GstStaticPadTemplate bin_sink_template = GST_STATIC_PAD_TEMPLATE("sink", GST_PAD_SINK, GST_PAD_ALWAYS, GST_STATIC_CAPS("audio/x-raw"));

static void iyakki_web_sink_class_init(IyakkiWebSinkClass *klass) {
    GstElementClass *ec = GST_ELEMENT_CLASS(klass);
    gst_element_class_set_static_metadata(ec, "IYAKKI web audio sink", "Sink/Audio", "The web engine's sound output: converts, and never fails", "IYAKKI");
    gst_element_class_add_static_pad_template(ec, &bin_sink_template);
}

static void iyakki_web_sink_init(IyakkiWebSink *self) {
    GstElement *bin = GST_ELEMENT(self);
    GstElement *conv = gst_element_factory_make("audioconvert", NULL);
    GstElement *res = gst_element_factory_make("audioresample", NULL);
    GstElement *cf = gst_element_factory_make("capsfilter", NULL);
    GstElement *snk = g_object_new(IYAKKI_TYPE_PCM_SINK, NULL);
    GstCaps *caps;
    GstPad *pad;
    if (!conv || !res || !cf || !snk) {
        GST_ERROR_OBJECT(self, "audioconvert, audioresample or capsfilter is missing");
        return;
    }
    caps = gst_caps_new_simple("audio/x-raw", "format", G_TYPE_STRING, GST_AUDIO_NE(S16), "layout", G_TYPE_STRING, "interleaved", "rate", G_TYPE_INT, RATE, "channels", G_TYPE_INT, CHANNELS, NULL);
    g_object_set(cf, "caps", caps, NULL);
    gst_caps_unref(caps);
    gst_bin_add_many(GST_BIN(bin), conv, res, cf, snk, NULL);
    gst_element_link_many(conv, res, cf, snk, NULL);
    pad = gst_element_get_static_pad(conv, "sink");
    gst_element_add_pad(bin, gst_ghost_pad_new_from_template("sink", pad, gst_static_pad_template_get(&bin_sink_template)));
    gst_object_unref(pad);
    GST_OBJECT_FLAG_SET(bin, GST_ELEMENT_FLAG_SINK);
}

static gboolean plugin_init(GstPlugin *plugin) {
    /* rank none: only chosen when the helper raises it */
    return gst_element_register(plugin, "iyakkiwebpcmsink", GST_RANK_NONE, IYAKKI_TYPE_PCM_SINK) && gst_element_register(plugin, "iyakkiwebaudiosink", GST_RANK_NONE, IYAKKI_TYPE_WEB_SINK);
}

GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, iyakkiwebsink, "IYAKKI web engine sound output (never fails)", plugin_init, "0.1", "unknown", "IYAKKI", "iyakki")
