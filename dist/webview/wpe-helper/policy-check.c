/* `policy-check HEIGHT CODECS [BASE-RANK]`: prints the prefix line and the rank string of mediapolicy.h so a test can compare them. */
#include "mediapolicy.h"
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    char pre[128], rank[512];
    mp_prefix(pre, sizeof pre, (uint32_t)strtoul(argv[1], NULL, 10), atoi(argv[2]));
    int rc = mp_rank(rank, sizeof rank, argc > 3 ? argv[3] : NULL, atoi(argv[2]));
    printf("%srank %d %s\n", pre, rc, rank);
    return 0;
}
