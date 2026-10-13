#!/usr/bin/env python3
"""Makes the IYAKKI sample media from nothing: a melody, a kuthu-style drum loop and a short test-pattern video, all
synthesised here (numpy) and encoded with ffmpeg. No third-party recordings or pictures are used.
Dedicated to the public domain (CC0 1.0), see README.md.

    python3 make-samples.py OUT_DIR          # writes OUT_DIR/Music/*.mp3 and OUT_DIR/Videos/*.webm
Needs: python3, numpy, ffmpeg with libmp3lame, libvpx-vp9 and libopus.
"""
import subprocess, sys, os
import numpy as np

SR = 44100
rng = np.random.default_rng(7)  # fixed seed: the same files every time


def env(n, a=0.01, d=0.15, s=0.6, r=0.2):
    e = np.ones(n)
    na, nd, nr = int(a * SR), int(d * SR), int(r * SR)
    e[:na] = np.linspace(0, 1, na, endpoint=False)
    e[na:na + nd] = np.linspace(1, s, nd)
    e[na + nd:] = s
    if nr < n:
        e[-nr:] *= np.linspace(1, 0, nr)
    return e


def tone(freq, dur, harm=(1, .5, .25, .12), vib=0.0):
    n = int(dur * SR)
    t = np.arange(n) / SR
    ph = 2 * np.pi * freq * t + (vib * np.sin(2 * np.pi * 5.5 * t) if vib else 0)
    y = sum(h * np.sin((i + 1) * ph) for i, h in enumerate(harm))
    return y * env(n, 0.012, 0.2, 0.55, min(0.25, dur / 2))


def place(buf, y, at):
    i = int(at * SR)
    j = min(len(buf), i + len(y))
    if i < len(buf):
        buf[i:j] += y[:j - i]


def echo(x, delay=0.28, gain=0.28, times=3):
    out = x.copy()
    for k in range(1, times + 1):
        d = int(delay * k * SR)
        out[d:] += x[:-d] * gain ** k
    return out


def melody():
    # A minor pentatonic, a slow lilting tune in 3 phrases, a soft pad underneath. 78 bpm, 6/8 feel.
    A = 220.0
    note = lambda semis: A * 2 ** (semis / 12)
    scale = {'a': 0, 'c': 3, 'd': 5, 'e': 7, 'g': 10, 'A': 12, 'C': 15, 'D': 17, 'E': 19}
    beat = 60 / 78 / 2  # an eighth
    phrases = [
        "a c e e D E D c a c e D c a",
        "e e D E C D c a g a c e c a",
        "E E D c D E A E D c a c e a",
        "c e D E D c a g a c a a a a",
    ]
    total = 26.0
    buf = np.zeros(int(total * SR))
    t = 0.5
    for rep in range(3):
        for ph in phrases:
            for ch in ph.split():
                if t > total - 1.5:
                    break
                dur = beat * (2 if ch in 'aA' else 1) * (1.3 if ch in 'eE' else 1)
                place(buf, tone(note(scale[ch]), dur * 1.1, vib=0.25) * 0.5, t)
                t += dur
        t += 0.2
    # pad: a-minor chords under the tune, long and quiet
    for k, root in enumerate([0, -4, -2, -5] * 3):
        at = 0.5 + k * 6 * beat * 1.0
        for iv in (0, 3, 7):
            place(buf, tone(note(root + iv - 12), 6 * beat * 1.05, harm=(1, .3), vib=0.0) * 0.12, at)
    buf = echo(buf)
    buf /= np.abs(buf).max() * 1.15
    # fade out
    f = int(1.5 * SR)
    buf[-f:] *= np.linspace(1, 0, f)
    return buf


def drum_loop():
    bpm = 140
    step = 60 / bpm / 4  # a sixteenth
    steps = 16
    total = 20.0
    buf = np.zeros(int(total * SR))

    def kick():
        n = int(0.28 * SR); t = np.arange(n) / SR
        f = 130 * np.exp(-t * 18) + 45
        return np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 11)

    def thavil():  # a sharp, tuned skin hit: noise burst on a ringing tone
        n = int(0.16 * SR); t = np.arange(n) / SR
        return (np.sin(2 * np.pi * 330 * t) * 0.5 + rng.standard_normal(n) * 0.5) * np.exp(-t * 28)

    def snare():
        n = int(0.2 * SR); t = np.arange(n) / SR
        return (rng.standard_normal(n) * 0.8 + np.sin(2 * np.pi * 190 * t) * 0.4) * np.exp(-t * 20)

    def hat(open_=False):
        n = int((0.18 if open_ else 0.04) * SR); t = np.arange(n) / SR
        x = rng.standard_normal(n); x = np.diff(x, prepend=0)  # a bright edge
        return x * np.exp(-t * (14 if open_ else 80)) * 0.5

    def bell():
        n = int(0.35 * SR); t = np.arange(n) / SR
        return (np.sin(2 * np.pi * 880 * t) + 0.5 * np.sin(2 * np.pi * 1320 * t)) * np.exp(-t * 9) * 0.25

    K = ".x..  ...x  ..x.  ....".replace(' ', '')
    S = "....x.......x..x"
    T = "x.xx..x.x.xx.x.x"  # the busy thavil pattern that gives it the kuthu push
    H = "x.x.x.x.x.x.x.xx"
    kk, ss, tt, hh = kick(), snare(), thavil(), hat()
    ho, bb = hat(True), bell()
    bars = int(total / (step * steps))
    for b in range(bars):
        for i in range(steps):
            at = (b * steps + i) * step + (0.012 if i % 2 else 0)  # a little swing
            if K[i] == 'x': place(buf, kk * 0.95, at)
            if S[i] == 'x': place(buf, ss * 0.7, at)
            if T[i] == 'x': place(buf, tt * (0.55 if i % 4 else 0.8), at)
            if H[i] == 'x': place(buf, hh * 0.5, at)
            if i in (6, 14): place(buf, ho * 0.5, at)
            if b >= 2 and i in (0, 3, 8, 11): place(buf, bb, at)
    buf = np.tanh(buf * 1.4)
    buf /= np.abs(buf).max() * 1.8
    f = int(0.6 * SR)
    buf[-f:] *= np.linspace(1, 0, f)
    return buf


def to_mp3(x, path, title, artist, album, genre, bitrate='96k'):
    pcm = (np.clip(x, -1, 1) * 32767).astype('<i2').tobytes()
    cmd = ['ffmpeg', '-v', 'error', '-y', '-f', 's16le', '-ar', str(SR), '-ac', '1', '-i', '-',
           '-c:a', 'libmp3lame', '-b:a', bitrate, '-id3v2_version', '3', '-write_id3v1', '1',
           '-metadata', f'title={title}', '-metadata', f'artist={artist}', '-metadata', f'album={album}',
           '-metadata', f'genre={genre}', '-metadata', 'date=2026', '-metadata', 'comment=CC0 1.0, made by IYAKKI',
           path]
    subprocess.run(cmd, input=pcm, check=True)


W, H, FPS, SECS = 640, 360, 15, 12


def video(path):
    # test pattern + moving shapes; frames are drawn here and piped to ffmpeg; the sound is a rising two-note tone
    n = FPS * SECS
    yy, xx = np.mgrid[0:H, 0:W]
    bars = [(255, 255, 255), (255, 235, 0), (0, 235, 235), (0, 220, 0), (235, 0, 235), (235, 0, 0), (0, 0, 235)]
    base = np.zeros((H, W, 3), np.uint8)
    for i, c in enumerate(bars):
        base[: int(H * 0.62), i * W // 7:(i + 1) * W // 7] = c
    rows = H - int(H * 0.62)
    grad = np.linspace(0, 255, W)[None, :, None].repeat(rows, 0).repeat(3, 2).astype(np.uint8)
    base[int(H * 0.62):] = grad
    t_audio = np.arange(SECS * SR) / SR
    note = np.where((t_audio % 2) < 1, 440.0, 554.37)
    aud = 0.3 * np.sin(2 * np.pi * np.cumsum(note) / SR) * (0.6 + 0.4 * np.sin(2 * np.pi * 2 * t_audio))
    wav = '/tmp/iyakki-sample-tone.wav'
    pcm = (aud * 32767).astype('<i2').tobytes()
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 's16le', '-ar', str(SR), '-ac', '1', '-i', '-', wav], input=pcm, check=True)
    cmd = ['ffmpeg', '-v', 'error', '-y', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-s', f'{W}x{H}', '-r', str(FPS), '-i', '-', '-i', wav,
           '-c:v', 'libvpx-vp9', '-b:v', '0', '-crf', '33', '-deadline', 'good', '-cpu-used', '4', '-pix_fmt', 'yuv420p', '-g', '30',
           '-c:a', 'libopus', '-b:a', '48k', '-shortest', '-metadata', 'title=IYAKKI Sample Video', path]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    for f in range(n):
        t = f / FPS
        fr = base.copy()
        # a bouncing disc
        cx = int(W / 2 + (W / 2 - 50) * np.sin(t * 1.7)); cy = int(H * 0.31 + 70 * np.sin(t * 2.9))
        m = (xx - cx) ** 2 + (yy - cy) ** 2 < 38 ** 2
        fr[m] = (20, 20, 60)
        m2 = (xx - cx) ** 2 + (yy - cy) ** 2 < 28 ** 2
        fr[m2] = (255, 255, 255)
        # a sweeping bar and a progress strip
        bx = int((t / SECS) * W)
        fr[int(H * 0.62):int(H * 0.62) + 6, :bx] = (255, 80, 80)
        sx = int((np.sin(t * 3.1) * 0.5 + 0.5) * (W - 80))
        fr[int(H * 0.8):int(H * 0.8) + 26, sx:sx + 80] = (255, 120, 20)
        # a rotating square
        a = t * 2.2
        c, s = np.cos(a), np.sin(a)
        u = (xx - 90) * c + (yy - 250) * s; v = -(xx - 90) * s + (yy - 250) * c
        fr[(np.abs(u) < 30) & (np.abs(v) < 30)] = (30, 200, 255)
        p.stdin.write(fr.tobytes())
    p.stdin.close()
    if p.wait() != 0:
        raise SystemExit('ffmpeg failed')
    os.remove(wav)


if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(f'{out}/Music', exist_ok=True)
    os.makedirs(f'{out}/Videos', exist_ok=True)
    to_mp3(melody(), f'{out}/Music/IYAKKI Sample Melody.mp3', 'IYAKKI Sample Melody', 'IYAKKI', 'IYAKKI Samples', 'melody')
    to_mp3(drum_loop(), f'{out}/Music/IYAKKI Sample Beats.mp3', 'IYAKKI Sample Beats', 'IYAKKI', 'IYAKKI Samples', 'kuthu')
    video(f'{out}/Videos/IYAKKI Sample Video.webm')
