"""Ask an audio-capable OpenRouter model to critique actual audio, not a transcript.

OPENROUTER_API_KEY must be in the environment. Example (from the repo root):
  python tools/listening_review.py --audio mix.wav --prompt prompt.txt --output out/review

Repeat --audio for blind comparisons; files are labelled A, B, ... in order.
Prompts, source hashes, model identity, usage and responses are saved, but never
credentials or base64 audio. Keep review outputs and generated media under out/.
This is a listening aid, not an objective quality test or a mastering processor.
"""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import urllib.error
import urllib.request


def sanitized(value):
    """Providers sometimes echo the submitted base64 audio in validation errors."""
    if isinstance(value, str):
        return re.sub(r"[A-Za-z0-9+/=]{512,}", "[media payload omitted]", value)
    if isinstance(value, dict):
        return {key: sanitized(item) for key, item in value.items()}
    if isinstance(value, list):
        return [sanitized(item) for item in value]
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", type=Path, action="append", required=True)
    parser.add_argument("--prompt", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", default="google/gemini-3.8-flash")
    parser.add_argument("--provider", help="Pin an OpenRouter provider name or slug.")
    parser.add_argument("--max-tokens", type=int, default=12000)
    parser.add_argument("--reasoning", choices=["high", "medium", "low", "none"], default="high")
    parser.add_argument("--temperature", type=float, default=0.5)
    args = parser.parse_args()
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        parser.error("Set OPENROUTER_API_KEY in the environment; do not pass it on the command line.")
    prompt = args.prompt.read_text()
    content = [{"type": "text", "text": prompt}]
    files = []
    for index, path in enumerate(args.audio):
        data = path.read_bytes()
        label = chr(ord("A") + index)
        fmt = path.suffix.lower().lstrip(".")
        if fmt not in {"wav", "mp3", "flac", "ogg", "aac", "m4a", "aiff"}:
            parser.error(f"Unsupported audio extension: {path.suffix}")
        files.append({"label": label, "path": str(path.resolve()),
                      "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
        content.extend([
            {"type": "text", "text": f"Audio {label}:"},
            {"type": "input_audio", "input_audio": {
                "data": base64.b64encode(data).decode("ascii"), "format": fmt}},
        ])
    system = (
        "You are a candid record producer and critical listening engineer. "
        "Listen to the attached audio natively. Base observations on what you hear. "
        "If audio is inaccessible, say so; never infer its sound from the prompt. "
        "Separate audible observations from hypotheses and subjective preferences. "
        "Do not invent exact loudness, true-peak, stereo correlation or spectral measurements. "
        "Do not flatter the composer or assume newer versions are better. "
        "Preserve the music's identity and give specific, prioritized, feasible changes."
    )
    payload = {"model": args.model, "messages": [
        {"role": "system", "content": system}, {"role": "user", "content": content}],
        "reasoning": ({"enabled": False} if args.reasoning == "none" else {"effort": args.reasoning}),
        "max_tokens": args.max_tokens, "temperature": args.temperature,
        "provider": {"allow_fallbacks": False, "sort": "price"}}
    if args.provider:
        payload["provider"] = {"only": [args.provider], "allow_fallbacks": False}
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "request.json").write_text(json.dumps({
        "model": args.model, "system": system, "prompt": prompt, "audio": files,
        "reasoning": payload["reasoning"], "temperature": payload["temperature"],
        "max_tokens": payload["max_tokens"], "provider": payload["provider"],
    }, indent=2) + "\n")
    request = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions",
        data=json.dumps(payload).encode(), headers={
            "Authorization": f"Bearer {key}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=240) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        body = sanitized(error.read().decode(errors="replace"))
        (args.output / "error.txt").write_text(body + "\n")
        raise SystemExit(f"OpenRouter HTTP {error.code}; see {args.output / 'error.txt'}.") from None
    result = sanitized(result)
    (args.output / "response.json").write_text(json.dumps(result, indent=2) + "\n")
    if "error" in result or not result.get("choices"):
        raise SystemExit("OpenRouter returned no completion; see response.json.")
    choice = result["choices"][0]
    review = choice["message"].get("content")
    if not isinstance(review, str) or not review.strip():
        raise SystemExit("OpenRouter returned no review text; see response.json.")
    (args.output / "review.md").write_text(review + "\n")
    print(json.dumps({"model": result.get("model"), "provider": result.get("provider"),
                      "usage": result.get("usage"),
                      "finish_reason": choice.get("finish_reason"),
                      "review": str(args.output / "review.md")}, indent=2))
    print(review)
    if choice.get("finish_reason") == "length":
        raise SystemExit("Review truncated at the output limit; saved for diagnosis, not a completed review.")


if __name__ == "__main__":
    main()
