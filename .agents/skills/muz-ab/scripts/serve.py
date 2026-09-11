#!/usr/bin/env python3
"""Prepare a muz batch pair and serve a private, local blind listening session."""

import argparse
import fcntl
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
from fractions import Fraction
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from sessions import SessionError, SessionStore, atomic_write, json_bytes


WEB = Path(__file__).resolve().parent / "web"
STATIC = {"/": ("index.html", "text/html; charset=utf-8"),
          "/index.html": ("index.html", "text/html; charset=utf-8"),
          "/app.js": ("app.js", "text/javascript; charset=utf-8"),
          "/audio.js": ("audio.js", "text/javascript; charset=utf-8"),
          "/style.css": ("style.css", "text/css; charset=utf-8")}
SESSION_ROUTE = re.compile(r"/api/sessions/([0-9a-f]{32})(?:/(answer|finish|export\.json|export\.csv))?")
AUDIO_ROUTE = re.compile(r"/api/sessions/([0-9a-f]{32})/trials/([0-9a-f]{32})/audio/([ABX])\.wav")


def sha256(path):
    with Path(path).open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def command(args):
    try:
        return subprocess.run(args, check=True, capture_output=True, text=True).stdout
    except FileNotFoundError:
        raise ValueError(f"Required executable not found: {args[0]}") from None
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip()[-2000:]
        raise ValueError(f"{args[0]} failed: {detail}") from None


def probe(path):
    metadata = json.loads(command([
        "ffprobe", "-v", "error", "-select_streams", "a", "-show_entries",
        "stream=codec_name,sample_rate,channels,duration,duration_ts,time_base:format=duration",
        "-of", "json", str(path),
    ]))
    streams = metadata.get("streams", [])
    if len(streams) != 1:
        raise ValueError(f"Expected exactly one audio stream: {path}")
    stream = streams[0]
    try:
        rate = int(stream["sample_rate"])
        channels = int(stream["channels"])
        if rate <= 0 or channels <= 0:
            raise ValueError
        if stream.get("duration_ts") is not None and stream.get("time_base"):
            duration = Fraction(stream["duration_ts"]) * Fraction(stream["time_base"])
        else:
            duration = Fraction(stream.get("duration", metadata.get("format", {}).get("duration")))
        exact_frames = duration * rate
        frames = round(exact_frames)
        if frames <= 0 or abs(exact_frames - frames) > Fraction(1, 10):
            raise ValueError
    except (KeyError, TypeError, ValueError, ZeroDivisionError, OverflowError):
        raise ValueError(f"ffprobe could not establish sample-accurate audio duration: {path}") from None
    return {"sample_rate": rate, "channels": channels, "frames": frames,
            "duration": float(duration), "codec": stream.get("codec_name")}


def candidate(row, manifest):
    name = row.get("name")
    if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        raise ValueError("Successful outputs must have valid muz output names")
    analysis = row.get("analysis")
    loudness = analysis.get("integrated_lufs") if isinstance(analysis, dict) else None
    if isinstance(loudness, bool) or not isinstance(loudness, (int, float)) or not math.isfinite(loudness):
        raise ValueError(f"Output {name!r} needs finite analysis.integrated_lufs; render with --match-levels")
    report = row["report"]
    reported_path = report.get("output")
    if isinstance(reported_path, str) and reported_path:
        path = Path(reported_path)
        if not path.is_absolute():
            path = manifest.parent / path
        if not path.is_file():
            path = manifest.parent / f"{name}.wav"
    else:
        path = manifest.parent / f"{name}.wav"
    path = path.resolve()
    if not path.is_file():
        raise ValueError(f"Audio file not found for {name!r}: {path}")
    return {"name": name, "path": str(path), "sha256": sha256(path),
            "integrated_lufs": float(loudness), "analysis": analysis,
            "render_report": report, "audio": probe(path)}


def prepare(manifest, output, a=None, b=None, label_a=None, label_b=None, title="Blind listening comparison"):
    manifest = Path(manifest).resolve()
    output = Path(output).resolve()
    raw_manifest = manifest.read_bytes()
    data = json.loads(raw_manifest)
    rows = data.get("outputs") if isinstance(data, dict) else None
    if not isinstance(rows, list):
        raise ValueError("Manifest must contain an outputs array")
    successful = [row for row in rows if isinstance(row, dict) and "error" not in row and isinstance(row.get("report"), dict)]
    names = [row.get("name") for row in successful]
    if any(not isinstance(name, str) for name in names) or len(set(names)) != len(names):
        raise ValueError("Successful output names must be unique strings")
    if (a is None) != (b is None):
        raise ValueError("Supply both --a and --b, or neither")
    if a is None:
        if len(successful) != 2:
            raise ValueError("Automatic selection requires exactly two successful outputs; select --a NAME --b NAME")
        selected = successful
    else:
        if a == b or a not in names or b not in names:
            raise ValueError("--a and --b must name distinct successful outputs")
        selected = [successful[names.index(a)], successful[names.index(b)]]
    candidates = [candidate(row, manifest) for row in selected]
    first, second = (item["audio"] for item in candidates)
    if first["sample_rate"] != second["sample_rate"] or first["channels"] != second["channels"]:
        raise ValueError("Candidates must have the same sample rate and channel count; no resampling is performed")
    if abs(first["frames"] - second["frames"]) > 1:
        raise ValueError("Candidates differ in duration by more than one sample; render aligned candidates")
    rate, channels = first["sample_rate"], first["channels"]
    frames = min(first["frames"], second["frames"])
    target = min(item["integrated_lufs"] for item in candidates)
    labels = [label_a if label_a is not None else candidates[0]["name"],
              label_b if label_b is not None else candidates[1]["name"]]
    for item, label in zip(candidates, labels):
        item["label"] = label
        item["listening_gain_db"] = min(0.0, target - item["integrated_lufs"])
    identity = {"version": 1, "format": "pcm_f32le", "sample_rate": rate, "channels": channels, "frames": frames,
                "candidates": [{"sha256": item["sha256"], "listening_gain_db": item["listening_gain_db"]} for item in candidates]}
    pair_id = hashlib.sha256(json_bytes(identity)).hexdigest()
    config = {"pair_id": pair_id, "title": title, "duration": frames / rate,
              "sample_rate": rate, "channels": channels, "level_matched": True}
    pair_file = output / "private" / "pair.json"
    if pair_file.exists() and json.loads(pair_file.read_text(encoding="utf-8")).get("config", {}).get("pair_id") != pair_id:
        raise ValueError("Output directory belongs to a different audio pair; choose another --output directory")
    media = output / "media"
    media.mkdir(parents=True, exist_ok=True)
    paths = [media / f"{pair_id}-{index}.wav" for index in (0, 1)]
    cache_path = output / "private" / "media.json"
    cache = json.loads(cache_path.read_text(encoding="utf-8")) if cache_path.exists() else {}
    cached_hashes = cache.get("sha256", [])
    cached = cache.get("pair_id") == pair_id and len(cached_hashes) == 2 and all(
        path.is_file() and sha256(path) == digest for path, digest in zip(paths, cached_hashes))
    if not cached:
        for item, destination in zip(candidates, paths):
            if destination.resolve() in (Path(source["path"]) for source in candidates):
                raise ValueError("Prepared media destination would overwrite a source bounce")
            temporary = None
            try:
                with tempfile.NamedTemporaryFile(dir=media, suffix=".wav", delete=False) as handle:
                    temporary = Path(handle.name)
                command(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", item["path"],
                         "-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1",
                         "-af", f"volume={item['listening_gain_db']:.17g}dB:precision=double,atrim=end_sample={frames},asetpts=PTS-STARTPTS",
                         "-c:a", "pcm_f32le", "-fflags", "+bitexact", "-flags:a", "+bitexact", "-f", "wav", str(temporary)])
                prepared = probe(temporary)
                if (prepared["sample_rate"], prepared["channels"], prepared["frames"], prepared["codec"]) != (rate, channels, frames, "pcm_f32le"):
                    raise ValueError("Prepared audio does not have the required aligned float32 format")
                with temporary.open("rb") as handle:
                    os.fsync(handle.fileno())
                os.replace(temporary, destination)
                temporary = None
            finally:
                if temporary is not None:
                    temporary.unlink(missing_ok=True)
        cache = {"pair_id": pair_id, "sha256": [sha256(path) for path in paths],
                 "ffmpeg_version": command(["ffmpeg", "-version"]).splitlines()[0]}
        atomic_write(cache_path, json_bytes(cache))
    provenance = {
        "pair_id": pair_id,
        "manifest_path": str(manifest), "manifest_sha256": hashlib.sha256(raw_manifest).hexdigest(),
        "source": data.get("source"), "recipe": data.get("recipe"), "candidates": candidates,
        "preparation": {**identity, "target_integrated_lufs": target, "prepared_sha256": cache["sha256"],
                        "ffmpeg_version": cache["ffmpeg_version"],
                        "policy": "Constant attenuation to the quieter selected source; metadata stripped; no boost, resampling, stretching, or original modification. At most one trailing sample is trimmed to align lengths."},
    }
    return config, labels, provenance, paths


class ComparisonServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port, store, media):
        self.store = store
        self.media = media
        super().__init__(("127.0.0.1", port), Handler)
        actual_port = self.server_address[1]
        self.hosts = {f"127.0.0.1:{actual_port}", f"localhost:{actual_port}"}


class Handler(BaseHTTPRequestHandler):
    server_version = "MuzAB"
    sys_version = ""

    def log_message(self, format, *args):
        # Session capabilities and trial routes need not enter access logs.
        pass

    def _headers(self, status, mime, size, filename=None):
        self.send_response(status)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(size))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; media-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")
        if filename:
            self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
        self.end_headers()

    def _json(self, status, value):
        content = json_bytes(value)
        self._headers(status, "application/json; charset=utf-8", len(content))
        if self.command != "HEAD":
            self.wfile.write(content)

    def _file(self, path, mime, filename=None):
        try:
            handle = Path(path).open("rb")
        except FileNotFoundError:
            raise SessionError(404, "Resource not found") from None
        with handle:
            self._headers(200, mime, os.fstat(handle.fileno()).st_size, filename)
            if self.command != "HEAD":
                shutil.copyfileobj(handle, self.wfile)

    def _check_request(self):
        hosts = self.headers.get_all("Host", [])
        if len(hosts) != 1 or hosts[0] not in self.server.hosts:
            raise SessionError(403, "Foreign Host is not allowed")
        origins = self.headers.get_all("Origin", [])
        if len(origins) > 1 or (origins and origins[0] != f"http://{hosts[0]}"):
            raise SessionError(403, "Foreign Origin is not allowed")
        if self.command == "POST" and not origins:
            raise SessionError(403, "POST requires a same-origin Origin header")
        fetch_site = self.headers.get("Sec-Fetch-Site")
        if fetch_site not in (None, "none", "same-origin"):
            raise SessionError(403, "Cross-origin requests are not allowed")
        try:
            parts = urlsplit(self.path)
        except ValueError:
            raise SessionError(400, "Invalid request target") from None
        if not self.path.startswith("/") or parts.scheme or parts.netloc or parts.fragment:
            raise SessionError(400, "Invalid request target")
        return parts.path

    def _payload(self):
        types = self.headers.get_all("Content-Type", [])
        if len(types) != 1 or types[0].split(";", 1)[0].strip().lower() != "application/json":
            raise SessionError(415, "POST requires application/json")
        lengths = self.headers.get_all("Content-Length", [])
        if self.headers.get("Transfer-Encoding") or len(lengths) != 1 or not re.fullmatch(r"[0-9]+", lengths[0]):
            raise SessionError(400, "A single Content-Length is required")
        digits = lengths[0].lstrip("0") or "0"
        if len(digits) > 5 or int(digits) > 65536:
            raise SessionError(413, "JSON request exceeds 65536 bytes")
        length = int(digits)
        raw = self.rfile.read(length)
        if len(raw) != length:
            raise SessionError(400, "Incomplete JSON request")
        try:
            def reject_constant(value):
                raise ValueError("Non-finite JSON number")
            payload = json.loads(raw, parse_constant=reject_constant)
        except (UnicodeDecodeError, ValueError, RecursionError):
            raise SessionError(400, "Invalid JSON request") from None
        if not isinstance(payload, dict):
            raise SessionError(400, "JSON request must be an object")
        return payload

    def _dispatch(self):
        try:
            self.connection.settimeout(30)
            path = self._check_request()
            if self.command in ("GET", "HEAD"):
                if path in STATIC:
                    filename, mime = STATIC[path]
                    self._file(WEB / filename, mime)
                    return
                if path == "/api/config":
                    self._json(200, self.server.store.config)
                    return
                audio = AUDIO_ROUTE.fullmatch(path)
                if audio:
                    index = self.server.store.media_candidate(*audio.groups())
                    self._file(self.server.media[index], "audio/wav")
                    return
                session = SESSION_ROUTE.fullmatch(path)
                if session:
                    session_id, action = session.groups()
                    if action is None:
                        self._json(200, self.server.store.get(session_id))
                        return
                    if action in ("export.json", "export.csv"):
                        suffix = action.split(".")[1]
                        file = self.server.store.export(session_id, suffix)
                        mime = "application/json; charset=utf-8" if suffix == "json" else "text/csv; charset=utf-8"
                        self._file(file, mime, f"{session_id}.{suffix}")
                        return
            elif self.command == "POST":
                payload = self._payload()
                if path == "/api/sessions":
                    self._json(201, self.server.store.create(payload))
                    return
                session = SESSION_ROUTE.fullmatch(path)
                if session:
                    session_id, action = session.groups()
                    if action == "answer":
                        self._json(200, self.server.store.answer(session_id, payload))
                        return
                    if action == "finish":
                        if payload:
                            raise SessionError(400, "finish requires an empty JSON object")
                        self._json(200, self.server.store.finish(session_id))
                        return
            raise SessionError(404, "Resource not found")
        except SessionError as error:
            self._json(error.status, {"error": str(error)})
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            self.close_connection = True
        except Exception as error:
            print(f"Request failed ({type(error).__name__}); inspect local state or restart the server", file=sys.stderr)
            self._json(500, {"error": "Internal server error; session state remains authoritative"})

    do_GET = _dispatch
    do_HEAD = _dispatch
    do_POST = _dispatch

    def send_error(self, code, message=None, explain=None):
        self.close_connection = True
        self._json(code, {"error": "Unsupported or malformed HTTP request"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--a")
    parser.add_argument("--b")
    parser.add_argument("--label-a")
    parser.add_argument("--label-b")
    parser.add_argument("--title", default="Blind listening comparison")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error("--port must be between 0 and 65535 (0 chooses an available port)")
    if not args.title.strip() or len(args.title) > 200:
        parser.error("--title must contain 1 to 200 characters and must not reveal source identities")
    output = args.output.resolve()
    private = output / "private"
    private.mkdir(parents=True, exist_ok=True, mode=0o700)
    private.chmod(0o700)
    # One server process owns an output directory, so per-session transitions
    # cannot race a second server's independent in-process lock.
    with (private / "server.lock").open("a+b") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            parser.error("Another server is using this --output directory")
        try:
            config, labels, provenance, media = prepare(args.manifest, output, args.a, args.b, args.label_a, args.label_b, args.title)
            store = SessionStore(output, config, labels, provenance)
            with ComparisonServer(args.port, store, media) as server:
                print(f"READY http://127.0.0.1:{server.server_address[1]}", flush=True)
                try:
                    server.serve_forever()
                except KeyboardInterrupt:
                    pass
        except (ValueError, OSError, SessionError) as error:
            parser.error(str(error))


if __name__ == "__main__":
    main()
