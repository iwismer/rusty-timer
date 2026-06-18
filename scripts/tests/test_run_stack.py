"""Unit tests for the deterministic full-stack E2E orchestrator helpers.

These cover the pure, process-free logic that T6.1 relies on:

* enumeration of both power-loss targets (receiver and forwarder), so the
  one-command run exercises both SIGKILL lanes by default;
* reading durable forwarder journal progress, so the forwarder lane can
  SIGKILL only after ``0 < count_at_kill < NUM_READS`` events are durable.
"""

import sqlite3
import tempfile
import unittest
from pathlib import Path

from scripts.e2e import run_stack

EVENTS_DDL = """
CREATE TABLE events (
    stream_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    raw_frame BLOB NOT NULL,
    read_kind TEXT NOT NULL,
    reader_timestamp TEXT,
    received_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (stream_id, seq)
);
"""


def _make_journal(path: Path, stream_id: str, n: int) -> None:
    conn = sqlite3.connect(str(path))
    try:
        conn.executescript(EVENTS_DDL)
        for seq in range(1, n + 1):
            conn.execute(
                "INSERT INTO events VALUES (?,?,?,?,?,?,?)",
                (stream_id, seq, 1, b"frame", "raw", None, 0),
            )
        conn.commit()
    finally:
        conn.close()


class PowerLossTargetTests(unittest.TestCase):
    def test_default_enumerates_both_targets(self) -> None:
        self.assertEqual(
            run_stack.resolve_power_loss_targets("both"),
            ["receiver", "forwarder"],
        )

    def test_single_targets(self) -> None:
        self.assertEqual(run_stack.resolve_power_loss_targets("receiver"), ["receiver"])
        self.assertEqual(run_stack.resolve_power_loss_targets("forwarder"), ["forwarder"])

    def test_invalid_target_raises(self) -> None:
        with self.assertRaises(ValueError):
            run_stack.resolve_power_loss_targets("nope")


class ForwarderJournalCountTests(unittest.TestCase):
    def test_counts_events_for_stream(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "forwarder.sqlite3"
            _make_journal(journal, "127.0.0.1:9999", 3)
            self.assertEqual(run_stack.forwarder_event_count(journal, "127.0.0.1:9999"), 3)

    def test_missing_journal_is_zero(self) -> None:
        self.assertEqual(
            run_stack.forwarder_event_count(Path("/nonexistent/forwarder.sqlite3"), "x"),
            0,
        )

    def test_partial_count_within_range(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "forwarder.sqlite3"
            _make_journal(journal, "s", 3)
            self.assertEqual(run_stack.partial_forwarder_count(journal, "s"), 3)

    def test_partial_count_zero_is_false(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "forwarder.sqlite3"
            _make_journal(journal, "s", 0)
            self.assertFalse(run_stack.partial_forwarder_count(journal, "s"))

    def test_partial_count_complete_is_false(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            journal = Path(tmp) / "forwarder.sqlite3"
            _make_journal(journal, "s", run_stack.NUM_READS)
            self.assertFalse(run_stack.partial_forwarder_count(journal, "s"))


if __name__ == "__main__":
    unittest.main()
