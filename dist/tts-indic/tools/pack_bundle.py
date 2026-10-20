#!/usr/bin/env python3
"""Print the checksums of exported Indic voice bundles in the form IYAKKI pins them.

  python3 pack_bundle.py <export-dir>            # <export-dir>/<lang>/{vocab.json,hifigan.onnx,fastpitch.onnx}

Output: a catalog.json for <data>/tts/indic/catalog.json (the administrator's own pins, used instead of the built-in ones for the
languages it lists). Copy the three files of each language to the place you publish bundles from, at <address>/<lang>/<file>.
"""
import hashlib, json, os, sys

FILES = ("vocab.json", "hifigan.onnx", "fastpitch.onnx")


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def main():
    root = sys.argv[1]
    cat = {}
    for lang in sorted(os.listdir(root)):
        d = os.path.join(root, lang)
        if all(os.path.isfile(os.path.join(d, f)) for f in FILES):
            cat[lang] = {f: {"sha256": sha(os.path.join(d, f)), "bytes": os.path.getsize(os.path.join(d, f))} for f in FILES}
    json.dump(cat, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
