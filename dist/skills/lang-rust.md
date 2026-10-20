---
name: lang-rust
description: Write, run and explain Rust code: cargo, ownership, types, functions, match, iterators, Vec/HashMap, strings, Result error handling and best practices
triggers: rust, cargo, rustc, rs, crate, clippy, rustfmt, borrow, ownership
---
Use this when writing or explaining Rust (2021 edition).

1. Run: `cargo new app && cd app && cargo run`; `cargo check`, `cargo build --release`, `cargo test`, `cargo add <crate>`. Single file: `rustc --edition 2021 main.rs && ./main`. Layout: Cargo.toml, src/main.rs (`fn main()`), src/lib.rs, tests/.
2. Comments: `// line`, `/* block */`, doc `/// text` (item) and `//! text` (module).
3. Variables: immutable by default `let x = 5; let mut n = 0; const MAX: u32 = 10;` shadowing `let x = x + 1;`. Types: i32, i64, u8, usize, f64, bool, char, String, &str. Names: snake_case values/functions, CamelCase types, SCREAMING_CASE consts.
4. Functions: `fn add(a: i32, b: i32) -> i32 { a + b }` (last expression, no semicolon, is returned); no defaults or varargs (use Option or a struct, macros like `println!`); closure `let sq = |x: i32| x * x;`; generics `fn mx<T: PartialOrd>(a: T, b: T) -> T { if a > b { a } else { b } }`.
5. Branch: `if a {..} else if b {..} else {..}` is an expression: `let y = if a { 1 } else { 2 };` `match n { 1 => "one", 2 | 3 => "few", 4..=9 => "some", _ => "many" }`; `if let Some(v) = opt {..}`.
6. Loops: `for i in 0..n {}`, `for v in &vec {}`, `for (i, v) in vec.iter().enumerate() {}`, `while c {}`, `loop { break; }`, break/continue. Iterators: `v.iter().map(|x| x * 2).filter(|x| *x > 2).collect::<Vec<_>>()`, `.sum::<i32>()`.
7. Data:
- `let mut v = vec![3, 1, 2]; v.push(4); v.pop(); v.remove(0); v.len(); v.sort(); v.contains(&2);` array `[i32; 3]`
- `use std::collections::{HashMap, HashSet};` `let mut m = HashMap::new(); m.insert("a", 1); m.get("a"); m.remove("a"); m.contains_key("a"); for (k, val) in &m {}`; `*m.entry("a").or_insert(0) += 1;`
- `let mut s = HashSet::new(); s.insert(1); s.contains(&1);` BTreeMap for ordered.
- Tuple `let t = (1, "x"); t.0`; struct `struct P { x: i32 }` with `impl P { fn new(x: i32) -> Self { P { x } } }`; `#[derive(Debug, Clone)]`; enum `enum Shape { Circle(f64), Square(f64) }`.
8. Strings: `s.len()` (bytes), `&s[1..3]`, `s.split(',')`, `v.join(",")`, `s.trim()`, `s.replace("a", "b")`, `s.find("x")` (Option), `s.contains("x")`, `s.to_uppercase()`, `s.chars()`, `format!("{} {:.2} {:?}", a, b, c)` or `{name}` inline. `let s = String::from("hi"); s.push_str("x");` Parse: `"5".parse::<i32>()` (Result). Print: `println!("{}", x)`. Files: `std::fs::read_to_string(p)?`, `std::fs::write(p, text)?`. Regex and JSON need crates `regex`, `serde_json`. Time: `std::time::Instant::now()`.
9. Errors: no exceptions. `fn f() -> Result<i32, Box<dyn std::error::Error>> { let n: i32 = "5".parse()?; Ok(n) }`; `match r { Ok(v) => .., Err(e) => .. }`; `Option` with `.unwrap_or(0)`, `?`; `panic!` only for bugs; avoid `unwrap()` outside tests.
10. Best practices:
- Style: Rust API Guidelines; run `cargo fmt` and `cargo clippy -- -D warnings`; tests `#[test]` with `cargo test`.
- Ownership: a value has one owner; borrow with `&` / `&mut` instead of cloning; clone only when needed.
- Prefer `&str` for params, `String` for owned; iterators over index loops.
- Model states with enums and `match` (exhaustive).
- Avoid `unsafe`; if used, wrap it in a small safe API with a SAFETY comment.
- Security: validate input, no shell strings (use `Command::new(x).arg(y)`), check integer overflow (`checked_add`), run `cargo audit`.
11. Assistant: when asked to write Rust, return a complete runnable program with `fn main()`, run it if a tool is available (sandbox_run language rust when the container cluster supports it), and explain any compiler error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.
