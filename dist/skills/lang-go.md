---
name: lang-go
description: Write, run and explain Go code: go run/build/test, types, functions, slices, maps, structs, strings, errors, goroutines and best practices
triggers: go, golang, go.mod, gofmt, goroutine, channel, slice, go run, go test
---
Use this when writing or explaining Go.

1. Run: `go run main.go`; project `go mod init example.com/app`, `go build ./...`, `go test ./...`, `go vet ./...`. Layout: go.mod, main.go (`package main`, `func main()`), pkg files, `*_test.go`.
2. Comments: `// line`, `/* block */`; doc comment is the line above a declaration, starting with its name: `// Add returns a+b.`
3. Variables: `var n int = 5`, short form `x := 2.5` (inside functions), `const Max = 10`. Types: int, int64, float64, bool, string, rune, byte, any. No mutability flag. Zero values (0, "", nil). Names: camelCase, exported names start uppercase, short names.
4. Functions: `func add(a, b int) int { return a + b }`; multiple returns `func div(a, b int) (int, error)`; variadic `func sum(xs ...int) int`; closure `sq := func(x int) int { return x*x }`; generics `func Max[T int|float64](a, b T) T`. No default args.
5. Branch: `if x > 0 {...} else if ... {...} else {...}`; with init `if v, err := f(); err != nil {...}`; no ternary; `switch c { case 1: ... default: ... }` (no fallthrough; `switch { case x>1: }`).
6. Loops: only `for`: `for i := 0; i < n; i++ {}`, `for cond {}`, `for {}`, `for i, v := range xs {}`, `for k, v := range m {}`, `for range 3 {}` (Go 1.22+, needs `go 1.22` in go.mod), break/continue.
7. Data:
- `xs := []int{3,1,2}; xs = append(xs, 4); len(xs); xs[1:3]`; remove index i: `xs = append(xs[:i], xs[i+1:]...)`
- `sort.Ints(xs)`, `sort.Slice(xs, func(i, j int) bool { return xs[i] < xs[j] })`; `slices.Sort(xs)`, `slices.Contains(xs, 3)` (Go 1.21+, import "slices")
- `m := map[string]int{"a": 1}; m["b"] = 2; v, ok := m["a"]; delete(m, "a")` (iteration order is random)
- Set: `map[string]struct{}`. Struct: `type P struct { Name string; Age int }; p := P{"Al", 3}` methods `func (p P) Hi() string`.
8. Strings (strings, strconv, fmt): `len(s)` (bytes), `s[1:3]`, `strings.Split(s, ",")`, `strings.Join(xs, ",")`, `strings.TrimSpace(s)`, `strings.ReplaceAll(s, "a", "b")`, `strings.Index(s, "x")` (-1), `strings.Contains`, `strings.ToUpper`, `fmt.Sprintf("%s %d %.2f %v", a, b, c, d)`, `strconv.Atoi(s)` (returns n, err), `strconv.ParseFloat(s, 64)`. Regex: `regexp.MustCompile("\\d+").FindString(s)`. Print: `fmt.Println(x)`. Files: `os.ReadFile(p)`, `os.WriteFile(p, b, 0644)`. JSON: `json.Marshal(v)`, `json.Unmarshal(b, &v)` (encoding/json; exported fields, tags `json:"name"`). Time: `time.Now()`, `t.Format("2006-01-02")`.
9. Errors: values, not exceptions: `v, err := f(); if err != nil { return fmt.Errorf("load: %w", err) }`; create `errors.New("msg")`; check `errors.Is(err, target)`; `defer f.Close()`; panic only for bugs.
10. Best practices:
- Style: Effective Go; always `gofmt -w .` (or goimports), lint with `go vet`, tests with `go test -race -cover ./...`.
- Always check err; never ignore with `_` unless justified.
- Pass `context.Context` first; `defer` cleanup right after opening a resource.
- Goroutines: `go f()` with `sync.WaitGroup`; share by channels `ch := make(chan int)`; protect shared maps with `sync.Mutex`; avoid goroutine leaks.
- Security: `database/sql` placeholders (`?`), use `exec.Command(name, args...)` not shell strings, `html/template` for HTML, validate input.
- Keep interfaces small, accept interfaces and return structs; slices share memory (copy when needed); do not write to a nil map.
11. Assistant: when asked to write Go, return a complete runnable program (package main), run it if a tool is available (sandbox_run language go when the container cluster supports it), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.
