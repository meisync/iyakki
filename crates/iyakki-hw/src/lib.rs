//! Hardware bridges: microphone PCM and camera frames piped into WASM guests.
//!
//! Sources are traits so guests can be developed and tested without devices
//! ([`SineSource`], [`TestPattern`]). Real devices sit behind cargo features:
//! `audio` (cpal) and `video` (V4L2).
//!
//! # Guest ABI for sensor modules
//! The guest exports `memory`, `iyakki_buf_ptr() -> i32`, `iyakki_buf_cap() -> i32`
//! (buffer size in bytes) and `process(len: i32) -> i32`. The host copies a chunk
//! into the buffer and calls `process` with its byte length. Data crosses into
//! guest memory with exactly one copy; host-side frames are shared `Arc`s.

use std::sync::Arc;

use iyakki_wasm::{Session, WasmError};

#[derive(Debug, thiserror::Error)]
pub enum HwError {
    #[error(transparent)]
    Wasm(#[from] WasmError),
    #[error("guest buffer has zero capacity or bad pointer")]
    BadBuffer,
    #[error("device error: {0}")]
    Device(String),
}

pub trait AudioSource {
    fn sample_rate(&self) -> u32;
    /// Next `samples` mono 16-bit samples, or `None` at end of stream.
    fn next_chunk(&mut self, samples: usize) -> Option<Vec<i16>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb8,
    Yuyv,
    Gray8,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Arc<[u8]>,
}

pub trait FrameSource {
    fn next_frame(&mut self) -> Option<Frame>;
}

/// Deterministic sine tone, for tests and demos.
pub struct SineSource {
    pub rate: u32,
    pub freq: f32,
    pub amplitude: i16,
    pub remaining: usize,
    n: u64,
}

impl SineSource {
    pub fn new(rate: u32, freq: f32, amplitude: i16, total_samples: usize) -> Self {
        Self { rate, freq, amplitude, remaining: total_samples, n: 0 }
    }
}

impl AudioSource for SineSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn next_chunk(&mut self, samples: usize) -> Option<Vec<i16>> {
        if self.remaining == 0 {
            return None;
        }
        let k = samples.min(self.remaining);
        self.remaining -= k;
        Some(
            (0..k)
                .map(|_| {
                    let t = self.n as f32 / self.rate as f32;
                    self.n += 1;
                    ((t * self.freq * std::f32::consts::TAU).sin() * self.amplitude as f32) as i16
                })
                .collect(),
        )
    }
}

/// Horizontal gradient gray frames; `count` frames then end.
pub struct TestPattern {
    pub width: u32,
    pub height: u32,
    pub count: usize,
}

impl FrameSource for TestPattern {
    fn next_frame(&mut self) -> Option<Frame> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        let mut d = Vec::with_capacity((self.width * self.height) as usize);
        for _ in 0..self.height {
            for x in 0..self.width {
                d.push((x * 255 / self.width.max(1)) as u8);
            }
        }
        Some(Frame { width: self.width, height: self.height, format: PixelFormat::Gray8, data: d.into() })
    }
}

pub fn pcm_to_bytes(s: &[i16]) -> Vec<u8> {
    s.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Copy `data` into the guest's sensor buffer in capacity-sized chunks, calling
/// `process(len)` for each. Returns each call's result (0 if the guest returns nothing).
pub fn feed_guest(session: &mut Session, data: &[u8]) -> Result<Vec<i32>, HwError> {
    let ptr = session.call("iyakki_buf_ptr", &[])?.result.ok_or(HwError::BadBuffer)?;
    let cap = session.call("iyakki_buf_cap", &[])?.result.ok_or(HwError::BadBuffer)?;
    if ptr < 0 || cap <= 0 {
        return Err(HwError::BadBuffer);
    }
    let mut out = Vec::new();
    for chunk in data.chunks(cap as usize) {
        session.write_memory(ptr as u32, chunk).map_err(|_| HwError::BadBuffer)?;
        out.push(session.call("process", &[chunk.len() as i32])?.result.unwrap_or(0));
    }
    Ok(out)
}

/// Pump an audio source into a guest until it ends; returns per-chunk results.
pub fn pump_audio(session: &mut Session, src: &mut dyn AudioSource, chunk_samples: usize) -> Result<Vec<i32>, HwError> {
    let mut all = Vec::new();
    while let Some(c) = src.next_chunk(chunk_samples) {
        all.extend(feed_guest(session, &pcm_to_bytes(&c))?);
    }
    Ok(all)
}

/// Pump a frame source into a guest until it ends.
pub fn pump_video(session: &mut Session, src: &mut dyn FrameSource) -> Result<Vec<i32>, HwError> {
    let mut all = Vec::new();
    while let Some(f) = src.next_frame() {
        all.extend(feed_guest(session, &f.data)?);
    }
    Ok(all)
}

#[cfg(feature = "audio")]
pub mod mic {
    //! Live microphone via cpal (ALSA / PipeWire-ALSA / CoreAudio / WASAPI).
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::mpsc::{channel, Receiver};

    pub struct Mic {
        _stream: cpal::Stream, // dropping stops capture
        rx: Receiver<Vec<i16>>,
        rate: u32,
    }

    impl Mic {
        pub fn open_default() -> Result<Self, HwError> {
            let dev = cpal::default_host()
                .default_input_device()
                .ok_or_else(|| HwError::Device("no input device".into()))?;
            let cfg = dev.default_input_config().map_err(|e| HwError::Device(e.to_string()))?;
            let (rate, ch, fmt) = (cfg.sample_rate().0, cfg.channels() as usize, cfg.sample_format());
            let (tx, rx) = channel();
            let err = |e| eprintln!("[iyakki mic] stream error: {e}");
            // Downmix to mono i16.
            let stream = match fmt {
                cpal::SampleFormat::F32 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[f32], _| {
                        let _ = tx.send(d.chunks(ch).map(|f| (f.iter().sum::<f32>() / ch as f32 * 32767.0) as i16).collect());
                    },
                    err,
                    None,
                ),
                cpal::SampleFormat::I16 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i16], _| {
                        let _ = tx.send(d.chunks(ch).map(|f| (f.iter().map(|&x| x as i32).sum::<i32>() / ch as i32) as i16).collect());
                    },
                    err,
                    None,
                ),
                other => return Err(HwError::Device(format!("unsupported sample format {other:?}"))),
            }
            .map_err(|e| HwError::Device(e.to_string()))?;
            stream.play().map_err(|e| HwError::Device(e.to_string()))?;
            Ok(Self { _stream: stream, rx, rate })
        }
    }

    impl AudioSource for Mic {
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        /// Blocks until a buffer arrives; returns whatever the device delivered (may differ from `samples`).
        fn next_chunk(&mut self, _samples: usize) -> Option<Vec<i16>> {
            self.rx.recv().ok()
        }
    }
}

#[cfg(feature = "video")]
pub mod camera {
    //! V4L2 camera capture (Linux), memory-mapped kernel buffers.
    use super::*;
    use v4l::buffer::Type;
    use v4l::io::mmap::Stream;
    use v4l::io::traits::CaptureStream;
    use v4l::video::Capture;
    use v4l::{Device, FourCC};

    /// Capture a single YUYV frame from `/dev/video{index}`.
    pub fn capture_one(index: usize, width: u32, height: u32) -> Result<Frame, HwError> {
        let e = |e: std::io::Error| HwError::Device(e.to_string());
        let mut dev = Device::new(index).map_err(e)?;
        let mut fmt = dev.format().map_err(e)?;
        fmt.width = width;
        fmt.height = height;
        fmt.fourcc = FourCC::new(b"YUYV");
        let fmt = dev.set_format(&fmt).map_err(e)?;
        let mut stream = Stream::with_buffers(&mut dev, Type::VideoCapture, 4).map_err(e)?;
        let (buf, _meta) = stream.next().map_err(e)?;
        Ok(Frame { width: fmt.width, height: fmt.height, format: PixelFormat::Yuyv, data: Arc::from(buf) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::CapabilityStore;
    use iyakki_vfs::MemFs;
    use iyakki_wasm::{Sandbox, SandboxConfig};
    use std::sync::Mutex;

    fn session(wat: &str) -> Session {
        Sandbox::new(SandboxConfig::default())
            .unwrap()
            .session(wat.as_bytes(), Arc::new(Mutex::new(MemFs::new())), CapabilityStore::new(), vec![])
            .unwrap()
    }

    /// Guest: 256-byte buffer at 1024; process(len) returns max |sample| of i16 PCM.
    const PEAK: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "iyakki_buf_ptr") (result i32) i32.const 1024)
      (func (export "iyakki_buf_cap") (result i32) i32.const 256)
      (func (export "process") (param $len i32) (result i32)
        (local $i i32) (local $v i32) (local $max i32)
        (loop $l
          (local.set $v (i32.load16_s (i32.add (i32.const 1024) (local.get $i))))
          (if (i32.lt_s (local.get $v) (i32.const 0)) (then (local.set $v (i32.sub (i32.const 0) (local.get $v)))))
          (if (i32.gt_s (local.get $v) (local.get $max)) (then (local.set $max (local.get $v))))
          (local.set $i (i32.add (local.get $i) (i32.const 2)))
          (br_if $l (i32.lt_u (local.get $i) (local.get $len))))
        (local.get $max)))
    "#;

    #[test]
    fn audio_reaches_guest_in_chunks() {
        let mut s = session(PEAK);
        // 1000 samples = 2000 bytes -> 8 chunks of <=256 bytes
        let mut src = SineSource::new(8000, 440.0, 1000, 1000);
        let peaks = pump_audio(&mut s, &mut src, 1000).unwrap();
        assert_eq!(peaks.len(), 8);
        assert!(peaks.iter().all(|p| (900..=1000).contains(p)), "{peaks:?}");
        // silence -> 0
        let z = feed_guest(&mut s, &[0u8; 64]).unwrap();
        assert_eq!(z, vec![0]);
    }

    #[test]
    fn video_frames_reach_guest() {
        let mut s = session(PEAK);
        let mut src = TestPattern { width: 16, height: 4, count: 2 };
        let r = pump_video(&mut s, &mut src).unwrap();
        assert_eq!(r.len(), 2); // 64-byte frames, one chunk each
    }

    #[test]
    fn bad_guest_abi_is_reported() {
        let mut s = session(r#"(module (memory (export "memory") 1) (func (export "iyakki_buf_ptr") (result i32) i32.const 0) (func (export "iyakki_buf_cap") (result i32) i32.const 0) (func (export "process") (param i32) (result i32) i32.const 0))"#);
        assert!(matches!(feed_guest(&mut s, b"x"), Err(HwError::BadBuffer)));
        let mut s = session(r#"(module (memory (export "memory") 1))"#);
        assert!(feed_guest(&mut s, b"x").is_err());
    }

    #[test]
    fn test_pattern_is_gradient() {
        let f = TestPattern { width: 4, height: 1, count: 1 }.next_frame().unwrap();
        assert_eq!(&*f.data, &[0, 63, 127, 191]);
    }
}
