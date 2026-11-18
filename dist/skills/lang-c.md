---
name: lang-c
description: Write, compile and explain C (C11): gcc/clang commands, types, pointers, functions, control flow, arrays/structs, string.h, stdio, error handling, memory safety best practices.
triggers: c, gcc, clang, makefile, malloc, pointer, printf, stdio, valgrind, header, cc
---
C reference for writing and explaining code.

1. Build: `gcc -std=c11 -Wall -Wextra -g main.c -o main` then `./main`; syntax only `gcc -fsyntax-only main.c`; multiple files `gcc a.c b.c -o app`; link math `-lm`; debug `gcc -fsanitize=address,undefined`, `valgrind ./main`. Layout: main.c, util.c, util.h (with include guard `#ifndef UTIL_H` ... `#endif`), Makefile. Entry: `int main(void) { ...; return 0; }`.
2. Comments: `// line`, `/* block */`. Doc style: a block above each function stating purpose, parameters, return.
3. Variables: `int n = 3; double d = 1.5; char c = 'a'; long big = 1L; unsigned u = 5;` `const int MAX = 10;` or `#define MAX 10`. Fixed width: `#include <stdint.h>` for `uint8_t`, `int64_t`; `bool` from `<stdbool.h>`. Declare and initialize (locals are uninitialized). Names snake_case. Size `sizeof(x)`.
4. Functions:
```
int add(int a, int b) { return a + b; }
void fill(int *out, size_t n);
```
Declare prototypes before use. Arguments pass by value; pass a pointer to modify. No defaults or overloading. Varargs `int f(int n, ...)` with `<stdarg.h>`. Function pointer: `int (*op)(int, int) = add;`.
5. Branching: `if (x > 0) {...} else if (x == 0) {...} else {...}`; ternary `c ? a : b`; `switch (k) { case 1: ...; break; default: ...; }` (do not forget break). Nonzero is true. Always use `==` in conditions, not `=`.
6. Loops: `for (int i = 0; i < n; i++)`, `while (c)`, `do {...} while (c);`, `break`, `continue`. Iterate array with its length: `for (size_t i = 0; i < n; i++) sum += a[i];`.
7. Data:
 - array: `int a[5] = {1, 2}; int n = sizeof a / sizeof a[0];` (only where a is a real array, not a parameter). No bounds checks, no add/remove: resize with `realloc`.
 - sort: `qsort(a, n, sizeof a[0], cmp);` with `int cmp(const void *x, const void *y)`.
 - struct: `typedef struct { int x; int y; } Point; Point p = {1, 2}; p.x; Point *q = &p; q->x;`.
 - dynamic: `int *a = malloc(n * sizeof *a); if (!a) return 1; ...; free(a);`.
 - enum: `enum Color { RED, GREEN };`. No built-in map or set: use arrays, or write a hash table.
8. Strings (char arrays ending in '\0', `#include <string.h>`): `strlen(s)`, `strcmp(a, b) == 0`, `strncpy`, `strcat`, `strchr(s, 'x')`, `strstr(s, "sub")`, `snprintf(buf, sizeof buf, "%s: %d", name, n)`, `printf("%d %f %s\n", i, d, s)`. Numbers: `strtol(s, &end, 10)`, `strtod`, `atoi` (no error check, avoid). Upper/lower per char: `toupper(c)` (ctype.h). Files: `FILE *f = fopen("f.txt", "r"); fgets(buf, sizeof buf, f); fclose(f);`. Time: `time(NULL)`. `<stdlib.h>` for malloc, exit, qsort.
9. Errors: functions return codes or NULL; check every result: `if (!f) { perror("fopen"); return 1; }`. `errno` from `<errno.h>`. Exit codes: `exit(EXIT_FAILURE)`. No exceptions; free resources on each error path (a single `goto cleanup;` label is idiomatic).
10. Best practice: compile with `-Wall -Wextra -Werror`; check malloc, fopen and scanf results; never use `gets`, `strcpy`, `sprintf`, `scanf("%s")` without bounds (use `fgets`, `snprintf`, `strncpy` with terminating NUL); no out-of-bounds, use-after-free or double free; set freed pointers to NULL; use `size_t` for sizes; never pass user text as a printf format (`printf("%s", s)`); validate input lengths; format with `clang-format`.
11. When asked to write C, return one complete file with includes and `main` that compiles cleanly with `gcc -Wall -Wextra`, run it if a tool is available, and explain compiler or runtime errors in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.
