(module
  (import "iyakki" "log" (func $log (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "hello from guest\n")
  (func (export "_start") (call $log (i32.const 0) (i32.const 17))))
