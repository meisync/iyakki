"""An in-house WAV listener for the sound checks of dist/autotest (no dependency: struct, array and math from the standard library).

It reads PCM WAV files (8, 16 or 24 bit; mono, stereo or more channels; any sample rate) and judges them:
  tone_present(wav, hz, tol_hz, min_dbfs, channel)   Goertzel over short windows: is a tone at HZ (within tol) there, and how loud
  level_dbfs(wav, channel)                           RMS and peak of a channel, in dBFS (full scale = 0 dBFS)
  silence(wav, threshold_dbfs)                       True when no window of the recording is louder than the threshold
  first_sound_at(wav, threshold_dbfs)                seconds until the first window above the threshold, or None
  sound_starts_within(wav, seconds)                  the first sound starts no later than SECONDS
  duration(wav)                                      seconds
  clipping(wav)                                      number of runs of samples at full scale (0 = none)
  left_right_balance(wav)                            right minus left RMS in dB (positive: right louder); None for mono
  speech_like(wav, min_seconds, max_seconds)         voice activity from the short-time energy envelope with a hangover.
                                                     It measures how long the energy comes and goes. It does NOT recognise words.

A recording being written by QEMU may still say data size 0 (or a size past the end of the file) in its header: the listener then reads
what is in the file. `load(path, start_frame)` skips the frames before a point (the scenario's start).
"""
import math
import struct
import sys
from array import array


class WavError(ValueError):
    """The file is not a PCM WAV this listener reads."""


class Wav:
    """Decoded samples: one float array per channel, scaled to -1.0 .. 1.0 (full scale)."""

    def __init__(self, rate, bits, chans):
        self.rate, self.bits, self.chans = rate, bits, chans

    @property
    def channels(self):
        return len(self.chans)

    @property
    def frames(self):
        return len(self.chans[0]) if self.chans else 0

    def pick(self, channel=None):
        """One channel's samples. None: the mix of all channels (the mean). 'left'/0 and 'right'/1 pick one channel."""
        if channel is None:
            if self.channels == 1:
                return self.chans[0]
            return array("f", [sum(t) / self.channels for t in zip(*self.chans)])
        idx = _channel_index(channel)
        if idx >= self.channels:
            raise ValueError("this recording has %d channel(s); %r does not exist" % (self.channels, channel))
        return self.chans[idx]

    def each(self, channel=None):
        """The channels to judge: all of them when channel is None, else the one picked."""
        return list(self.chans) if channel is None else [self.pick(channel)]


def _channel_index(channel):
    if isinstance(channel, int):
        return channel
    name = str(channel).lower()
    if name in ("left", "l", "0"):
        return 0
    if name in ("right", "r", "1"):
        return 1
    raise ValueError("channel must be left, right or a number, not %r" % (channel,))


def _parse(raw):
    """(chans, rate, bits, align, body, size) of the file's PCM data; raises WavError for anything else."""
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise WavError("not a RIFF WAVE file")
    fmt = data = None
    pos = 12
    while pos + 8 <= len(raw):
        cid = raw[pos:pos + 4]
        size = struct.unpack("<I", raw[pos + 4:pos + 8])[0]
        body = pos + 8
        if cid == b"fmt ":
            tag, chans, rate, _, align, bits = struct.unpack("<HHIIHH", raw[body:body + 16])
            if tag == 0xFFFE and size >= 26:  # WAVE_FORMAT_EXTENSIBLE: the sub-format says PCM
                tag = struct.unpack("<H", raw[body + 24:body + 26])[0]
            fmt = (tag, chans, rate, align, bits)
        elif cid == b"data":
            avail = len(raw) - body
            if size == 0 or size > avail:  # still being recorded: the header is not final
                size = avail
            data = (body, size)
        pos = body + size + (size & 1)
    if fmt is None or data is None:
        raise WavError("the file has no fmt or data chunk")
    tag, chans, rate, align, bits = fmt
    if tag != 1:
        raise WavError("only PCM is read (format tag %d)" % tag)
    if bits not in (8, 16, 24) or chans < 1 or rate < 1:
        raise WavError("unsupported format: %d bit, %d channel(s), %d Hz" % (bits, chans, rate))
    if align != chans * bits // 8:
        raise WavError("block align %d does not match %d channel(s) of %d bit" % (align, chans, bits))
    return chans, rate, bits, align, data[0], data[1]


def _raw(source):
    return open(source, "rb").read() if isinstance(source, str) else bytes(source)


def load(source, start_frame=0):
    """A Wav from a path or from the bytes of a file. start_frame skips that many frames at the start."""
    raw = _raw(source)
    chans, rate, bits, align, body, size = _parse(raw)
    start = body + min(max(start_frame, 0) * align, size)
    frames = max(0, (body + size - start) // align)
    ints = _decode(raw, start, frames * align, bits)
    scale = 1.0 / float(2 ** (bits - 1))
    chans_out = [array("f", [v * scale for v in ints[c::chans]]) for c in range(chans)]
    return Wav(rate, bits, chans_out)


def frame_count(source):
    """Frames the file holds now, read from the header and the file size only (no samples decoded)."""
    chans, rate, bits, align, body, size = _parse(_raw(source))
    return size // align


def _decode(raw, start, nbytes, bits):
    chunk = raw[start:start + nbytes]
    if bits == 16:
        return struct.unpack("<%dh" % (len(chunk) // 2), chunk)
    if bits == 8:
        return [b - 128 for b in chunk]
    # 24 bit: three little-endian bytes per sample, sign from the top bit (int.from_bytes is a builtin, not a dependency)
    return [int.from_bytes(chunk[i:i + 3], "little", signed=True) for i in range(0, len(chunk), 3)]


def _as_wav(x):
    return x if isinstance(x, Wav) else load(x)


def _db(v):
    return -math.inf if v <= 0 else 20.0 * math.log10(v)


def _rms(seg):
    n = len(seg)
    return math.sqrt(sum(v * v for v in seg) / n) if n else 0.0


def _windows(x, n):
    """(start index, RMS) for consecutive windows of n samples."""
    return [(s, _rms(x[s:s + n])) for s in range(0, len(x) - n + 1, n)]


def duration(wav):
    wav = _as_wav(wav)
    return wav.frames / float(wav.rate)


def level_dbfs(wav, channel=None):
    """RMS and peak of the channel (the mix when None), in dBFS. Silence is -inf dBFS."""
    wav = _as_wav(wav)
    x = wav.pick(channel)
    rms = _rms(x)
    peak = max((abs(v) for v in x), default=0.0)
    return {"rms": rms, "rms_dbfs": _db(rms), "peak": peak, "peak_dbfs": _db(peak)}


def silence(wav, threshold_dbfs=-60.0, channel=None, window_s=0.05):
    """True when no window of the recording (of every channel when channel is None) is louder than the threshold."""
    wav = _as_wav(wav)
    n = max(1, int(round(window_s * wav.rate)))
    for x in wav.each(channel):
        for _, rms in _windows(x, n):
            if _db(rms) >= threshold_dbfs:
                return False
    return True


def first_sound_at(wav, threshold_dbfs=-50.0, channel=None, window_s=0.02):
    """Seconds from the start to the first window louder than the threshold (all channels when None), or None for silence."""
    wav = _as_wav(wav)
    n = max(1, int(round(window_s * wav.rate)))
    best = None
    for x in wav.each(channel):
        for s, rms in _windows(x, n):
            if _db(rms) >= threshold_dbfs:
                t = s / float(wav.rate)
                best = t if best is None else min(best, t)
                break
    return best


def sound_starts_within(wav, seconds, threshold_dbfs=-50.0, channel=None):
    t = first_sound_at(wav, threshold_dbfs, channel)
    return t is not None and t <= seconds


def _candidates(hz, tol_hz):
    if tol_hz <= 0:
        return [float(hz)]
    return [hz + tol_hz * k / 2.0 for k in (-2, -1, 0, 1, 2)]


def _goertzel_rms(x, start, n, freq, rate):
    """RMS of the sinusoid at freq in the window (Goertzel: one DFT bin without a full transform)."""
    k = 2.0 * math.cos(2.0 * math.pi * freq / rate)
    s1 = s2 = 0.0
    for i in range(start, start + n):
        s0 = x[i] + k * s1 - s2
        s2, s1 = s1, s0
    power = max(s1 * s1 + s2 * s2 - k * s1 * s2, 0.0)
    amplitude = 2.0 * math.sqrt(power) / n
    return amplitude / math.sqrt(2.0)


def tone_present(wav, hz, tol_hz=10.0, min_dbfs=-40.0, channel=None, min_seconds=0.2, window_s=0.1, purity=0.5):
    """Is a tone at hz (within tol_hz) in the recording? Each window of window_s is measured with Goertzel at five frequencies across
    hz - tol .. hz + tol. A window counts when the tone holds at least `purity` of the window's energy and its RMS is at least min_dbfs.
    The tone is present when the counted windows add up to min_seconds.
    Returns a dict: present, seconds (counted), level_dbfs (median of the counted windows), hz (median frequency), first_at, last_at."""
    wav = _as_wav(wav)
    x = wav.pick(channel)
    n = max(1, int(round(window_s * wav.rate)))
    cands = _candidates(hz, tol_hz)
    counted = []  # (start seconds, frequency, tone dBFS)
    for s, win_rms in _windows(x, n):
        if _db(win_rms) < min_dbfs:  # the tone cannot be louder than the window
            continue
        best_f, best_r = None, 0.0
        for f in cands:
            r = _goertzel_rms(x, s, n, f, wav.rate)
            if r > best_r:
                best_f, best_r = f, r
        if best_f is None:
            continue
        if (best_r * best_r) / (win_rms * win_rms) >= purity and _db(best_r) >= min_dbfs:
            counted.append((s / float(wav.rate), best_f, _db(best_r)))
    seconds = len(counted) * window_s
    freqs = sorted(c[1] for c in counted)
    levels = sorted(c[2] for c in counted)
    return {
        "present": bool(counted) and seconds >= min_seconds,
        "seconds": seconds,
        "hz": freqs[len(freqs) // 2] if freqs else None,
        "level_dbfs": levels[len(levels) // 2] if levels else None,
        "first_at": counted[0][0] if counted else None,
        "last_at": counted[-1][0] + window_s if counted else None,
    }


def clipping(wav, level=0.99, run=3):
    """Number of runs of `run` or more samples at or above `level` (0.99 = 1 percent under full scale) in any channel. 0 means no clipping."""
    wav = _as_wav(wav)
    runs = 0
    for x in wav.chans:
        length = 0
        for v in x:
            if abs(v) >= level:
                length += 1
                if length == run:
                    runs += 1
            else:
                length = 0
    return runs


def left_right_balance(wav):
    """Right minus left, in dB of RMS over the whole recording. Positive: the right channel is louder. None for a mono recording."""
    wav = _as_wav(wav)
    if wav.channels < 2:
        return None
    floor = -120.0  # a silent channel is -120 dB here, not minus infinity
    return max(_db(_rms(wav.chans[1])), floor) - max(_db(_rms(wav.chans[0])), floor)


def speech_like(wav, min_seconds, max_seconds, channel=None, frame_s=0.02, hangover_s=0.25, margin_db=12.0, floor_dbfs=-50.0):
    """Voice activity from the energy envelope. A frame is active when its RMS is margin_db above the noise floor (the tenth percentile of the
    frames, never below floor_dbfs). Gaps shorter than hangover_s between active frames are bridged (the hangover), so a word's syllables
    count as one burst. `seconds` is the span from the first active frame to the last one.
    ok when min_seconds <= seconds <= max_seconds. This says how long energy comes and goes, not what was said."""
    wav = _as_wav(wav)
    x = wav.pick(channel)
    n = max(1, int(round(frame_s * wav.rate)))
    db = [_db(r) for _, r in _windows(x, n)]
    if not db:
        return {"ok": False, "seconds": 0.0, "bursts": 0, "floor_dbfs": None, "threshold_dbfs": None}
    ordered = sorted(db)
    floor = ordered[len(ordered) // 10]
    # a steady sound has no quiet frames to measure a floor on: it counts as activity above the absolute floor
    thr = max(floor + margin_db, floor_dbfs) if ordered[-1] - floor >= margin_db else floor_dbfs
    active = [i for i, v in enumerate(db) if v >= thr]
    if not active:
        return {"ok": False, "seconds": 0.0, "bursts": 0, "floor_dbfs": floor, "threshold_dbfs": thr}
    hang = int(round(hangover_s / frame_s))
    bursts, prev = 1, active[0]
    for i in active[1:]:
        if i - prev > hang + 1:  # a gap longer than the hangover ends the burst
            bursts += 1
        prev = i
    seconds = (active[-1] - active[0] + 1) * frame_s
    return {"ok": min_seconds <= seconds <= max_seconds, "seconds": seconds, "bursts": bursts, "floor_dbfs": floor, "threshold_dbfs": thr}


def main(argv):
    if len(argv) != 2:
        print("usage: listen.py FILE.wav", file=sys.stderr)
        return 2
    wav = load(argv[1])
    print("%d Hz, %d channel(s), %d bit, %.2f s" % (wav.rate, wav.channels, wav.bits, duration(wav)))
    for c in range(wav.channels):
        lv = level_dbfs(wav, c)
        print("channel %d: rms %.1f dBFS, peak %.1f dBFS" % (c, lv["rms_dbfs"], lv["peak_dbfs"]))
    print("clipped runs: %d" % clipping(wav))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
