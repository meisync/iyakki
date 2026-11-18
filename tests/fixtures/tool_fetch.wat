;; IYAKKI tool fixture: GETs https://example.com/ with capability token #0 and returns the response.
(module
  (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
  (import "iyakki" "http_fetch" (func $hf (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 3)
  (data (i32.const 64) "https://example.com/")
  (func (export "run") (param i32) (result i32)
    (drop (call $tg (i32.const 0) (i32.const 0)))
    (call $hf (i32.const 0) (i32.const 64) (i32.const 20) (i32.const 0) (i32.const 0) (i32.const 131072) (i32.const 4096))))
