#!/usr/bin/env python3
"""Capture a real streamed model response, with its timing, for the replay console.

    OPENAI_API_KEY=... python3 examples/console/capture.py \
        --model gpt-6.2-sol --name pinball \
        --prompt "Build a playable pinball table as one HTML file."

Writes examples/console/transcripts/<name>.json: the prompt, the model, every streamed
text delta with the milliseconds since the request was sent, and the usage the API reported.
app.html replays exactly that. Nothing is edited after capture; if the answer is not good
enough for a video, capture another one rather than touching this one.
"""

import argparse
import datetime
import json
import os
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--prompt", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--max-output-tokens", type=int, default=6000)
    a = ap.parse_args()
    key = os.environ.get("OPENAI_API_KEY")
    if not key:
        sys.exit("set OPENAI_API_KEY in your environment first")

    body = json.dumps({
        "model": a.model,
        "input": a.prompt,
        "stream": True,
        "max_output_tokens": a.max_output_tokens,
    }).encode()
    req = urllib.request.Request(
        "https://api.openai.com/v1/responses",
        data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )
    t0 = time.monotonic()
    chunks, usage = [], None
    with urllib.request.urlopen(req, timeout=600) as r:
        for raw in r:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            ev = json.loads(data)
            kind = ev.get("type", "")
            if kind == "response.output_text.delta":
                chunks.append({"t_ms": round((time.monotonic() - t0) * 1000), "text": ev["delta"]})
            elif kind == "response.completed":
                usage = ev.get("response", {}).get("usage")
            elif kind in ("response.failed", "error"):
                sys.exit(f"the API reported an error: {json.dumps(ev)[:400]}")
    if not chunks:
        sys.exit("no text came back")
    out = {
        "model": a.model,
        "prompt": a.prompt,
        "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "usage": usage,
        "chunks": chunks,
    }
    os.makedirs(os.path.join(HERE, "transcripts"), exist_ok=True)
    path = os.path.join(HERE, "transcripts", f"{a.name}.json")
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
    text = "".join(c["text"] for c in chunks)
    print(f"wrote {path}: {len(chunks)} chunks, {chunks[-1]['t_ms'] / 1000:.1f}s, {len(text)} chars")


if __name__ == "__main__":
    main()
