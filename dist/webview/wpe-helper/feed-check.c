/* `feed-check FILE AFTER`: prints `ok W H SEQ SUM` (SUM is the sum of all bytes mod 2^32) or `none` for the feed file FILE. A test writes the file
 * with the shell's real writer and compares. */
#include "screenfeed.h"

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    SfFrame f;
    if (!sf_read(argv[1], strtoull(argv[2], NULL, 10), &f)) { puts("none"); return 0; }
    uint32_t sum = 0;
    for (size_t i = 0; i < (size_t)f.w * f.h * 4; i++) sum += f.rgba[i];
    printf("ok %u %u %llu %u\n", f.w, f.h, (unsigned long long)f.seq, sum);
    free(f.rgba);
    return 0;
}
