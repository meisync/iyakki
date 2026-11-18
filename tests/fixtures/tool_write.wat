;; IYAKKI tool fixture: writes "ok" to {FILE} using capability token #0 and returns the host's status
;; (0 = written, negative = refused). Tests replace {FILE} and {LEN} (its length).
(module
  (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
  (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 3)
  (data (i32.const 64) "{FILE}")
  (data (i32.const 4096) "ok")
  (func (export "run") (param i32) (result i32)
    (drop (call $tg (i32.const 0) (i32.const 0)))
    (call $fw (i32.const 0) (i32.const 64) (i32.const {LEN}) (i32.const 4096) (i32.const 2))))
