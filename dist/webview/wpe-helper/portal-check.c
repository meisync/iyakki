/* `portal-check KIND SENDER TOKEN` prints the object path or `refused`; `portal-check start FEED` prints whether Start may answer. */
#include "portalpure.h"

int main(int argc, char **argv) {
    char p[256];
    if (argc == 3 && !strcmp(argv[1], "start")) { puts(pp_may_start(argv[2]) ? "may" : "may not"); return 0; }
    if (argc < 4) return 2;
    puts(pp_path(argv[1], argv[2], argv[3], p, sizeof p) ? p : "refused");
    return 0;
}
