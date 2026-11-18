;; IYAKKI tool fixture: never returns (the budget has to stop it).
(module
  (memory (export "memory") 3)
  (func (export "run") (param i32) (result i32)
    (loop $l (br $l))
    (i32.const 0)))
