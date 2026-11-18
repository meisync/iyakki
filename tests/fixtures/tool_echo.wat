;; IYAKKI tool fixture: returns its JSON input unchanged. Input is at 0x10000, output at 0x20000.
(module
  (memory (export "memory") 3)
  (func (export "run") (param $n i32) (result i32)
    (memory.copy (i32.const 131072) (i32.const 65536) (local.get $n))
    (local.get $n)))
