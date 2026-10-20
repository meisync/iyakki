;; __TITLE__: a sandboxed IYAKKI tool. The host puts the JSON arguments at 0x10000 (65536) and calls run(length);
;; the module writes its JSON answer at 0x20000 (131072) and returns its length. This starting point echoes its input.
;; Replace the body of `run` (or compile Rust or C to wasm32 with the same exports) and keep `memory` and `run`.
(module
  (memory (export "memory") 3)
  (func (export "run") (param $n i32) (result i32)
    (memory.copy (i32.const 131072) (i32.const 65536) (local.get $n))
    (local.get $n)))
