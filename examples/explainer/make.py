#!/usr/bin/env python3
"""Film an "X in 60 seconds" explainer from an episode file.

    python3 examples/explainer/make.py examples/explainer/episodes/mcp.json --out mcp.mp4

An episode is a list of scenes (see README.md). Each scene's "say" is narrated while it is on
screen; a cards scene narrates its intro, then each card as it appears; a terminal scene has
kaviri type its command for real. This writes the kaviri script next to the output, serves the
repo so the stage can load its fonts and media, and runs kaviri record with narration,
burned captions and an optional music bed.

The voice is the episode's "voice" (any kaviri voice, "engine:voice" included), or --voice.
"""

import argparse
import functools
import http.server
import json
import os
import subprocess
import sys
import threading
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def script(ep, ep_url, port):
    url = f"http://127.0.0.1:{port}/examples/explainer/stage.html?ep={urllib.parse.quote(ep_url)}"
    ops = [{"op": "navigate", "url": url},
           {"op": "wait", "selector": "body[data-ready]", "timeout_ms": 20000},
           {"op": "wait", "ms": 600},
           {"op": "start_recording"},
           {"op": "wait", "ms": 200}]

    def say(text):
        if text:
            ops.extend([{"op": "say", "text": text}, {"op": "wait", "ms": 120}])

    def press(ms=150):
        ops.append({"op": "press", "key": "ArrowRight"})
        ops.append({"op": "wait", "ms": ms, "stretch": False})

    for i, s in enumerate(ep["scenes"]):
        if i:
            press(250)
        if s["type"] == "terminal":
            ops.append({"op": "say", "text": s.get("say", "")}) if s.get("say") else None
            ops.append({"op": "wait", "ms": 700, "stretch": False})
            ops.append({"op": "type", "selector": ".scene.on .cmd", "text": s["command"], "typewriter_ms": 40})
            ops.append({"op": "wait", "ms": 150})
            ops.append({"op": "press", "key": "Enter"})
            ops.append({"op": "wait", "ms": 600, "stretch": False})
            continue
        say(s.get("say"))
        for it in s.get("items", []) if s["type"] == "cards" else []:
            press(150)
            say(it.get("say"))
    ops.append({"op": "wait", "ms": 500})
    ops.append({"op": "title", "style": "end", "at": "end", "ms": int(ep.get("end_ms", 2400))})
    ops.append({"op": "stop_recording"})
    return [o for o in ops if o]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("episode")
    ap.add_argument("--out", required=True)
    ap.add_argument("--voice")
    ap.add_argument("--music", help="a music bed you have the rights to; ducked under the voice")
    ap.add_argument("--slowmo", default="4", help="page clock slowdown for smooth capture (1 is off)")
    ap.add_argument("--kaviri", default=os.path.join(ROOT, "target", "release", "kaviri"))
    ap.add_argument("--port", type=int, default=8197)
    a = ap.parse_args()

    ep_path = os.path.abspath(a.episode)
    ep = json.load(open(ep_path))
    if not ep_path.startswith(ROOT + os.sep):
        sys.exit("the episode has to live inside the repo, so the stage can fetch it")
    ep_url = "/" + os.path.relpath(ep_path, ROOT).replace(os.sep, "/")
    out = os.path.abspath(a.out)
    jsonl = os.path.splitext(out)[0] + ".jsonl"
    with open(jsonl, "w") as f:
        for op in script(ep, ep_url, a.port):
            f.write(json.dumps(op, ensure_ascii=False) + "\n")

    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=ROOT)
    handler.log_message = lambda *args: None
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", a.port), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    voice = a.voice or ep.get("voice")
    cmd = [a.kaviri, "record", "--script", jsonl, "--out", out, "--preset", "tiktok",
           "--background", "none", "--cursor", "none", "--slowmo", str(a.slowmo),
           "--tts", "piper", "--captions", "burn", "--caption-style", "bold"]
    if voice:
        cmd += ["--voice", voice]
    if a.music:
        cmd += ["--music", a.music, "--music-volume", "-20"]
    print(" ".join(cmd), file=sys.stderr)
    try:
        sys.exit(subprocess.call(cmd))
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
