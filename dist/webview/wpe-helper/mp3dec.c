/*
 * IYAKKI MP3 decoder for the web engine: a GStreamer audio decoder element (SPDX-License-Identifier: Apache-2.0).
 *
 * Why: the image carries no MP3 decoder (gst-libav is parked and inactive, gst-plugins-ugly and mpg123 are not allowed by the release
 * licence gate), so a page that plays or converts MP3 stalls (a recorder site's "Processing...", a sound cue that never plays). MP3
 * patents expired in 2017. IYAKKI's own radio and MEDIA player already decode MP3 with `nanomp3` (MIT OR Apache-2.0), a Rust port of
 * minimp3 (lieff/minimp3, CC0 / public domain). A C plugin cannot link the Rust crate without new build machinery, so this element
 * uses minimp3 itself (third_party/minimp3.h, one file, CC0): the same algorithm, bit-for-bit what `nanomp3` ports. The test
 * crates/iyakki-agent/tests/mp3dec_match.rs checks that both give the same samples for the same file.
 *
 * The element, `iyakkimp3dec`, sits behind mpegaudioparse (gst-plugins-good "audioparsers", already in the image) so every input
 * buffer is one frame: sink audio/mpeg mpegversion=1 layer=3 parsed=true, src audio/x-raw S16LE interleaved, 1 or 2 channels. It has
 * no properties and no side effects. Rank primary+1: nothing else decodes MP3 here. Only GStreamer (LGPL, used dynamically) is linked.
 */
#ifndef PACKAGE
#define PACKAGE "iyakki"
#endif
#include <string.h>
#include <gst/gst.h>
#include <gst/audio/gstaudiodecoder.h>

#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_MP3
#include "third_party/minimp3.h"

GST_DEBUG_CATEGORY_STATIC(mp3dec_debug);
#define GST_CAT_DEFAULT mp3dec_debug

#define IYAKKI_TYPE_MP3_DEC (iyakki_mp3_dec_get_type())
#define IYAKKI_MP3_DEC(obj) (G_TYPE_CHECK_INSTANCE_CAST((obj), IYAKKI_TYPE_MP3_DEC, IyakkiMp3Dec))

typedef struct {
    GstAudioDecoder parent;
    mp3dec_t dec;
    gint rate, channels; /* the output format last announced; 0 before the first frame */
    guint frames, bad;
} IyakkiMp3Dec;

typedef struct {
    GstAudioDecoderClass parent_class;
} IyakkiMp3DecClass;

G_DEFINE_TYPE(IyakkiMp3Dec, iyakki_mp3_dec, GST_TYPE_AUDIO_DECODER)

static GstStaticPadTemplate sink_template = GST_STATIC_PAD_TEMPLATE("sink", GST_PAD_SINK, GST_PAD_ALWAYS,
    GST_STATIC_CAPS("audio/mpeg, mpegversion = (int) 1, layer = (int) 3, rate = (int) [ 8000, 48000 ], channels = (int) [ 1, 2 ], parsed = (boolean) true"));

static GstStaticPadTemplate src_template = GST_STATIC_PAD_TEMPLATE("src", GST_PAD_SRC, GST_PAD_ALWAYS,
    GST_STATIC_CAPS("audio/x-raw, format = (string) " GST_AUDIO_NE(S16) ", layout = (string) interleaved, rate = (int) [ 8000, 48000 ], channels = (int) [ 1, 2 ]"));

static gboolean mp3_start(GstAudioDecoder *d) {
    IyakkiMp3Dec *s = IYAKKI_MP3_DEC(d);
    mp3dec_init(&s->dec);
    s->rate = s->channels = 0;
    s->frames = s->bad = 0;
    return TRUE;
}

static gboolean mp3_stop(GstAudioDecoder *d) {
    IyakkiMp3Dec *s = IYAKKI_MP3_DEC(d);
    GST_INFO_OBJECT(s, "decoded %u frame(s), %u undecodable", s->frames, s->bad);
    return TRUE;
}

static gboolean mp3_set_format(GstAudioDecoder *d, GstCaps *caps) {
    (void)d;
    (void)caps; /* the real rate and channels come from the frame header; the first decoded frame sets the output format */
    return TRUE;
}

static void mp3_flush(GstAudioDecoder *d, gboolean hard) {
    IyakkiMp3Dec *s = IYAKKI_MP3_DEC(d);
    (void)hard;
    mp3dec_init(&s->dec); /* the bit reservoir of the old position means nothing after a seek */
}

static GstFlowReturn mp3_handle_frame(GstAudioDecoder *d, GstBuffer *buf) {
    IyakkiMp3Dec *s = IYAKKI_MP3_DEC(d);
    if (buf == NULL) {
        return GST_FLOW_OK; /* drain: nothing is held back */
    }
    GstMapInfo in;
    if (!gst_buffer_map(buf, &in, GST_MAP_READ)) {
        return gst_audio_decoder_finish_frame(d, NULL, 1);
    }
    mp3d_sample_t pcm[MINIMP3_MAX_SAMPLES_PER_FRAME];
    mp3dec_frame_info_t info;
    int samples = in.size > 0 && in.size <= G_MAXINT ? mp3dec_decode_frame(&s->dec, in.data, (int)in.size, pcm, &info) : 0;
    gst_buffer_unmap(buf, &in);
    if (samples <= 0 || info.channels < 1 || info.channels > 2 || info.hz < 8000) {
        /* the first frames after a seek can carry no sound (the bit reservoir is empty), a damaged one is skipped: both are dropped */
        s->bad += samples <= 0 && info.frame_bytes == 0;
        return gst_audio_decoder_finish_frame(d, NULL, 1);
    }
    if (s->rate != info.hz || s->channels != info.channels) {
        GstAudioInfo ai;
        gst_audio_info_set_format(&ai, GST_AUDIO_FORMAT_S16, info.hz, info.channels, NULL);
        if (!gst_audio_decoder_set_output_format(d, &ai)) {
            return GST_FLOW_NOT_NEGOTIATED;
        }
        s->rate = info.hz;
        s->channels = info.channels;
    }
    gsize bytes = (gsize)samples * (gsize)info.channels * sizeof(mp3d_sample_t);
    GstBuffer *out = gst_audio_decoder_allocate_output_buffer(d, bytes);
    if (out == NULL) {
        return GST_FLOW_ERROR;
    }
    gst_buffer_fill(out, 0, pcm, bytes);
    s->frames++;
    return gst_audio_decoder_finish_frame(d, out, 1);
}

static void iyakki_mp3_dec_class_init(IyakkiMp3DecClass *klass) {
    GstElementClass *ec = GST_ELEMENT_CLASS(klass);
    GstAudioDecoderClass *dc = GST_AUDIO_DECODER_CLASS(klass);
    gst_element_class_add_static_pad_template(ec, &sink_template);
    gst_element_class_add_static_pad_template(ec, &src_template);
    gst_element_class_set_static_metadata(ec, "IYAKKI MP3 decoder", "Codec/Decoder/Audio", "Decodes MP3 (MPEG-1/2 audio layer III) with minimp3 (CC0)", "IYAKKI");
    dc->start = mp3_start;
    dc->stop = mp3_stop;
    dc->set_format = mp3_set_format;
    dc->handle_frame = mp3_handle_frame;
    dc->flush = mp3_flush;
    GST_DEBUG_CATEGORY_INIT(mp3dec_debug, "iyakkimp3dec", 0, "IYAKKI MP3 decoder");
}

static void iyakki_mp3_dec_init(IyakkiMp3Dec *s) {
    mp3dec_init(&s->dec);
}

static gboolean mp3_plugin_init(GstPlugin *plugin) {
    return gst_element_register(plugin, "iyakkimp3dec", GST_RANK_PRIMARY + 1, IYAKKI_TYPE_MP3_DEC);
}

#ifndef MP3DEC_NO_PLUGIN_DEFINE
GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, iyakkimp3, "IYAKKI MP3 decoder (minimp3, CC0)", mp3_plugin_init, "1", "unknown", "IYAKKI", "iyakki")
#endif
