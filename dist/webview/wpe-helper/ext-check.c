/* `ext-check snapshot.bin cases.txt [report-text]`: parses a snapshot written by the Rust encoder (crates/iyakki-webview/src/ext.rs)
 * with extsnap.h and checks the headers chosen for each case line `URL<TAB>name,name` (`-` for none). With a third argument the
 * parse report must contain that text. Exit 0 when everything agrees. Run by iyakki-agent's browserext tests; needs only a C compiler. */
#include "extsnap.h"

static uint8_t *slurp(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb");
    if (!f) { perror(path); exit(2); }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *b = malloc((size_t)len + 1);
    if (fread(b, 1, (size_t)len, f) != (size_t)len) exit(2);
    fclose(f);
    *n = (size_t)len;
    return b;
}

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    size_t n;
    uint8_t *snap = slurp(argv[1], &n);
    XHeaders h = {0};
    char rep[1024];
    int rc = xs_parse(snap, n, &h, rep, sizeof rep);
    printf("parse %d, %u rules, report: %s\n", rc, h.n, rep);
    if (argc > 3 && !strstr(rep, argv[3])) { printf("FAIL: report lacks '%s'\n", argv[3]); return 1; }
    size_t cn;
    char *cases = (char *)slurp(argv[2], &cn);
    cases[cn] = 0;
    int bad = 0, count = 0;
    for (char *line = strtok(cases, "\n"); line; line = strtok(NULL, "\n")) {
        char *tab = strchr(line, '\t');
        if (!tab) continue;
        *tab = 0;
        const char *want = tab + 1;
        XTarget t;
        char got[512] = "";
        if (xs_target(line, &t) == 0) {
            XOut out[XS_MAX_PER_REQUEST];
            int k = xs_headers_for(&h, &t, out);
            for (int i = 0; i < k; i++) { if (i) strcat(got, ","); strncat(got, out[i].name, 40); }
        }
        if (!got[0]) strcpy(got, "-");
        count++;
        if (strcmp(got, want)) { printf("FAIL %s: want %s got %s\n", line, want, got); bad++; }
    }
    printf("%d cases, %d failed\n", count, bad);
    return bad ? 1 : 0;
}
