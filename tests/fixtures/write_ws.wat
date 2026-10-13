;; Writes "ok" to /ws/out using token #0; exits with the status code (0 = allowed, 2 = denied as errno 254..).
(module
  (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
  (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 64) "/ws/out")
  (data (i32.const 128) "ok")
  (func (export "_start") (result i32)
    (drop (call $tg (i32.const 0) (i32.const 0)))
    (call $fw (i32.const 0) (i32.const 64) (i32.const 7) (i32.const 128) (i32.const 2))))
