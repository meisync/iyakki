/*
 * IYAKKI microphone scan (SPDX-License-Identifier: Apache-2.0). Header only: included by micshare.c (the GStreamer device provider),
 * by helper.c (one log line per microphone request) and by mic-check.c (the self check). Needs glib; alsa-lib only when
 * IYAKKI_MIC_WITH_ALSA is defined (the capability probe).
 *
 * What it does: reads /proc/asound/cards and /proc/asound/card<N>/pcm<D>c/info (a capture stream per file), skips what is not a
 * microphone (HDMI, DisplayPort, IEC958/S-PDIF, loopbacks such as the T2 "Bridge Loopback"), ranks the rest (a device called "mic"
 * first, then a USB microphone, then analog inputs, then the rest), and for every device asks alsa-lib what the hardware offers
 * (rate, channels) WITHOUT keeping the device open. The devices are named for the person from the card ("Built-in microphone
 * (Apple T2 Audio)") and addressed as plughw:C,D, the route IYAKKI's own dictation uses: plughw converts rate, format and channels,
 * so a page asking for 44100 Hz stereo gets it from hardware that only does 48000 Hz.
 *
 * The sound card of a T2 Mac (driver AppleT2x4) is special: its capture streams are refused by the driver unless the module
 * parameter apple_bce capture is on, and opening one while something plays has hung the computer. So its devices are listed only
 * when the parameter is on (the same gate as the dictation of the Voice tile) and they are never opened to be inspected.
 */
#ifndef IYAKKI_MICSCAN_H
#define IYAKKI_MICSCAN_H
#include <ctype.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <glib.h>
#include <glib/gstdio.h>
#ifdef IYAKKI_MIC_WITH_ALSA
#include <alsa/asoundlib.h>
#endif

typedef struct {
    int card, dev;
    char driver[40], card_id[40], card_name[100], pcm_name[100];
    char alsa[24];            /* plughw:C,D */
    char label[240];          /* what the person sees */
    gboolean usb, t2, probed;
    int score;
    int rate_min, rate_max, ch_min, ch_max;
    char probe[300];          /* what the probe found, or why it failed */
} IyakkiMic;

typedef struct { GPtrArray *mics; GPtrArray *skipped; gchar *choice; gboolean choice_found; } IyakkiMicScan;   /* IyakkiMic*, char*; the person's choice */

static void micscan_free(IyakkiMicScan *s) {
    if (!s) return;
    if (s->mics) g_ptr_array_free(s->mics, TRUE);
    if (s->skipped) g_ptr_array_free(s->skipped, TRUE);
    g_free(s->choice);
    g_free(s);
}

static gchar *micscan_read(const char *dir, const char *file) {
    gchar *p = g_build_filename(dir, file, NULL), *text = NULL;
    if (!g_file_get_contents(p, &text, NULL, NULL)) text = NULL;
    g_free(p);
    return text;
}

static gboolean micscan_has_ci(const char *hay, const char *needle) {
    gchar *a = g_ascii_strdown(hay, -1), *b = g_ascii_strdown(needle, -1);
    gboolean r = strstr(a, b) != NULL;
    g_free(a); g_free(b);
    return r;
}

/* " 0 [AppleT2x4      ]: AppleT2x4 - Apple T2 Audio" -> card number, id, driver, name. FALSE for any other line. */
static gboolean micscan_parse_card_line(const char *line, int *card, char *id, char *driver, char *name) {
    const char *p = line;
    while (*p == ' ') p++;
    if (!isdigit((unsigned char)*p)) return FALSE;
    *card = atoi(p);
    const char *lb = strchr(p, '['), *rb = lb ? strchr(lb, ']') : NULL, *colon = rb ? strstr(rb, "]:") : NULL;
    if (!lb || !rb || !colon) return FALSE;
    gchar *i = g_strstrip(g_strndup(lb + 1, (gsize)(rb - lb - 1)));
    g_strlcpy(id, i, 40); g_free(i);
    const char *rest = colon + 2;
    while (*rest == ' ') rest++;
    const char *dash = strstr(rest, " - ");
    gchar *d = g_strstrip(dash ? g_strndup(rest, (gsize)(dash - rest)) : g_strdup(rest));
    g_strlcpy(driver, d, 40); g_free(d);
    gchar *n = g_strstrip(g_strdup(dash ? dash + 3 : rest));
    g_strlcpy(name, n, 100); g_free(n);
    return TRUE;
}

/* "key: value" of a pcm info file */
static gchar *micscan_info_value(const char *text, const char *key) {
    gchar **lines = g_strsplit(text, "\n", -1);
    gchar *out = NULL;
    gsize kl = strlen(key);
    for (int i = 0; lines[i] && !out; i++)
        if (!strncmp(lines[i], key, kl) && lines[i][kl] == ':') out = g_strstrip(g_strdup(lines[i] + kl + 1));
    g_strfreev(lines);
    return out;
}

/* Why a stream is not a microphone, or NULL */
static const char *micscan_reject(const IyakkiMic *m) {
    static const char *const bad[] = { "hdmi", "displayport", "iec958", "spdif", "s/pdif", "loopback", "bridge", NULL };
    for (int i = 0; bad[i]; i++)
        if (micscan_has_ci(m->pcm_name, bad[i]) || micscan_has_ci(m->card_name, bad[i]) || micscan_has_ci(m->driver, bad[i])) return bad[i];
    return NULL;
}

static int micscan_score(const IyakkiMic *m) {
    if (micscan_has_ci(m->pcm_name, "mic")) return 100;
    if (m->usb) return 80;
    if (micscan_has_ci(m->pcm_name, "analog")) return 60;
    if (micscan_has_ci(m->pcm_name, "input") || micscan_has_ci(m->pcm_name, "line")) return 30;
    return 40;
}

static void micscan_label(IyakkiMic *m) {
    const char *card = m->card_name[0] ? m->card_name : m->card_id;
    if (m->usb) g_snprintf(m->label, sizeof m->label, "%s (USB microphone)", card);
    else if (micscan_has_ci(m->pcm_name, "mic")) g_snprintf(m->label, sizeof m->label, "Built-in microphone (%s)", card);
    else g_snprintf(m->label, sizeof m->label, "%s (%s)", m->pcm_name[0] ? m->pcm_name : "Audio input", card);
}

#ifdef IYAKKI_MIC_WITH_ALSA
/* Asks the hardware, without keeping it: first hw (the limits of the device itself), then plughw. A T2 card is never opened. */
static void micscan_probe(IyakkiMic *m, gboolean *busy_only) {
    *busy_only = FALSE;
    if (m->t2) { g_strlcpy(m->probe, "not opened (T2 card: opening capture can hang it)", sizeof m->probe); return; }
    GString *why = g_string_new("");
    int busy = 0, tried = 0;
    for (int k = 0; k < 2; k++) {
        char name[24];
        g_snprintf(name, sizeof name, "%s:%d,%d", k ? "plughw" : "hw", m->card, m->dev);
        snd_pcm_t *h = NULL;
        int e = snd_pcm_open(&h, name, SND_PCM_STREAM_CAPTURE, SND_PCM_NONBLOCK);
        tried++;
        if (e < 0) { g_string_append_printf(why, "%s%s: %s", why->len ? "; " : "", name, snd_strerror(e)); if (e == -EBUSY) busy++; continue; }
        snd_pcm_hw_params_t *hp;
        snd_pcm_hw_params_alloca(&hp);
        e = snd_pcm_hw_params_any(h, hp);
        if (e >= 0) {
            unsigned lo = 0, hi = 0, cl = 0, ch = 0;
            int d = 0;
            snd_pcm_hw_params_get_rate_min(hp, &lo, &d);
            snd_pcm_hw_params_get_rate_max(hp, &hi, &d);
            snd_pcm_hw_params_get_channels_min(hp, &cl);
            snd_pcm_hw_params_get_channels_max(hp, &ch);
            m->rate_min = (int)lo; m->rate_max = (int)hi; m->ch_min = (int)cl; m->ch_max = (int)ch;
            m->probed = TRUE;
            g_snprintf(m->probe, sizeof m->probe, "%s %d-%d Hz %d-%d ch%s%s", name, m->rate_min, m->rate_max, m->ch_min, m->ch_max,
                       why->len ? " (after " : "", why->len ? why->str : "");
            if (why->len) g_strlcat(m->probe, ")", sizeof m->probe);
            snd_pcm_close(h);
            g_string_free(why, TRUE);
            return;
        }
        g_string_append_printf(why, "%s%s: hw_params_any: %s", why->len ? "; " : "", name, snd_strerror(e));
        snd_pcm_close(h);
    }
    *busy_only = busy == tried;
    g_snprintf(m->probe, sizeof m->probe, "cannot open: %s", why->str);
    g_string_free(why, TRUE);
}
#endif

static gint micscan_cmp(gconstpointer a, gconstpointer b) {
    const IyakkiMic *x = *(IyakkiMic *const *)a, *y = *(IyakkiMic *const *)b;
    if (x->score != y->score) return y->score - x->score;
    if (x->card != y->card) return x->card - y->card;
    return x->dev - y->dev;
}


/* The folder the shell and the plugin share (the shell writes the person's choice, the plugin writes why a microphone failed). */
#define MICSCAN_CHOICE_FILE "iyakki-browser-mic"
#define MICSCAN_STATUS_FILE "iyakki-browser-mic-status"
#include "devlist.h"
#define MICSCAN_HIDDEN_FILE "iyakki-browser-mic-hidden"
static const char *micscan_dir(void) { return devlist_dir(); }

/* The microphone the person chose in the Voice tile or the Browser (an ALSA id such as plughw:1,0), or NULL for "automatic". */
static gchar *micscan_read_choice(void) {
    gchar *p = g_build_filename(micscan_dir(), MICSCAN_CHOICE_FILE, NULL), *t = NULL;
    if (!g_file_get_contents(p, &t, NULL, NULL)) t = NULL;
    g_free(p);
    if (!t) return NULL;
    g_strstrip(t);
    if (!*t || strlen(t) > 30) { g_free(t); return NULL; }
    return t;
}

/* "<name>\t<reason>" for the shell, which tells the person in plain words; NULL label removes the file (the microphone works). */
G_GNUC_UNUSED static void micscan_write_status(const char *label, const char *reason) {
    gchar *p = g_build_filename(micscan_dir(), MICSCAN_STATUS_FILE, NULL);
    if (!label) remove(p);
    else {
        gchar *clean = g_strdelimit(g_strdup(reason ? reason : "unknown"), "\t\n\r", ' ');
        gchar *t = g_strdup_printf("%s\t%s\n", label, clean);
        g_file_set_contents(p, t, -1, NULL);
        g_free(t); g_free(clean);
    }
    g_free(p);
}

/* The person's choice goes first. Returns whether it was found. */
static gboolean micscan_apply_choice(IyakkiMicScan *s, const char *choice) {
    if (!choice) return FALSE;
    for (guint i = 0; i < s->mics->len; i++) {
        IyakkiMic *m = s->mics->pdata[i];
        if (!strcmp(m->alsa, choice)) {
            if (i > 0) { g_ptr_array_steal_index(s->mics, i); g_ptr_array_insert(s->mics, 0, m); }
            return TRUE;
        }
    }
    return FALSE;
}

/*
 * proc: the directory that holds cards and card<N> (/proc/asound); sys: the root of /sys (for the T2 capture switch);
 * probe: ask the hardware (needs IYAKKI_MIC_WITH_ALSA; tests pass FALSE).
 */
static IyakkiMicScan *micscan_run(const char *proc, const char *sys, gboolean probe) {
    IyakkiMicScan *s = g_new0(IyakkiMicScan, 1);
    s->mics = g_ptr_array_new_with_free_func(g_free);
    s->skipped = g_ptr_array_new_with_free_func(g_free);
    gchar *cards = micscan_read(proc, "cards");
    if (!cards) return s;
    gchar **lines = g_strsplit(cards, "\n", -1);
    GHashTable *hidden = devlist_read(MICSCAN_HIDDEN_FILE);   /* microphones the person marked "don't use": never listed, never opened */
    gchar *capfile = g_build_filename(sys, "module/apple_bce/parameters/capture", NULL), *capval = NULL;
    gboolean t2_on = FALSE;
    if (g_file_get_contents(capfile, &capval, NULL, NULL)) { g_strstrip(capval); t2_on = !strcmp(capval, "Y") || !strcmp(capval, "1"); }
    g_free(capval); g_free(capfile);
    for (int i = 0; lines[i]; i++) {
        int card; char id[40], drv[40], name[100];
        if (!micscan_parse_card_line(lines[i], &card, id, drv, name)) continue;
        gchar *cdir = g_strdup_printf("%s/card%d", proc, card);
        GDir *d = g_dir_open(cdir, 0, NULL);
        GPtrArray *names = g_ptr_array_new_with_free_func(g_free);
        const char *e;
        while (d && (e = g_dir_read_name(d)))
            if (g_str_has_prefix(e, "pcm") && g_str_has_suffix(e, "c") && isdigit((unsigned char)e[3])) g_ptr_array_add(names, g_strdup(e));
        if (d) g_dir_close(d);
        for (guint k = 0; k < names->len; k++) {
            IyakkiMic *m = g_new0(IyakkiMic, 1);
            m->card = card; m->dev = atoi((char *)names->pdata[k] + 3);
            g_strlcpy(m->card_id, id, sizeof m->card_id); g_strlcpy(m->driver, drv, sizeof m->driver); g_strlcpy(m->card_name, name, sizeof m->card_name);
            gchar *sub = g_strdup_printf("%s/%s", cdir, (char *)names->pdata[k]);
            gchar *info = micscan_read(sub, "info");
            gchar *pn = info ? micscan_info_value(info, "name") : NULL;
            g_strlcpy(m->pcm_name, pn ? pn : "", sizeof m->pcm_name);
            g_free(pn); g_free(info); g_free(sub);
            g_snprintf(m->alsa, sizeof m->alsa, "plughw:%d,%d", card, m->dev);
            m->usb = g_str_has_prefix(drv, "USB-Audio");
            m->t2 = g_str_has_prefix(drv, "AppleT2");
            const char *why = micscan_reject(m);
            if (!why && devlist_has(hidden, m->alsa)) why = "hidden by the person";
            if (!why && m->t2 && !t2_on) why = "T2 capture switched off (apple_bce capture=0)";
            if (why) {
                g_ptr_array_add(s->skipped, g_strdup_printf("%d,%d '%s' (%s)", card, m->dev, m->pcm_name, why));
                g_free(m);
                continue;
            }
            m->score = micscan_score(m);
            micscan_label(m);
#ifdef IYAKKI_MIC_WITH_ALSA
            if (probe) {
                gboolean busy_only = FALSE;
                micscan_probe(m, &busy_only);
                if (!m->probed && !m->t2 && !busy_only) {
                    g_ptr_array_add(s->skipped, g_strdup_printf("%d,%d '%s' (%s)", card, m->dev, m->pcm_name, m->probe));
                    g_free(m);
                    continue;
                }
            } else
#endif
            { (void)probe; g_strlcpy(m->probe, m->t2 ? "not opened (T2 card)" : "not probed", sizeof m->probe); }
            g_ptr_array_add(s->mics, m);
        }
        g_ptr_array_free(names, TRUE);
        g_free(cdir);
    }
    g_strfreev(lines);
    g_free(cards);
    g_hash_table_unref(hidden);
    g_ptr_array_sort(s->mics, micscan_cmp);
    s->choice = micscan_read_choice();
    s->choice_found = micscan_apply_choice(s, s->choice);
    return s;
}

/* The one mode a shared hub serves: S16LE at 48000 Hz, stereo when the hardware has two channels or more, else mono. The T2
 * microphone and any device that was not probed give mono (what the dictation records and the T2 driver is known to do). */
static int micscan_channels(const IyakkiMic *m) { return m->probed && m->ch_max >= 2 ? 2 : 1; }

static gchar *micscan_report(const IyakkiMicScan *s, const char *when) {
    GString *g = g_string_new("");
    g_string_append_printf(g, "microphone: %s; capture devices (%u):", when, s->mics->len);
    if (!s->mics->len) g_string_append(g, " none");
    for (guint i = 0; i < s->mics->len; i++) {
        const IyakkiMic *m = s->mics->pdata[i];
        g_string_append_printf(g, " [%s '%s' score %d, %s]", m->alsa, m->label, m->score, m->probe);
    }
    if (s->choice) g_string_append_printf(g, "; the person's choice %s is %s", s->choice, s->choice_found ? "first" : "not present (automatic order used)");
    if (s->skipped->len) {
        g_string_append(g, "; skipped:");
        for (guint i = 0; i < s->skipped->len; i++) g_string_append_printf(g, " [%s]", (char *)s->skipped->pdata[i]);
    }
    if (s->mics->len) {
        const IyakkiMic *m = s->mics->pdata[0];
        g_string_append_printf(g, "; chosen %s via alsasrc ! audioconvert ! audioresample as S16LE 48000 Hz %d ch", m->alsa, micscan_channels(m));
    } else g_string_append(g, "; chosen none");
    return g_string_free(g, FALSE);
}
#endif
