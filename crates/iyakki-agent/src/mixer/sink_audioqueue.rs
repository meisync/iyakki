//! The mixer's sound output on macOS: an AudioToolbox *AudioQueue* on the system's default output device, through a few
//! hand-written `extern "C"` declarations (system frameworks only, no crates). [`Pool`] (the free-buffer bookkeeping shared
//! with the queue's callback thread) is plain Rust and tested everywhere; the FFI part is `cfg(target_os = "macos")`.
//!
//! How it works: `open` makes a queue for 16-bit interleaved PCM at the source's rate and channel count (the system converts
//! to whatever the speakers need) and allocates `buffer_ms / period_ms` buffers of one period each. `write` copies PCM into a
//! free buffer (waiting for the callback to hand one back when all are queued: that wait is the back-pressure the mixer's
//! clock relies on) and enqueues it. The queue is started with the first buffer, so nothing plays before there is sound.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// The buffers an output queue owns, by id: which are free to fill, how many frames came back played.
pub struct Pool {
    state: Mutex<PoolState>,
    cv: Condvar,
}

struct PoolState {
    free: Vec<usize>,
    total: usize,
    frames_played: u64,
}

impl Pool {
    pub fn new(ids: impl IntoIterator<Item = usize>) -> Pool {
        let free: Vec<usize> = ids.into_iter().collect();
        let total = free.len();
        Pool { state: Mutex::new(PoolState { free, total, frames_played: 0 }), cv: Condvar::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A free buffer, waiting up to `wait` for the device to finish one.
    pub fn take(&self, wait: Duration) -> Option<usize> {
        let mut g = self.lock();
        let end = std::time::Instant::now() + wait;
        loop {
            if let Some(id) = g.free.pop() {
                return Some(id);
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// The device finished with buffer `id` that held `frames` frames (called from the queue's thread).
    pub fn give_back(&self, id: usize, frames: u64) {
        let mut g = self.lock();
        g.free.push(id);
        g.frames_played += frames;
        drop(g);
        self.cv.notify_all();
    }

    /// Every buffer is back: the device has played all that was queued.
    pub fn all_free(&self) -> bool {
        let g = self.lock();
        g.free.len() == g.total
    }

    pub fn frames_played(&self) -> u64 {
        self.lock().frames_played
    }
}

#[cfg(target_os = "macos")]
pub use mac::{consumed_frames, AudioQueueSink};

#[cfg(target_os = "macos")]
mod mac {
    use super::Pool;
    use crate::mixer::sink::Sink;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    type AudioQueueRef = *mut c_void;

    #[repr(C)]
    struct StreamDescription {
        sample_rate: f64,
        format_id: u32,
        format_flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels_per_frame: u32,
        bits_per_channel: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct QueueBuffer {
        capacity: u32,
        data: *mut c_void,
        byte_size: u32,
        user_data: *mut c_void,
        packet_capacity: u32,
        packet_descriptions: *mut c_void,
        packet_count: u32,
    }

    type OutputCallback = extern "C" fn(user: *mut c_void, queue: AudioQueueRef, buffer: *mut QueueBuffer);

    #[link(name = "AudioToolbox", kind = "framework")]
    extern "C" {
        fn AudioQueueNewOutput(format: *const StreamDescription, callback: OutputCallback, user: *mut c_void, run_loop: *mut c_void, run_loop_mode: *const c_void, flags: u32, out: *mut AudioQueueRef) -> i32;
        fn AudioQueueAllocateBuffer(queue: AudioQueueRef, bytes: u32, out: *mut *mut QueueBuffer) -> i32;
        fn AudioQueueEnqueueBuffer(queue: AudioQueueRef, buffer: *mut QueueBuffer, descs: u32, descriptions: *const c_void) -> i32;
        fn AudioQueueStart(queue: AudioQueueRef, start_time: *const c_void) -> i32;
        fn AudioQueueStop(queue: AudioQueueRef, immediate: u8) -> i32;
        fn AudioQueueDispose(queue: AudioQueueRef, immediate: u8) -> i32;
    }

    const LINEAR_PCM: u32 = 0x6C70_636D; // 'lpcm'
    const SIGNED_PACKED: u32 = 4 | 8; // kLinearPCMFormatFlagIsSignedInteger | IsPacked (little-endian on every Mac)

    /// Every frame the system has played through any queue of this program (for the doctor and for checks on a real Mac).
    static CONSUMED: AtomicU64 = AtomicU64::new(0);

    pub fn consumed_frames() -> u64 {
        CONSUMED.load(Ordering::Relaxed)
    }

    /// What the queue's callback needs: the pool and the bytes per frame (to count frames).
    struct Shared {
        pool: Pool,
        frame_bytes: u32,
    }

    extern "C" fn played(user: *mut c_void, _queue: AudioQueueRef, buffer: *mut QueueBuffer) {
        // SAFETY: `user` is the `Arc<Shared>` pointer given to AudioQueueNewOutput; the sink keeps it alive until after the
        // queue is disposed. `buffer` is one of the buffers allocated on this queue; `user_data` holds its id.
        unsafe {
            let shared = &*(user as *const Shared);
            let b = &*buffer;
            let frames = (b.byte_size / shared.frame_bytes.max(1)) as u64;
            CONSUMED.fetch_add(frames, Ordering::Relaxed);
            shared.pool.give_back(b.user_data as usize, frames);
        }
    }

    struct Open {
        queue: AudioQueueRef,
        buffers: Vec<*mut QueueBuffer>,
        shared: *const Shared,
        started: bool,
        rate: u32,
        channels: usize,
        buffer_ms: u32,
        period_ms: u32,
        low: Option<(u32, u32)>,
    }

    /// The default output of the Mac, through an AudioQueue.
    pub struct AudioQueueSink {
        open: Option<Open>,
        want_low: Option<(u32, u32)>,
    }

    // SAFETY: the raw pointers are only used by the thread that owns the sink (the mixer's), and by the queue's own callback
    // thread through the `Shared` they point to (which is `Sync`: a mutex and a condition variable).
    unsafe impl Send for AudioQueueSink {}

    const DEFAULT_PROFILE: (u32, u32) = (400, 50);

    impl AudioQueueSink {
        pub fn new() -> AudioQueueSink {
            AudioQueueSink { open: None, want_low: None }
        }

        fn dispose(o: Open, immediate: bool) {
            // SAFETY: the queue was made by `open`; stopping and disposing it ends the callbacks, after which the `Shared`
            // (leaked from an Arc in `open`) is freed.
            unsafe {
                AudioQueueStop(o.queue, immediate as u8);
                AudioQueueDispose(o.queue, 1);
                drop(Arc::from_raw(o.shared));
            }
        }
    }

    impl Default for AudioQueueSink {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Sink for AudioQueueSink {
        fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
            if self.open.is_some() {
                self.close();
            }
            let (buffer_ms, period_ms) = self.want_low.unwrap_or(DEFAULT_PROFILE);
            let period_ms = period_ms.max(5);
            let count = (buffer_ms / period_ms).clamp(2, 32) as usize;
            let frame_bytes = (channels * 2) as u32;
            let period_bytes = ((rate as u64 * period_ms as u64 / 1000) as u32).max(64) * frame_bytes;
            let fmt = StreamDescription { sample_rate: rate as f64, format_id: LINEAR_PCM, format_flags: SIGNED_PACKED, bytes_per_packet: frame_bytes, frames_per_packet: 1, bytes_per_frame: frame_bytes, channels_per_frame: channels as u32, bits_per_channel: 16, reserved: 0 };
            let shared = Arc::into_raw(Arc::new(Shared { pool: Pool::new(0..count), frame_bytes }));
            let mut queue: AudioQueueRef = std::ptr::null_mut();
            // SAFETY: `fmt` is valid for the call; the callback and `shared` outlive the queue (freed in `dispose`); a null run
            // loop makes the queue call back on its own thread.
            let rc = unsafe { AudioQueueNewOutput(&fmt, played, shared as *mut c_void, std::ptr::null_mut(), std::ptr::null(), 0, &mut queue) };
            if rc != 0 || queue.is_null() {
                // SAFETY: undo the `into_raw` above; nothing else holds it.
                unsafe { drop(Arc::from_raw(shared)) };
                return Err(format!("the system sound output could not be opened (AudioQueue error {rc})"));
            }
            let mut buffers = Vec::with_capacity(count);
            for id in 0..count {
                let mut b: *mut QueueBuffer = std::ptr::null_mut();
                // SAFETY: the queue is valid; `b` receives a buffer owned by the queue.
                let rc = unsafe { AudioQueueAllocateBuffer(queue, period_bytes, &mut b) };
                if rc != 0 || b.is_null() {
                    let o = Open { queue, buffers, shared, started: false, rate, channels, buffer_ms, period_ms, low: self.want_low };
                    Self::dispose(o, true);
                    return Err(format!("the system sound output could not get a buffer (AudioQueue error {rc})"));
                }
                // SAFETY: `b` is a valid buffer; the id travels in `user_data` to the callback.
                unsafe { (*b).user_data = id as *mut c_void };
                buffers.push(b);
            }
            self.open = Some(Open { queue, buffers, shared, started: false, rate, channels, buffer_ms: period_ms * count as u32, period_ms, low: self.want_low });
            let _ = buffer_ms;
            Ok(())
        }

        fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
            let Some(o) = self.open.as_mut() else { return Err("the sound device is not open".into()) };
            // SAFETY: `shared` is alive while the sink is open.
            let shared = unsafe { &*o.shared };
            let per_chunk = {
                // SAFETY: buffer 0 is valid while open.
                let cap = unsafe { (*o.buffers[0]).capacity } as usize / 2;
                cap - cap % o.channels.max(1)
            };
            for chunk in pcm.chunks(per_chunk.max(o.channels.max(1))) {
                let Some(id) = shared.pool.take(Duration::from_secs(3)) else { return Err("the system sound output stopped taking sound".into()) };
                let b = o.buffers[id];
                // SAFETY: `b` is a valid buffer of capacity >= chunk bytes, owned by us until enqueued; the copy stays inside it.
                let rc = unsafe {
                    std::ptr::copy_nonoverlapping(chunk.as_ptr() as *const u8, (*b).data as *mut u8, chunk.len() * 2);
                    (*b).byte_size = (chunk.len() * 2) as u32;
                    AudioQueueEnqueueBuffer(o.queue, b, 0, std::ptr::null())
                };
                if rc != 0 {
                    shared.pool.give_back(id, 0);
                    return Err(format!("the system sound output refused sound (AudioQueue error {rc})"));
                }
                if !o.started {
                    // SAFETY: the queue is valid; a null start time means "now".
                    let rc = unsafe { AudioQueueStart(o.queue, std::ptr::null()) };
                    if rc != 0 {
                        return Err(format!("the system sound output could not start (AudioQueue error {rc})"));
                    }
                    o.started = true;
                }
            }
            Ok(())
        }

        fn close(&mut self) {
            if let Some(o) = self.open.take() {
                // let what is queued play out (at most about 600 ms), then stop
                // SAFETY: `shared` is alive until `dispose`.
                let shared = unsafe { &*o.shared };
                let end = std::time::Instant::now() + Duration::from_millis(600);
                while !shared.pool.all_free() && std::time::Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Self::dispose(o, true);
            }
        }

        fn is_open(&self) -> bool {
            self.open.is_some()
        }

        fn name(&self) -> String {
            "the system default output (AudioQueue)".into()
        }

        fn detail(&self) -> String {
            match &self.open {
                // SAFETY: `shared` is alive while open.
                Some(o) => format!("AudioQueue {} Hz x {}, {} buffers of {} ms, {} frames played so far", o.rate, o.channels, o.buffers.len(), o.period_ms, unsafe { &*o.shared }.pool.frames_played()),
                None => String::new(),
            }
        }

        fn latency_ms(&self) -> u32 {
            self.open.as_ref().map_or(0, |o| o.buffer_ms)
        }

        fn set_low_latency(&mut self, profile: Option<(u32, u32)>) {
            self.want_low = profile;
        }

        fn low_latency(&self) -> Option<(u32, u32)> {
            self.open.as_ref().and_then(|o| o.low)
        }

        fn abort(&mut self) {
            if let Some(o) = self.open.take() {
                Self::dispose(o, true);
            }
        }
    }

    impl Drop for AudioQueueSink {
        fn drop(&mut self) {
            self.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_taken_buffer_comes_back_through_the_callback_and_counts_its_frames() {
        let p = Arc::new(Pool::new(0..2));
        let (a, b) = (p.take(Duration::ZERO).unwrap(), p.take(Duration::ZERO).unwrap());
        assert_ne!(a, b);
        assert!(p.take(Duration::from_millis(20)).is_none(), "all queued: the writer waits, then gives up");
        assert!(!p.all_free());
        let p2 = p.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            p2.give_back(a, 2400);
        });
        assert_eq!(p.take(Duration::from_secs(2)), Some(a), "a waiting writer wakes when the device finishes one");
        t.join().unwrap();
        p.give_back(b, 2400);
        p.give_back(a, 100);
        assert!(p.all_free());
        assert_eq!(p.frames_played(), 4900);
    }

    /// On a Mac with an output device: queue 1 s of silence and see the system consume it. Run by hand (it needs the audio
    /// service): `cargo test -p iyakki-agent --lib audioqueue -- --ignored`. Silent, so nobody is disturbed.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a Mac's audio service"]
    fn audioqueue_plays_silence_and_the_system_consumes_it() {
        use crate::mixer::sink::Sink;
        let mut s = super::AudioQueueSink::new();
        s.open(48_000, 2).expect("open");
        let before = super::consumed_frames();
        for _ in 0..20 {
            s.write(&vec![0i16; 4800 * 2]).expect("write"); // 20 x 100 ms
        }
        s.close();
        let got = super::consumed_frames() - before;
        eprintln!("audioqueue consumed {got} frames");
        assert!(got >= 48_000, "{got}");
    }
}
