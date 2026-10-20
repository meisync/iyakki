# IYAKKI Indic voice server (AI4Bharat Indic-TTS as ONNX, OpenAI-compatible /v1/audio/speech).
#
#   docker build -t localhost/iyakki-tts-indic:1.0.0 dist/tts-indic
#   docker run --rm -p 8189:8189 -v /path/to/tts/indic:/models:ro localhost/iyakki-tts-indic:1.0.0
#
# The image holds the server (numpy + onnxruntime) and NO model. The models are a volume (/models/<lang>/{fastpitch.onnx,hifigan.onnx,
# vocab.json}). Building needs network access once (the base image and two Python wheels), running needs none. Runs as user 10001,
# works with a read-only root file system.
FROM python:3.12-slim-bookworm
RUN pip install --no-cache-dir --disable-pip-version-check numpy==1.26.4 onnxruntime==1.23.2 \
 && useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin tts \
 && mkdir /models && chown 10001:10001 /models
COPY server.py /app/server.py
ENV IYAKKI_TTS_MODELS=/models IYAKKI_TTS_LISTEN=0.0.0.0:8189 IYAKKI_TTS_THREADS=2 PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 10001:10001
EXPOSE 8189
HEALTHCHECK --interval=30s --timeout=3s CMD python -c "import urllib.request;urllib.request.urlopen('http://127.0.0.1:8189/healthz',timeout=2)"
ENTRYPOINT ["python", "/app/server.py"]
