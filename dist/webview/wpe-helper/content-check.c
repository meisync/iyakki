/* `content-check snapshot.bin`: parses a snapshot written by the Rust encoder (crates/iyakki-webview/src/ext.rs) with extsnap.h and prints what the
 * helper would act on, one line each, so a test can compare it with what the Rust side put in:
 *   parse RC, N scripts
 *   report: TEXT
 *   script|style start|end BODYLEN allow=a,b
 *   ua=TEXT (or ua=- when the snapshot says nothing)
 * Needs only a C compiler. Run by iyakki-agent's browserext tests. */
#include "extsnap.h"

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    FILE *f = fopen(argv[1], "rb");
    if (!f) { perror(argv[1]); return 2; }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *b = malloc((size_t)len + 1);
    if (fread(b, 1, (size_t)len, f) != (size_t)len) return 2;
    fclose(f);
    XHeaders h = {0};
    XContent c = {0};
    char rep[1024];
    int rc = xs_parse_all(b, (size_t)len, &h, &c, rep, sizeof rep);
    printf("parse %d, %u scripts, %u header rules\n", rc, c.n, h.n);
    printf("report: %s\n", rep);
    for (uint32_t i = 0; i < c.n; i++) {
        XScript *s = &c.s[i];
        printf("%s %s %zu allow=", s->style ? "style" : "script", s->at_end ? "end" : "start", strlen(s->body));
        for (uint32_t k = 0; k < s->sites.nallow; k++) printf("%s%s", k ? "," : "", s->sites.allow[k]);
        printf(" nullterm=%d\n", s->sites.allow[s->sites.nallow] == NULL);
    }
    printf("ua=%s\n", c.has_ua ? (c.ua[0] ? c.ua : "(engine's own)") : "-");
    xs_content_free(&c);
    xs_free(&h);
    free(b);
    return rc < 0 ? 1 : 0;
}
