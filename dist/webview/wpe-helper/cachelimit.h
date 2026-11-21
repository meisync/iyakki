/* The engine's disk cache limit (message 0x1a CacheLimit, F-BROX-17), pure parts: plain libc, no WebKit and no GLib, so cache-check.c compiles
 * and runs them anywhere. WebKit has no call that sets the size of its disk cache, so the helper keeps the limit itself: it adds up the cache
 * folder and, when the folder is above the limit, empties the disk cache the way the reader's cache is emptied. */
#ifndef IYAKKI_CACHELIMIT_H
#define IYAKKI_CACHELIMIT_H

#include <dirent.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>

#define CL_MAX_ENTRIES 200000u
#define CL_MAX_DEPTH 8

/* limits above 64 GB are no limit (a typo is not a limit anyone can see); 0 is no limit */
static inline uint32_t cl_limit_mb(uint32_t mb) { return mb > 65536u ? 0 : mb; }

/* Total bytes of the regular files under path (links are not followed, a folder that cannot be read counts nothing). Stops after
 * CL_MAX_ENTRIES entries and says so through *cut, so a huge or hostile tree cannot hold the helper. */
static inline uint64_t cl_dir_bytes(const char *path, int depth, uint32_t *seen, int *cut) {
    DIR *d = opendir(path);
    if (!d) return 0;
    uint64_t total = 0;
    struct dirent *e;
    while ((e = readdir(d))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        if (++*seen > CL_MAX_ENTRIES) { *cut = 1; break; }
        char p[PATH_MAX];
        if (snprintf(p, sizeof p, "%s/%s", path, e->d_name) >= (int)sizeof p) continue;
        struct stat st;
        if (lstat(p, &st) != 0) continue;
        if (S_ISREG(st.st_mode)) total += (uint64_t)st.st_size;
        else if (S_ISDIR(st.st_mode) && depth < CL_MAX_DEPTH) total += cl_dir_bytes(p, depth + 1, seen, cut);
    }
    closedir(d);
    return total;
}

/* 1 when the cache folder is above the limit (limit_mb 0 = never); a cut count (a tree too big to add up) counts as above */
static inline int cl_over(const char *dir, uint32_t limit_mb) {
    if (!limit_mb || !dir || !*dir) return 0;
    uint32_t seen = 0;
    int cut = 0;
    uint64_t bytes = cl_dir_bytes(dir, 0, &seen, &cut);
    return cut || bytes > (uint64_t)limit_mb * 1024u * 1024u;
}

#endif
