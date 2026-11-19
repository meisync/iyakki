;; IYAKKI tool fixture: reads {FILE} with capability token #0 and returns its bytes as the output.
;; Tests replace {FILE} and {LEN} (its length).
(module
  (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
  (import "iyakki" "fs_read" (func $fr (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 3)
  (data (i32.const 64) "{FILE}")
  (func (export "run") (param i32) (result i32)
    (drop (call $tg (i32.const 0) (i32.const 0)))
    (call $fr (i32.const 0) (i32.const 64) (i32.const {LEN}) (i32.const 131072) (i32.const 4096))))
