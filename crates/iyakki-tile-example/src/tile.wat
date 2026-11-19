;; Example tile (ABI 1): a counter with a button and a text field, written by hand in WAT so it
;; builds with no toolchain at all. Events: a Hit on region 1 counts up, a Hit on region 2 clears
;; the field, typed ASCII text goes into the field, Backspace deletes, Resize sets the background.
;; Memory map: 1024 constants | 2048 field text (32 bytes) | 3000 number scratch | 4096.. input heap
;;             | 32768 frame buffer: u32 length, then the display list.
(module
  (import "iyakki_tile" "measure_text" (func $measure (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "TILE DEMO")   ;; 9 bytes
  (data (i32.const 1040) "+1")          ;; 2
  (data (i32.const 1048) "COUNT")       ;; 5
  (global $cnt (mut i32) (i32.const 0))
  (global $tlen (mut i32) (i32.const 0))
  (global $w (mut i32) (i32.const 320))
  (global $h (mut i32) (i32.const 200))
  (global $heap (mut i32) (i32.const 4096))
  (global $cur (mut i32) (i32.const 0))
  (global $rd (mut i32) (i32.const 0))

  (func (export "iyakki_tile_abi") (result i32) (i32.const 1))

  (func (export "iyakki_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))

  ;; ---- reading events
  (func $rb (result i32)
    (local $b i32)
    (local.set $b (i32.load8_u (global.get $rd)))
    (global.set $rd (i32.add (global.get $rd) (i32.const 1)))
    (local.get $b))
  (func $rv (result i32)
    (local $v i32) (local $s i32) (local $b i32)
    (loop $l
      (local.set $b (call $rb))
      (local.set $v (i32.or (local.get $v) (i32.shl (i32.and (local.get $b) (i32.const 127)) (local.get $s))))
      (local.set $s (i32.add (local.get $s) (i32.const 7)))
      (br_if $l (i32.and (i32.ne (i32.and (local.get $b) (i32.const 128)) (i32.const 0))
                         (i32.lt_u (local.get $s) (i32.const 35)))))
    (local.get $v))

  ;; parse a batch at $ptr; returns 1 when the picture changed
  (func $events (param $ptr i32) (result i32)
    (local $n i32) (local $tag i32) (local $a i32) (local $l i32) (local $i i32) (local $ch i32) (local $dirty i32)
    (global.set $rd (i32.add (local.get $ptr) (i32.const 6)))
    (local.set $n (call $rv))
    (block $done (loop $next
      (br_if $done (i32.eqz (local.get $n)))
      (local.set $n (i32.sub (local.get $n) (i32.const 1)))
      (local.set $tag (call $rb))
      (if (i32.or (i32.eq (local.get $tag) (i32.const 1)) (i32.eq (local.get $tag) (i32.const 2)))
        (then (drop (call $rv)) (drop (call $rv)) (drop (call $rb))))
      (if (i32.eq (local.get $tag) (i32.const 3))
        (then (drop (call $rv)) (drop (call $rv))))
      (if (i32.eq (local.get $tag) (i32.const 4))
        (then
          (local.set $a (call $rv))
          (drop (call $rb))
          (if (i32.and (i32.eq (call $rb) (i32.const 1))
                       (i32.and (i32.eq (local.get $a) (i32.const 8)) (i32.gt_u (global.get $tlen) (i32.const 0))))
            (then (global.set $tlen (i32.sub (global.get $tlen) (i32.const 1))) (local.set $dirty (i32.const 1))))))
      (if (i32.eq (local.get $tag) (i32.const 5))
        (then
          (local.set $l (call $rv))
          (local.set $i (i32.const 0))
          (block $e (loop $c
            (br_if $e (i32.ge_u (local.get $i) (local.get $l)))
            (local.set $ch (call $rb))
            (if (i32.and (i32.lt_u (global.get $tlen) (i32.const 32))
                         (i32.and (i32.ge_u (local.get $ch) (i32.const 32)) (i32.lt_u (local.get $ch) (i32.const 127))))
              (then (i32.store8 (i32.add (i32.const 2048) (global.get $tlen)) (local.get $ch))
                    (global.set $tlen (i32.add (global.get $tlen) (i32.const 1)))
                    (local.set $dirty (i32.const 1))))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $c)))))
      (if (i32.eq (local.get $tag) (i32.const 6)) (then (drop (call $rv))))
      (if (i32.eq (local.get $tag) (i32.const 7))
        (then (global.set $w (call $rv)) (global.set $h (call $rv)) (drop (call $rv)) (local.set $dirty (i32.const 1))))
      (if (i32.eq (local.get $tag) (i32.const 8))
        (then (global.set $rd (i32.add (global.get $rd) (i32.const 13)))))
      (if (i32.eq (local.get $tag) (i32.const 9))
        (then
          (local.set $a (call $rv))
          (if (i32.eq (local.get $a) (i32.const 1))
            (then (global.set $cnt (i32.add (global.get $cnt) (i32.const 1)))))
          (if (i32.eq (local.get $a) (i32.const 2)) (then (global.set $tlen (i32.const 0))))
          (local.set $dirty (i32.const 1))))
      (br $next)))
    (local.get $dirty))

  (func (export "iyakki_tile_init") (param $ver i32) (param $ptr i32) (param $len i32) (result i32)
    (if (i32.ne (local.get $ver) (i32.const 1)) (then (return (i32.const 1))))
    (drop (call $events (local.get $ptr)))
    (global.set $heap (i32.const 4096))
    (i32.const 0))

  (func (export "iyakki_tile_event") (param $ptr i32) (param $len i32) (result i32)
    (local $d i32)
    (local.set $d (call $events (local.get $ptr)))
    (global.set $heap (i32.const 4096))
    (local.get $d))

  ;; ---- writing the display list
  (func $wb (param $v i32)
    (i32.store8 (global.get $cur) (local.get $v))
    (global.set $cur (i32.add (global.get $cur) (i32.const 1))))
  (func $wv (param $v i32)
    (loop $l
      (if (i32.ge_u (local.get $v) (i32.const 128))
        (then (call $wb (i32.or (i32.and (local.get $v) (i32.const 127)) (i32.const 128)))
              (local.set $v (i32.shr_u (local.get $v) (i32.const 7)))
              (br $l))))
    (call $wb (local.get $v)))
  (func $wz (param $v i32)
    (call $wv (i32.xor (i32.shl (local.get $v) (i32.const 1)) (i32.shr_s (local.get $v) (i32.const 31)))))
  (func $wcol (param $r i32) (param $g i32) (param $b i32)
    (call $wb (local.get $r)) (call $wb (local.get $g)) (call $wb (local.get $b)) (call $wb (i32.const 255)))
  (func $wrect (param $x i32) (param $y i32) (param $w i32) (param $h i32)
    (call $wz (local.get $x)) (call $wz (local.get $y)) (call $wv (local.get $w)) (call $wv (local.get $h)))
  (func $wtext (param $x i32) (param $y i32) (param $size i32) (param $r i32) (param $g i32) (param $b i32)
               (param $ptr i32) (param $len i32)
    (local $i i32)
    (call $wb (i32.const 5))
    (call $wz (local.get $x)) (call $wz (local.get $y))
    (call $wb (i32.const 0))
    (call $wv (local.get $size))
    (call $wcol (local.get $r) (local.get $g) (local.get $b))
    (call $wv (local.get $len))
    (block $e (loop $c
      (br_if $e (i32.ge_u (local.get $i) (local.get $len)))
      (call $wb (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $c))))

  (func (export "iyakki_tile_frame") (result i32)
    (local $n i32) (local $pos i32) (local $lw i32)
    (global.set $cur (i32.const 32772))
    (call $wb (i32.const 73)) (call $wb (i32.const 89)) (call $wb (i32.const 68)) (call $wb (i32.const 76)) ;; IYDL
    (call $wb (i32.const 1)) (call $wb (i32.const 0))
    (call $wv (i32.const 10))
    ;; 1 background
    (call $wb (i32.const 1)) (call $wrect (i32.const 0) (i32.const 0) (global.get $w) (global.get $h))
    (call $wcol (i32.const 245) (i32.const 245) (i32.const 245))
    ;; 2 title
    (call $wtext (i32.const 20) (i32.const 16) (i32.const 16) (i32.const 30) (i32.const 30) (i32.const 30)
                 (i32.const 1024) (i32.const 9))
    ;; 3 "COUNT" label, 4 the number
    (call $wtext (i32.const 20) (i32.const 44) (i32.const 14) (i32.const 60) (i32.const 60) (i32.const 60)
                 (i32.const 1048) (i32.const 5))
    (local.set $pos (i32.const 3120))
    (local.set $n (global.get $cnt))
    (loop $d
      (local.set $pos (i32.sub (local.get $pos) (i32.const 1)))
      (i32.store8 (local.get $pos) (i32.add (i32.const 48) (i32.rem_u (local.get $n) (i32.const 10))))
      (local.set $n (i32.div_u (local.get $n) (i32.const 10)))
      (br_if $d (local.get $n)))
    (call $wtext (i32.const 92) (i32.const 44) (i32.const 14) (i32.const 40) (i32.const 110) (i32.const 220)
                 (local.get $pos) (i32.sub (i32.const 3120) (local.get $pos)))
    ;; 5 button, 6 its label centred with the host's own measurement
    (call $wb (i32.const 2)) (call $wrect (i32.const 20) (i32.const 70) (i32.const 100) (i32.const 36))
    (call $wv (i32.const 8)) (call $wcol (i32.const 40) (i32.const 110) (i32.const 220))
    (local.set $lw (call $measure (i32.const 0) (i32.const 18) (i32.const 1040) (i32.const 2)))
    (call $wtext (i32.add (i32.const 20) (i32.div_s (i32.sub (i32.const 100) (local.get $lw)) (i32.const 2)))
                 (i32.const 78) (i32.const 18) (i32.const 255) (i32.const 255) (i32.const 255)
                 (i32.const 1040) (i32.const 2))
    ;; 7 hit region 1
    (call $wb (i32.const 9)) (call $wv (i32.const 1))
    (call $wrect (i32.const 20) (i32.const 70) (i32.const 100) (i32.const 36))
    ;; 8 field outline, 9 field text, 10 hit region 2
    (call $wb (i32.const 3)) (call $wrect (i32.const 20) (i32.const 120) (i32.const 200) (i32.const 32))
    (call $wv (i32.const 1)) (call $wcol (i32.const 120) (i32.const 120) (i32.const 120))
    (call $wtext (i32.const 28) (i32.const 128) (i32.const 16) (i32.const 20) (i32.const 20) (i32.const 20)
                 (i32.const 2048) (global.get $tlen))
    (call $wb (i32.const 9)) (call $wv (i32.const 2))
    (call $wrect (i32.const 20) (i32.const 120) (i32.const 200) (i32.const 32))
    (i32.store (i32.const 32768) (i32.sub (global.get $cur) (i32.const 32772)))
    (i32.const 32768))
)
