"""Focused experiment invariants using synthetic sessions, never musical fixtures."""

import csv
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from sessions import SessionError, SessionStore, exact_abx, exact_preference, wilson


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.output = Path(self.temporary.name)
        self.config = {"pair_id": "synthetic-pair", "title": "Blind comparison", "duration": 10.0,
                       "sample_rate": 48000, "channels": 2, "level_matched": True}
        self.labels = ["Hidden room, one", 'Hidden room "two"']
        self.provenance = {"source": "/private/synthetic.wav", "gain_db": -2.5}
        self.store = self.reopen()

    def reopen(self):
        return SessionStore(self.output, self.config, self.labels, self.provenance)

    def payload(self, public, response="A", **observations):
        return {"trial_id": public["trial"]["id"], "response": response,
                "loop_start": 1.25, "loop_end": 3.0, **observations}

    def choose_candidate(self, public, candidate):
        trial = public["trial"]
        return next(slot for slot in ("A", "B")
                    if self.store.media_candidate(public["id"], trial["id"], slot) == candidate)

    def abx_answer(self, public, correct=True):
        trial = public["trial"]
        candidate = self.store.media_candidate(public["id"], trial["id"], "X")
        response = self.choose_candidate(public, candidate if correct else 1 - candidate)
        return self.store.answer(public["id"], self.payload(public, response))

    def test_no_live_unblinding_or_export_before_last_answer(self):
        public = self.store.create({"mode": "abx", "planned_trials": 2})
        public = self.abx_answer(public)
        encoded = json.dumps(public)
        for secret in [*self.labels, self.provenance["source"], "mapping", "correct", "accuracy", "p_value", "provenance", "trial_records"]:
            self.assertNotIn(secret, encoded)
        self.assertEqual(public["completed_trials"], 1)
        for suffix in ("json", "csv"):
            with self.assertRaises(SessionError) as raised:
                self.store.export(public["id"], suffix)
            self.assertEqual(raised.exception.status, 403)
        finished = self.abx_answer(public)
        self.assertEqual(finished["status"], "completed")
        self.assertNotIn("trial", finished)
        self.assertEqual(finished["result"]["labels"], self.labels)
        self.assertEqual(finished["result"]["provenance"], self.provenance)
        self.assertEqual(finished["result"]["correct"], 2)
        exported = json.loads(self.store.export(public["id"], "json").read_text())
        self.assertEqual(exported, finished)

    def test_duplicate_and_stale_answers_do_not_change_pending_trial(self):
        public = self.store.create({"mode": "abx", "planned_trials": 2})
        payload = self.payload(public)
        payload["planned_trials"] = 1
        advanced = self.store.answer(public["id"], payload)
        self.assertEqual(advanced["planned_trials"], 2)
        for trial_id in (payload["trial_id"], "0" * 32):
            with self.assertRaises(SessionError) as raised:
                self.store.answer(public["id"], {**payload, "trial_id": trial_id})
            self.assertEqual(raised.exception.status, 409)
            self.assertEqual(self.store.get(public["id"]), advanced)
        finished = self.abx_answer(advanced)
        with self.assertRaises(SessionError) as raised:
            self.store.answer(public["id"], self.payload(advanced))
        self.assertEqual(raised.exception.status, 409)
        self.assertEqual(self.store.get(public["id"]), finished)

    def test_restart_preserves_pending_audio_assignment_and_prior_answer(self):
        public = self.store.create({"mode": "abx", "planned_trials": 3})
        public = self.abx_answer(public, correct=False)
        trial_id = public["trial"]["id"]
        assignment = {slot: self.store.media_candidate(public["id"], trial_id, slot) for slot in ("A", "B", "X")}
        self.store = self.reopen()
        self.assertEqual(self.store.get(public["id"]), public)
        self.assertEqual({slot: self.store.media_candidate(public["id"], trial_id, slot) for slot in ("A", "B", "X")}, assignment)
        public = self.abx_answer(public)
        finished = self.abx_answer(public)
        self.assertEqual(finished["result"]["correct"], 2)
        self.assertEqual(finished["result"]["trials"], 3)
        self.assertFalse(finished["result"]["trial_records"][0]["correct"])

    def test_fixed_count_reports_inference_but_abort_only_describes(self):
        full = self.store.create({"mode": "abx", "planned_trials": 5})
        for correct in (True, False, True, True, True):
            full = self.abx_answer(full, correct)
        self.assertTrue(full["result"]["completed_as_planned"])
        self.assertEqual(full["result"]["p_value"], 6 / 32)
        low, high = full["result"]["confidence_interval"]
        self.assertLess(low, 0.8)
        self.assertGreater(high, 0.8)
        stopped = self.store.create({"mode": "abx", "planned_trials": 5})
        for _ in range(4):
            stopped = self.abx_answer(stopped)
        stopped = self.store.finish(stopped["id"])
        self.assertEqual(stopped["status"], "aborted")
        self.assertEqual(stopped["result"]["correct"], 4)
        self.assertEqual(stopped["result"]["accuracy"], 1)
        self.assertFalse(stopped["result"]["completed_as_planned"])
        self.assertIsNone(stopped["result"]["p_value"])
        self.assertIsNone(stopped["result"]["confidence_interval"])

    def test_preference_counts_candidates_not_visible_slots_and_excludes_ties(self):
        # Visible A represents opposite candidates on successive trials.
        with patch("sessions.secrets.randbelow", side_effect=[0, 1, 0, 1, 1, 0, 0]):
            public = self.store.create({"mode": "preference", "planned_trials": 7})
            visible_choices = []
            for selected in (0, 0, 0, 0, 0, 1, None):
                response = "tie" if selected is None else self.choose_candidate(public, selected)
                visible_choices.append(response)
                public = self.store.answer(public["id"], self.payload(public, response))
        self.assertIn("A", visible_choices)
        self.assertIn("B", visible_choices)
        result = public["result"]
        self.assertEqual(result["counts"], {"candidate_0": 5, "candidate_1": 1, "tie": 1})
        self.assertEqual(result["decisive_trials"], 6)
        self.assertEqual(result["p_value"], 14 / 64)
        self.assertEqual(result["confidence_interval"], wilson(5, 6))
        self.assertIsNone(result["correct"])

    def test_all_ties_and_zero_answer_abort_have_no_inference(self):
        public = self.store.create({"mode": "preference", "planned_trials": 2})
        public = self.store.answer(public["id"], self.payload(public, "tie"))
        public = self.store.answer(public["id"], self.payload(public, "tie"))
        self.assertEqual(public["result"]["counts"], {"candidate_0": 0, "candidate_1": 0, "tie": 2})
        self.assertEqual(public["result"]["decisive_trials"], 0)
        self.assertIsNone(public["result"]["p_value"])
        self.assertIsNone(public["result"]["confidence_interval"])
        empty = self.store.create({"mode": "abx", "planned_trials": 10})
        empty = self.store.finish(empty["id"])
        self.assertEqual(empty["result"]["trials"], 0)
        self.assertIsNone(empty["result"]["accuracy"])
        rows = list(csv.DictReader(io.StringIO(self.store.export(empty["id"], "csv").read_text())))
        self.assertEqual(len(rows), 1)
        self.assertEqual(json.loads(rows[0]["summary"])["trials"], 0)

    def test_invalid_observations_leave_answer_retryable(self):
        public = self.store.create({"mode": "abx", "planned_trials": 1})
        invalid = [self.payload(public, loop_end=0.5),
                   self.payload(public, listened_seconds={"A": float("nan")}),
                   self.payload(public, switches=True),
                   self.payload(public, "tie")]
        for payload in invalid:
            with self.assertRaises(SessionError) as raised:
                self.store.answer(public["id"], payload)
            self.assertEqual(raised.exception.status, 400)
            self.assertEqual(self.store.get(public["id"]), public)
        finished = self.abx_answer(public)
        self.assertEqual(finished["completed_trials"], 1)

    def test_csv_round_trips_quotes_newlines_and_private_provenance_after_finish(self):
        public = self.store.create({"mode": "preference", "planned_trials": 1})
        note = 'First, "wide" room\nSecond line: café'
        public = self.store.answer(public["id"], self.payload(public, "tie", note=note))
        path = self.output / "results" / f"{public['id']}.csv"
        with path.open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        self.assertEqual(rows[0]["note"], note)
        self.assertEqual(json.loads(rows[0]["labels"]), self.labels)
        self.assertEqual(json.loads(rows[0]["provenance"]), self.provenance)
        self.assertEqual(json.loads(rows[0]["mapping"]), public["result"]["trial_records"][0]["mapping"])

    def test_different_pair_cannot_resume_an_existing_output(self):
        public = self.store.create({"mode": "abx", "planned_trials": 3})
        with self.assertRaises(ValueError):
            SessionStore(self.output, {**self.config, "pair_id": "different-pair"}, self.labels, self.provenance)
        self.assertEqual(self.reopen().get(public["id"]), public)


class StatisticsTests(unittest.TestCase):
    def test_exact_tails_include_observed_outcome_and_handle_extremes(self):
        self.assertEqual(exact_abx(4, 5), 6 / 32)
        self.assertEqual(exact_abx(0, 1000), 1)
        self.assertEqual(exact_abx(1000, 1000), 2.0 ** -1000)
        self.assertEqual(exact_preference(5, 1), 14 / 64)
        self.assertEqual(exact_preference(1, 5), 14 / 64)
        self.assertEqual(exact_preference(5, 5), 1)
        self.assertEqual(exact_preference(1000, 0), 2.0 ** -999)
        self.assertIsNone(exact_preference(0, 0))

    def test_wilson_interval_is_bounded_at_zero_and_one(self):
        low, high = wilson(5, 10)
        self.assertAlmostEqual(low, 0.2365930905)
        self.assertAlmostEqual(high, 0.7634069095)
        low, high = wilson(0, 10)
        self.assertAlmostEqual(low, 0)
        self.assertGreater(high, 0)
        low, high = wilson(10, 10)
        self.assertLess(low, 1)
        self.assertAlmostEqual(high, 1)
        self.assertIsNone(wilson(0, 0))


if __name__ == "__main__":
    unittest.main()
