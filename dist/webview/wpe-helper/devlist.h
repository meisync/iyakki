/*
 * The files the shell hands to IYAKKI's GStreamer plugins (SPDX-License-Identifier: Apache-2.0): the person's choices about cameras and
 * microphones. Both plugins include this header (micscan.h for the microphones, camshare.c for the cameras).
 *
 *   iyakki-browser-mic          the microphone to use first (an ALSA id such as plughw:1,0); no file: automatic
 *   iyakki-browser-mic-hidden   microphones the person marked "don't use", one per line
 *   iyakki-browser-cam          the camera to use first (the V4L2 card name); no file: automatic
 *   iyakki-browser-cam-hidden   cameras marked "don't use", one name per line
 *
 * The folder is IYAKKI_MIC_DIR when set (tests), else /dev/shm, else the temp folder: the same rule as the shell (iyakki-agent
 * sysinfo::shared_memory_dir), so both sides always meet. Names are compared with the spaces at both ends removed.
 */
#ifndef IYAKKI_DEVLIST_H
#define IYAKKI_DEVLIST_H
#include <glib.h>
#include <string.h>

#define DEVLIST_CAM_CHOICE "iyakki-browser-cam"
#define DEVLIST_CAM_HIDDEN "iyakki-browser-cam-hidden"
#define DEVLIST_MAX_ITEMS 64
#define DEVLIST_MAX_LEN 120

static const char *devlist_dir(void) {
    const char *e = g_getenv("IYAKKI_MIC_DIR");
    if (e && *e) return e;
    return g_file_test("/dev/shm", G_FILE_TEST_IS_DIR) ? "/dev/shm" : g_get_tmp_dir();
}

/* The trimmed lines of a file as a set (keys are strings); an empty set when the file is missing. Free with g_hash_table_unref. */
G_GNUC_UNUSED static GHashTable *devlist_read(const char *file) {
    GHashTable *set = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
    gchar *p = g_build_filename(devlist_dir(), file, NULL), *t = NULL;
    if (g_file_get_contents(p, &t, NULL, NULL)) {
        gchar **lines = g_strsplit(t, "\n", -1);
        for (int i = 0; lines[i] && g_hash_table_size(set) < DEVLIST_MAX_ITEMS; i++) {
            g_strstrip(lines[i]);
            if (*lines[i] && strlen(lines[i]) <= DEVLIST_MAX_LEN) g_hash_table_add(set, g_strdup(lines[i]));
        }
        g_strfreev(lines);
        g_free(t);
    }
    g_free(p);
    return set;
}

/* The first line of a file, trimmed, or NULL (free with g_free). */
G_GNUC_UNUSED static gchar *devlist_value(const char *file) {
    gchar *p = g_build_filename(devlist_dir(), file, NULL), *t = NULL, *out = NULL;
    if (g_file_get_contents(p, &t, NULL, NULL)) {
        gchar *nl = strchr(t, '\n');
        if (nl) *nl = 0;
        g_strstrip(t);
        if (*t && strlen(t) <= DEVLIST_MAX_LEN) out = g_strdup(t);
        g_free(t);
    }
    g_free(p);
    return out;
}

/* Whether `name` (an id or a device's display name) is in the set, ends trimmed. */
G_GNUC_UNUSED static gboolean devlist_has(GHashTable *set, const char *name) {
    if (!set || !name) return FALSE;
    gchar *n = g_strstrip(g_strdup(name));
    gboolean r = g_hash_table_contains(set, n);
    g_free(n);
    return r;
}
#endif
