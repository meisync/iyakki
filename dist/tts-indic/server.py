#!/usr/bin/env python3
"""IYAKKI Indic TTS server: a small OpenAI-compatible /v1/audio/speech server for the AI4Bharat Indic-TTS voices
(FastPitch + HiFi-GAN, exported to ONNX). Needs only numpy and onnxruntime; nothing is downloaded by this program.

  python3 server.py --models /path/to/tts/indic --listen 127.0.0.1:8189

Model folder layout (one folder per language, made by tools/export_onnx.py or Settings -> Voice -> Indic voices):
  <models>/<lang>/fastpitch.onnx  hifigan.onnx  vocab.json

Endpoints
  GET  /healthz           -> {"ok": true}
  GET  /v1/voices         -> {"voices": [{"id": "ta-female", "lang": "ta", "gender": "female"}], "installed": ["ta"]}
  POST /v1/audio/speech   {"input": "...", "voice": "ta-female", "speed": 1.0, "response_format": "wav"} -> audio/wav
The text frontend here is only the model's own cleaner (lower-case, a few symbols, unknown characters dropped). Numbers,
Latin words and long-text chunking are the caller's job (IYAKKI does them); long input is still split on sentence ends.
"""
import argparse, io, json, os, re, struct, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np
import onnxruntime as ort

MAX_BODY = 64 * 1024
MAX_CHARS = 1500
CHUNK = 220
LANG_RE = re.compile(r"^[a-z]{2,3}$")


def clean(t):
    t = t.lower().replace(";", ",").replace("-", " ").replace(":", ",")
    t = re.sub(r'[<>()\[\]"]+', "", t)
    return re.sub(r"\s+", " ", t).strip()


def split_text(t, limit=CHUNK):
    parts = [p.strip() for p in re.split(r"(?<=[.!?।])\s+", t) if p.strip()]
    out = []
    for p in parts:
        while len(p) > limit:
            cut = max(p.rfind(",", 0, limit), p.rfind(" ", 0, limit))
            if cut < limit // 3:
                cut = limit
            out.append(p[:cut].strip())
            p = p[cut:].lstrip(", ")
        if p:
            out.append(p)
    return out


class Voice:
    def __init__(self, d, threads):
        self.v = json.load(open(os.path.join(d, "vocab.json"), encoding="utf-8"))
        self.c2i = {t: int(i) for i, t in self.v["tokens"].items()}
        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        self.fp = ort.InferenceSession(os.path.join(d, "fastpitch.onnx"), so, providers=["CPUExecutionProvider"])
        self.hg = ort.InferenceSession(os.path.join(d, "hifigan.onnx"), so, providers=["CPUExecutionProvider"])
        self.rate = int(self.v["sample_rate"])
        ids = self.v.get("speaker_ids") or {n: i for i, n in enumerate(self.v["speakers"])}
        self.speakers = {k: v for k, v in ids.items() if k in ("female", "male")}

    def one(self, text, speaker, speed):
        ids = [self.c2i[c] for c in clean(text) if c in self.c2i]
        if not ids:
            return np.zeros(0, dtype=np.float32)
        x = np.array([ids], dtype=np.int64)
        scale = np.array(1.0 / max(0.5, min(2.0, speed)), dtype=np.float32)
        mel = self.fp.run(None, {"tokens": x, "speaker": np.array([speaker], dtype=np.int64), "scale": scale})[0]
        return self.hg.run(None, {"mel": mel})[0].reshape(-1)

    def speak(self, text, gender, speed):
        sid = self.speakers.get(gender)
        if sid is None:
            raise KeyError("voice")
        gap = np.zeros(int(self.rate * 0.18), dtype=np.float32)
        out = []
        for piece in split_text(text):
            w = self.one(piece, sid, speed)
            if len(w):
                out += [w, gap]
        return np.concatenate(out[:-1]) if out else np.zeros(0, dtype=np.float32)


def wav_bytes(w, rate):
    pcm = (np.clip(w, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    h = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16) + b"data" + struct.pack("<I", len(pcm))
    return h + pcm


class State:
    def __init__(self, models, threads, keep):
        self.models, self.threads, self.keep = models, threads, keep
        self.loaded = {}
        self.lock = threading.Lock()

    def installed(self):
        out = []
        try:
            for d in sorted(os.listdir(self.models)):
                if LANG_RE.match(d) and all(os.path.isfile(os.path.join(self.models, d, f)) for f in ("fastpitch.onnx", "hifigan.onnx", "vocab.json")):
                    out.append(d)
        except OSError:
            pass
        return out

    def voice(self, lang):
        if lang not in self.installed():
            return None
        v = self.loaded.get(lang)
        if v is None:
            while len(self.loaded) >= self.keep:
                self.loaded.pop(next(iter(self.loaded)))
            v = self.loaded[lang] = Voice(os.path.join(self.models, lang), self.threads)
        return v

    def voices(self):
        out = []
        for lang in self.installed():
            try:
                v = json.load(open(os.path.join(self.models, lang, "vocab.json"), encoding="utf-8"))
            except (OSError, ValueError):
                continue
            for g in v.get("speakers", []):
                if g not in ("female", "male"):
                    continue
                out.append({"id": f"{lang}-{g}", "lang": lang, "gender": g, "sample_rate": v.get("sample_rate")})
        return out


class H(BaseHTTPRequestHandler):
    server_version = "iyakki-tts-indic"
    protocol_version = "HTTP/1.1"
    timeout = 30

    def log_message(self, f, *a):
        sys.stderr.write("%s %s\n" % (self.address_string(), f % a))

    def send(self, code, body, ctype="application/json"):
        if isinstance(body, (dict, list)):
            body = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        st = self.server.st
        if self.path == "/healthz":
            return self.send(200, {"ok": True})
        if self.path == "/v1/voices":
            return self.send(200, {"voices": st.voices(), "installed": st.installed()})
        self.send(404, {"error": "not found"})

    def do_POST(self):
        st = self.server.st
        if self.path != "/v1/audio/speech":
            return self.send(404, {"error": "not found"})
        try:
            n = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            n = -1
        if n <= 0 or n > MAX_BODY:
            return self.send(413, {"error": "body missing or too large"})
        try:
            req = json.loads(self.rfile.read(n))
            text = str(req["input"])
            voice = str(req.get("voice", ""))
            speed = float(req.get("speed", 1.0))
        except (ValueError, KeyError, TypeError):
            return self.send(400, {"error": "need JSON with input and voice"})
        if len(text) > MAX_CHARS:
            return self.send(413, {"error": f"input longer than {MAX_CHARS} characters"})
        m = re.match(r"^([a-z]{2,3})-([a-z]{3,10})$", voice)
        if not m:
            return self.send(400, {"error": "voice is <lang>-<gender>, for example ta-female"})
        lang, gender = m.groups()
        with st.lock:
            try:
                v = st.voice(lang)
                if v is None:
                    return self.send(404, {"error": f"model for {lang} not installed", "code": "model_not_installed", "lang": lang})
                t0 = time.time()
                w = v.speak(text, gender, speed if speed == speed else 1.0)
            except KeyError:
                return self.send(404, {"error": f"no voice {voice}", "code": "no_voice"})
            except Exception as e:  # inference failure must not kill the server
                return self.send(500, {"error": f"synthesis failed: {type(e).__name__}"})
        if not len(w):
            return self.send(422, {"error": "nothing speakable in the text (characters the model does not know)", "code": "nothing_to_say"})
        sys.stderr.write("synth %s %d chars %.2fs audio in %.2fs\n" % (voice, len(text), len(w) / v.rate, time.time() - t0))
        self.send(200, wav_bytes(w, v.rate), "audio/wav")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models", default=os.environ.get("IYAKKI_TTS_MODELS", "models"))
    ap.add_argument("--listen", default=os.environ.get("IYAKKI_TTS_LISTEN", "127.0.0.1:8189"))
    ap.add_argument("--threads", type=int, default=int(os.environ.get("IYAKKI_TTS_THREADS", "2")))
    ap.add_argument("--keep", type=int, default=2, help="languages kept loaded")
    a = ap.parse_args()
    host, _, port = a.listen.rpartition(":")
    srv = ThreadingHTTPServer((host or "127.0.0.1", int(port)), H)
    srv.st = State(a.models, a.threads, max(1, a.keep))
    sys.stderr.write("indic tts on %s, models %s, installed %s\n" % (a.listen, a.models, srv.st.installed()))
    srv.serve_forever()


if __name__ == "__main__":
    main()
