;; A model-router strategy as a WebAssembly module (F-ROUTER-13). The core gives it the strategy input as JSON on `iyakki.stdin_read`
;; and takes the decision from `iyakki.log`; the module has NO other import, so it can touch no file, program or network.
;; What it does: the first listed model plans; in the assign phase it gives the first ready step to the first listed model; otherwise it has no
;; opinion (`{"v":1}`: try again). The core holds every answer to the contract and falls back to the built-in strategy when one is refused.
;; Add it with router_strategy_add (source = this text), then router_strategy_set.
(module
  (import "iyakki" "stdin_read" (func $rd (param i32 i32) (result i32)))
  (import "iyakki" "log" (func $log (param i32 i32)))
  (memory (export "memory") 4)
  (data (i32.const 0x30000) "\22phase\22:\22plan\22") ;; plan
  (data (i32.const 0x30020) "\22phase\22:\22assign\22") ;; assign
  (data (i32.const 0x30040) "\22models\22:[") ;; models
  (data (i32.const 0x30140) "\22id\22:\22") ;; id
  (data (i32.const 0x30060) "\22ready\22:[\22") ;; ready
  (data (i32.const 0x30080) "{\22v\22:1,\22planner\22:\22") ;; p1
  (data (i32.const 0x300a0) "\22}") ;; p2
  (data (i32.const 0x300c0) "{\22v\22:1,\22assign\22:[{\22step\22:\22") ;; a1
  (data (i32.const 0x300e0) "\22,\22model\22:\22") ;; a2
  (data (i32.const 0x30100) "\22}]}") ;; a3
  (data (i32.const 0x30120) "{\22v\22:1}") ;; none
  ;; the position after the first `needle` in the input [from, hlen), or -1
  (func $find (param $np i32) (param $nl i32) (param $from i32) (param $hl i32) (result i32)
    (local $i i32) (local $j i32)
    (local.set $i (local.get $from))
    (block $out (loop $next
      (br_if $out (i32.gt_s (i32.add (local.get $i) (local.get $nl)) (local.get $hl)))
      (local.set $j (i32.const 0))
      (block $miss (loop $cmp
        (if (i32.eq (local.get $j) (local.get $nl)) (then (return (i32.add (local.get $i) (local.get $nl)))))
        (br_if $miss (i32.ne (i32.load8_u (i32.add (local.get $i) (local.get $j))) (i32.load8_u (i32.add (local.get $np) (local.get $j)))))
        (local.set $j (i32.add (local.get $j) (i32.const 1)))
        (br $cmp)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $next)))
    (i32.const -1))
  ;; copy bytes of the input from `src` up to the next quote to `dst`; returns the new end of the output
  (func $word (param $src i32) (param $dst i32) (param $hl i32) (result i32)
    (local $c i32)
    (block $done (loop $more
      (br_if $done (i32.ge_u (local.get $src) (local.get $hl)))
      (local.set $c (i32.load8_u (local.get $src)))
      (br_if $done (i32.eq (local.get $c) (i32.const 34)))
      (i32.store8 (local.get $dst) (local.get $c))
      (local.set $src (i32.add (local.get $src) (i32.const 1)))
      (local.set $dst (i32.add (local.get $dst) (i32.const 1)))
      (br $more)))
    (local.get $dst))
  (func $put (param $src i32) (param $n i32) (param $dst i32) (result i32)
    (memory.copy (local.get $dst) (local.get $src) (local.get $n))
    (i32.add (local.get $dst) (local.get $n)))
  (func (export "decide") (result i32)
    (local $len i32) (local $n i32) (local $m i32) (local $r i32) (local $o i32) (local $end i32)
    (block $read (loop $again
      (local.set $n (call $rd (local.get $len) (i32.sub (i32.const 0x2f000) (local.get $len))))
      (br_if $read (i32.le_s (local.get $n) (i32.const 0)))
      (local.set $len (i32.add (local.get $len) (local.get $n)))
      (br_if $read (i32.ge_u (local.get $len) (i32.const 0x2f000)))
      (br $again)))
    (local.set $o (i32.const 0x31000))
    (local.set $m (call $find (i32.const 0x30040) (i32.const 10) (i32.const 0) (local.get $len)))
    (if (i32.ge_s (local.get $m) (i32.const 0)) (then (local.set $m (call $find (i32.const 0x30140) (i32.const 6) (local.get $m) (local.get $len)))))
    (local.set $r (call $find (i32.const 0x30060) (i32.const 10) (i32.const 0) (local.get $len)))
    (if (i32.ge_s (call $find (i32.const 0x30000) (i32.const 14) (i32.const 0) (local.get $len)) (i32.const 0))
      (then (if (i32.ge_s (local.get $m) (i32.const 0)) (then
        (local.set $end (call $put (i32.const 0x30080) (i32.const 18) (local.get $o)))
        (local.set $end (call $word (local.get $m) (local.get $end) (local.get $len)))
        (local.set $end (call $put (i32.const 0x300a0) (i32.const 2) (local.get $end)))
        (call $log (local.get $o) (i32.sub (local.get $end) (local.get $o)))
        (return (i32.const 0))))))
    (if (i32.ge_s (call $find (i32.const 0x30020) (i32.const 16) (i32.const 0) (local.get $len)) (i32.const 0))
      (then (if (i32.and (i32.ge_s (local.get $m) (i32.const 0)) (i32.ge_s (local.get $r) (i32.const 0))) (then
        (local.set $end (call $put (i32.const 0x300c0) (i32.const 26) (local.get $o)))
        (local.set $end (call $word (local.get $r) (local.get $end) (local.get $len)))
        (local.set $end (call $put (i32.const 0x300e0) (i32.const 11) (local.get $end)))
        (local.set $end (call $word (local.get $m) (local.get $end) (local.get $len)))
        (local.set $end (call $put (i32.const 0x30100) (i32.const 4) (local.get $end)))
        (call $log (local.get $o) (i32.sub (local.get $end) (local.get $o)))
        (return (i32.const 0))))))
    (call $log (i32.const 0x30120) (i32.const 7))
    (i32.const 0)))
