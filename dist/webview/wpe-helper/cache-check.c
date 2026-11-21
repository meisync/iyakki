/* `cache-check DIR LIMIT_MB`: prints `bytes N over 0|1` for DIR so a test can build a folder of known size and compare. */
#include "cachelimit.h"
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    uint32_t seen = 0;
    int cut = 0;
    uint64_t b = cl_dir_bytes(argv[1], 0, &seen, &cut);
    printf("bytes %llu entries %u cut %d over %d limit %u\n", (unsigned long long)b, seen, cut, cl_over(argv[1], cl_limit_mb((uint32_t)strtoul(argv[2], NULL, 10))), cl_limit_mb((uint32_t)strtoul(argv[2], NULL, 10)));
    return 0;
}
