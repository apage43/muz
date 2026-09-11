"""Persist blinded, fixed-size listening experiments and their finished reports."""

import copy
import csv
import io
import json
import math
import os
import re
import secrets
import tempfile
import threading
from datetime import datetime, timezone
from pathlib import Path


class SessionError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def atomic_write(path, content):
    """Replace a complete file, including directory durability after rename."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        temporary = None
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")


def exact_abx(correct, trials):
    """One-sided exact probability under independent fair Bernoulli trials."""
    return sum(math.comb(trials, k) for k in range(correct, trials + 1)) / (1 << trials)


def exact_preference(candidate_0, candidate_1):
    """Two-sided exact binomial probability; ties are not Bernoulli trials."""
    trials = candidate_0 + candidate_1
    if not trials:
        return None
    tail = sum(math.comb(trials, k) for k in range(min(candidate_0, candidate_1) + 1))
    return min(1.0, 2 * tail / (1 << trials))


def wilson(successes, trials):
    if not trials:
        return None
    z = 1.959963984540054
    proportion = successes / trials
    denominator = 1 + z * z / trials
    center = (proportion + z * z / (2 * trials)) / denominator
    radius = z * math.sqrt(proportion * (1 - proportion) / trials + z * z / (4 * trials * trials)) / denominator
    return [max(0.0, center - radius), min(1.0, center + radius)]


def report_result(state):
    records = state["records"]
    trials = len(records)
    complete = state["status"] == "completed" and trials == state["planned_trials"]
    result = {
        "mode": state["mode"],
        "planned_trials": state["planned_trials"],
        "trials": trials,
        "completed_as_planned": complete,
        "labels": state["labels"],
        "correct": None,
        "accuracy": None,
        "counts": None,
        "decisive_trials": None,
        "p_value": None,
        "confidence_interval": None,
        "test": "",
        "caveats": [
            "Nominal results describe one listener and this pair; they are not a population or equivalence claim.",
            "Multiple sessions, pairs, or selectively reported results increase false-positive risk; no multiplicity correction is applied.",
            "The trial count was fixed before listening. Optional stopping or choosing which sessions to report invalidates fixed-N interpretation.",
            "Listening times, switches, regions, and notes are client self-reports, not proof of listening or experimental compliance.",
            "Intervals are 95% Wilson score intervals, not exact binomial intervals; repeated listening can violate trial independence.",
        ],
        "trial_records": records,
        "provenance": state["provenance"],
    }
    if state["mode"] == "abx":
        correct = sum(record["correct"] for record in records)
        result.update(correct=correct, accuracy=correct / trials if trials else None,
                      test="One-sided exact binomial: P[Bin(n, 0.5) >= correct]. Wilson interval is for correct-response probability.")
        if complete:
            result.update(p_value=exact_abx(correct, trials), confidence_interval=wilson(correct, trials))
    else:
        counts = {"candidate_0": 0, "candidate_1": 0, "tie": 0}
        for record in records:
            key = "tie" if record["selected_candidate"] is None else f"candidate_{record['selected_candidate']}"
            counts[key] += 1
        decisive = counts["candidate_0"] + counts["candidate_1"]
        result.update(counts=counts, decisive_trials=decisive,
                      test="Two-sided exact binomial on decisive candidate choices only; ties excluded. Wilson interval is for candidate_0 preference among decisive votes.")
        if complete and decisive:
            result.update(p_value=exact_preference(counts["candidate_0"], counts["candidate_1"]),
                          confidence_interval=wilson(counts["candidate_0"], decisive))
        if not decisive:
            result["caveats"].append("There were no decisive preference votes, so no p-value or confidence interval is reported.")
    if not complete:
        result["caveats"].append("Stopped early: descriptive counts only; no fixed-N p-value or confidence interval is reported.")
    return result


def report_csv(public):
    """One row per answered trial; a zero-answer abort still exports its summary."""
    result = public["result"]
    summary = {
        "session_id": public["id"], "mode": public["mode"], "status": public["status"],
        "created_at": public["created_at"], "finished_at": public["finished_at"],
        "planned_trials": public["planned_trials"], "completed_trials": public["completed_trials"],
        "labels": json.dumps(result["labels"], ensure_ascii=False),
        "summary": json.dumps({key: value for key, value in result.items() if key not in ("trial_records", "provenance", "labels")}, ensure_ascii=False),
        "provenance": json.dumps(result["provenance"], ensure_ascii=False),
    }
    trial_fields = ["trial_id", "number", "response", "selected_candidate", "correct", "mapping", "loop_start", "loop_end", "note", "listened_seconds", "switches", "elapsed_seconds", "answered_at"]
    stream = io.StringIO(newline="")
    writer = csv.DictWriter(stream, fieldnames=[*summary, *trial_fields])
    writer.writeheader()
    for record in result["trial_records"] or [{}]:
        row = dict(summary)
        for field in trial_fields:
            value = record.get(field)
            row[field] = json.dumps(value, ensure_ascii=False) if isinstance(value, (dict, list)) else value
        writer.writerow(row)
    return stream.getvalue().encode("utf-8")


def finite_number(value, field, minimum=0.0, maximum=None):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise SessionError(400, f"{field} must be a finite number")
    try:
        valid = math.isfinite(value) and value >= minimum and (maximum is None or value <= maximum)
    except OverflowError:
        valid = False
    if not valid:
        raise SessionError(400, f"{field} is outside its allowed range")
    return float(value)


class SessionStore:
    """Disk is authoritative; the lock covers read/validate/replace transitions."""

    def __init__(self, output, config, labels, provenance):
        self.output = Path(output)
        self.config = copy.deepcopy(config)
        self.labels = list(labels)
        self.provenance = copy.deepcopy(provenance)
        self.private = self.output / "private"
        self.states = self.private / "sessions"
        self.results = self.output / "results"
        self.private.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.private.chmod(0o700)
        self.states.mkdir(exist_ok=True, mode=0o700)
        self.results.mkdir(exist_ok=True)
        self.lock = threading.RLock()
        pair_file = self.private / "pair.json"
        if pair_file.exists():
            saved = json.loads(pair_file.read_text(encoding="utf-8"))
            if saved.get("config", {}).get("pair_id") != config["pair_id"]:
                raise ValueError("Output directory belongs to a different audio pair; choose another --output directory")
        else:
            atomic_write(pair_file, json_bytes({"config": config, "labels": labels, "provenance": provenance}))
        # Recover reports if a process died after committing finished state.
        for path in self.states.glob("*.json"):
            state = self._load(path.stem)
            if state["status"] != "active":
                self._exports(state)

    def _load(self, session_id):
        if not isinstance(session_id, str) or not re.fullmatch(r"[0-9a-f]{32}", session_id):
            raise SessionError(404, "Session not found")
        try:
            state = json.loads((self.states / f"{session_id}.json").read_text(encoding="utf-8"))
        except FileNotFoundError:
            raise SessionError(404, "Session not found") from None
        if state.get("version") != 1 or state.get("id") != session_id:
            raise SessionError(500, "Stored session is not readable")
        if state.get("pair_id") != self.config["pair_id"]:
            raise SessionError(409, "This session belongs to a different audio pair")
        return state

    def _save(self, state):
        atomic_write(self.states / f"{state['id']}.json", json_bytes(state))

    def _trial(self, mode, number):
        first = secrets.randbelow(2)
        mapping = {"A": first, "B": 1 - first}
        if mode == "abx":
            mapping["X"] = secrets.randbelow(2)
        return {"id": secrets.token_hex(16), "number": number, "mapping": mapping}

    def _public(self, state):
        public = {key: state[key] for key in ("id", "mode", "planned_trials", "status", "created_at")}
        public["completed_trials"] = len(state["records"])
        if state["status"] == "active":
            trial = state["trial"]
            base = f"/api/sessions/{state['id']}/trials/{trial['id']}/audio"
            public["trial"] = {"id": trial["id"], "number": trial["number"],
                               "audio": {slot: f"{base}/{slot}.wav" for slot in trial["mapping"]}}
        else:
            public["finished_at"] = state["finished_at"]
            public["result"] = state["result"]
        return copy.deepcopy(public)

    def _exports(self, state):
        public = self._public(state)
        for suffix, encode in (("json", json_bytes), ("csv", report_csv)):
            path = self.results / f"{state['id']}.{suffix}"
            if not path.exists():
                atomic_write(path, encode(public))

    def create(self, payload):
        mode = payload.get("mode")
        planned = payload.get("planned_trials")
        if mode not in ("abx", "preference"):
            raise SessionError(400, "mode must be abx or preference")
        if type(planned) is not int or not 1 <= planned <= 1000:
            raise SessionError(400, "planned_trials must be an integer from 1 to 1000")
        with self.lock:
            state = {"version": 1, "id": secrets.token_hex(16), "pair_id": self.config["pair_id"],
                     "mode": mode, "planned_trials": planned, "created_at": utc_now(), "status": "active",
                     "labels": self.labels, "provenance": self.provenance, "records": [],
                     "trial": self._trial(mode, 1)}
            self._save(state)
            return self._public(state)

    def get(self, session_id):
        with self.lock:
            state = self._load(session_id)
            if state["status"] != "active":
                self._exports(state)
            return self._public(state)

    def answer(self, session_id, payload):
        with self.lock:
            state = self._load(session_id)
            if state["status"] != "active" or payload.get("trial_id") != state["trial"]["id"]:
                raise SessionError(409, "Trial is no longer awaiting an answer; reload this session")
            trial = state["trial"]
            response = payload.get("response")
            allowed = ("A", "B", "tie") if state["mode"] == "preference" else ("A", "B")
            if response not in allowed:
                raise SessionError(400, "Invalid response for this session mode")
            start = finite_number(payload.get("loop_start"), "loop_start", maximum=self.config["duration"])
            end = finite_number(payload.get("loop_end"), "loop_end", maximum=self.config["duration"])
            if end <= start:
                raise SessionError(400, "loop_end must be after loop_start")
            note = payload.get("note", "")
            if not isinstance(note, str) or len(note) > 10000:
                raise SessionError(400, "note must be a string of at most 10000 characters")
            listened = payload.get("listened_seconds", {})
            if not isinstance(listened, dict) or any(key not in ("A", "B", "X") for key in listened):
                raise SessionError(400, "listened_seconds must contain only A, B, and X durations")
            listened = {slot: finite_number(listened.get(slot, 0), f"listened_seconds.{slot}") for slot in ("A", "B", "X")}
            switches = payload.get("switches", 0)
            if type(switches) is not int or switches < 0 or switches > (1 << 53) - 1:
                raise SessionError(400, "switches must be a nonnegative safe integer")
            elapsed = finite_number(payload.get("elapsed_seconds", 0), "elapsed_seconds")
            selected = None if response == "tie" else trial["mapping"][response]
            record = {"trial_id": trial["id"], "number": trial["number"], "mapping": trial["mapping"],
                      "response": response, "selected_candidate": selected,
                      "correct": selected == trial["mapping"]["X"] if state["mode"] == "abx" else None,
                      "loop_start": start, "loop_end": end, "note": note, "listened_seconds": listened,
                      "switches": switches, "elapsed_seconds": elapsed, "answered_at": utc_now()}
            state["records"].append(record)
            if len(state["records"]) == state["planned_trials"]:
                self._end(state, "completed")
            else:
                state["trial"] = self._trial(state["mode"], len(state["records"]) + 1)
                self._save(state)
            return self._public(state)

    def _end(self, state, status):
        state["status"] = status
        state["finished_at"] = utc_now()
        state["trial"] = None
        state["result"] = report_result(state)
        self._save(state)
        self._exports(state)

    def finish(self, session_id):
        with self.lock:
            state = self._load(session_id)
            if state["status"] != "active":
                raise SessionError(409, "Session has already finished")
            self._end(state, "aborted")
            return self._public(state)

    def media_candidate(self, session_id, trial_id, slot):
        with self.lock:
            state = self._load(session_id)
            trial = state["trial"]
            if state["status"] != "active" or trial_id != trial["id"] or slot not in trial["mapping"]:
                raise SessionError(404, "Trial audio not found")
            return trial["mapping"][slot]

    def export(self, session_id, suffix):
        if suffix not in ("json", "csv"):
            raise SessionError(404, "Export not found")
        with self.lock:
            state = self._load(session_id)
            if state["status"] == "active":
                raise SessionError(403, "Reports are available only after the session finishes")
            self._exports(state)
            return self.results / f"{session_id}.{suffix}"
