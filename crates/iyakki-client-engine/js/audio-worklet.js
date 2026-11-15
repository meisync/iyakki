// Audio worklet of the IYAKKI client page (EXPERIMENTAL): the player for the machine's sound and the capture for the client's own microphone.
// Hand-written, no libraries. It runs in the audio thread and talks to the page only through its message port.
//
// "iyakki-out": plays 16-bit PCM chunks (48 kHz, 20 ms, stereo, numbered) that the page posts as { t: "pcm", seq, data: Float32Array interleaved }.
//   * The queue holds at most MAX_QUEUE_MS: when more arrives the OLDEST sound is dropped (late sound is worse than a gap).
//   * Playing starts after PREFILL_MS is queued and stops (silence) when the queue runs dry; a gap in the sequence numbers is not filled in.
//   * { t: "flush" } empties the queue at once (the sound stopped or paused on the machine, or the person switched side).
// "iyakki-mic": captures the first channel of the client's microphone, makes 16 kHz 16-bit mono chunks of 40 ms and posts them as { t: "mic", data: Int16Array }.

export const RATE = 48000;
export const MAX_QUEUE_MS = 100;
export const PREFILL_MS = 40;
export const MIC_RATE = 16000;
export const MIC_CHUNK = 640;

// The queue of the player, kept apart from the audio API so that it can be tested without a browser.
export class PlayQueue {
  constructor(outRate = RATE) {
    this.outRate = outRate;                       // the audio context's rate
    this.cap = Math.round(outRate * MAX_QUEUE_MS / 1000) * 2;   // samples (stereo)
    this.prefill = Math.round(outRate * PREFILL_MS / 1000) * 2;
    this.buf = new Float32Array(this.cap + 8192);
    this.len = 0; this.pos = 0;                   // queued samples are buf[pos .. pos+len)
    this.primed = false; this.expected = null;
    this.dropped = 0; this.lost = 0; this.underruns = 0;
    this.frac = 0;                                // resampling position between input frames
  }

  flush() { this.len = 0; this.pos = 0; this.primed = false; this.expected = null; this.frac = 0; }

  // interleaved stereo float samples at 48 kHz, resampled linearly when the context runs at another rate
  push(seq, data) {
    if (this.expected !== null && seq !== this.expected) this.lost += (seq - this.expected) >>> 0;
    this.expected = (seq + 1) >>> 0;
    let x = data;
    if (this.outRate !== RATE) x = resample(data, RATE, this.outRate);
    if (this.pos + this.len + x.length > this.buf.length) { this.buf.copyWithin(0, this.pos, this.pos + this.len); this.pos = 0; }
    if (x.length > this.buf.length - this.len) { x = x.subarray(x.length - (this.buf.length - this.len)); }
    this.buf.set(x, this.pos + this.len); this.len += x.length;
    if (this.len > this.cap) { const drop = this.len - this.cap; this.pos += drop; this.len -= drop; this.dropped += drop / 2; }
  }

  // fill `l` and `r` (one render quantum); returns the number of frames of real sound
  pull(l, r) {
    const n = l.length;
    if (!this.primed) { if (this.len >= this.prefill) this.primed = true; }
    if (!this.primed) { l.fill(0); r.fill(0); return 0; }
    const have = Math.min(n, this.len >> 1);
    for (let i = 0; i < have; i++) { l[i] = this.buf[this.pos + 2 * i]; r[i] = this.buf[this.pos + 2 * i + 1]; }
    this.pos += have * 2; this.len -= have * 2;
    if (have < n) { for (let i = have; i < n; i++) { l[i] = 0; r[i] = 0; } this.primed = false; this.underruns++; }
    return have;
  }

  queuedMs() { return Math.round(this.len / 2 * 1000 / this.outRate); }
}

export function resample(data, from, to) {
  const frames = data.length >> 1, outFrames = Math.floor(frames * to / from), out = new Float32Array(outFrames * 2);
  for (let i = 0; i < outFrames; i++) {
    const p = i * from / to, a = Math.min(frames - 1, Math.floor(p)), b = Math.min(frames - 1, a + 1), f = p - a;
    out[2 * i] = data[2 * a] * (1 - f) + data[2 * b] * f;
    out[2 * i + 1] = data[2 * a + 1] * (1 - f) + data[2 * b + 1] * f;
  }
  return out;
}

// capture: floats at the context's rate to 16 kHz 16-bit mono, in chunks
export class MicChunker {
  constructor(inRate) { this.step = inRate / MIC_RATE; this.t = 0; this.out = new Int16Array(MIC_CHUNK); this.n = 0; }
  push(samples, post) {
    for (; this.t < samples.length; this.t += this.step) {
      const a = Math.floor(this.t), b = Math.min(samples.length - 1, a + 1), f = this.t - a;
      const v = Math.max(-1, Math.min(1, samples[a] * (1 - f) + samples[b] * f));
      this.out[this.n++] = Math.round(v * 32767);
      if (this.n === MIC_CHUNK) { post(this.out.slice()); this.n = 0; }
    }
    this.t -= samples.length;
  }
}

if (typeof AudioWorkletProcessor !== "undefined") {
  registerProcessor("iyakki-out", class extends AudioWorkletProcessor {
    constructor() {
      super();
      this.q = new PlayQueue(sampleRate);
      this.ticks = 0;
      this.port.onmessage = (e) => {
        const m = e.data;
        if (m.t === "pcm") this.q.push(m.seq, m.data);
        else if (m.t === "flush") this.q.flush();
      };
    }
    process(_in, outputs) {
      const o = outputs[0];
      this.q.pull(o[0], o[1] || o[0]);
      if (++this.ticks % 375 === 0) this.port.postMessage({ t: "stats", queued: this.q.queuedMs(), dropped: this.q.dropped, lost: this.q.lost, underruns: this.q.underruns });
      return true;
    }
  });
  registerProcessor("iyakki-mic", class extends AudioWorkletProcessor {
    constructor() { super(); this.c = new MicChunker(sampleRate); this.on = true; this.port.onmessage = (e) => { if (e.data.t === "stop") this.on = false; }; }
    process(inputs) {
      const ch = inputs[0] && inputs[0][0];
      if (ch && this.on) this.c.push(ch, (d) => this.port.postMessage({ t: "mic", data: d }, [d.buffer]));
      return this.on;
    }
  });
}
