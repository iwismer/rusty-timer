import json
import tempfile
import unittest
from pathlib import Path

try:
    from scripts.e2e.agent_ui import bridge_agent
except ImportError as exc:  # pragma: no cover - exercised in RED before implementation
    bridge_agent = None
    IMPORT_ERROR = exc
else:
    IMPORT_ERROR = None


class BridgeAgentTests(unittest.TestCase):
    def setUp(self) -> None:
        if bridge_agent is None:
            self.fail(f"bridge_agent module is missing: {IMPORT_ERROR}")

    def test_load_scenario_requires_goal_and_expected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scenario.json"
            path.write_text(json.dumps({"goal": "Inspect state", "expected": {"cursor_seq": 8}}))

            scenario = bridge_agent.load_scenario(path)

        self.assertEqual(scenario.goal, "Inspect state")
        self.assertEqual(scenario.expected["cursor_seq"], 8)

    def test_findings_pass_when_bridge_state_matches_expected_stream(self) -> None:
        scenario = bridge_agent.BridgeScenario(
            goal="Inspect receiver bridge state",
            expected={"cursor_seq": 8, "streams_count": 1, "subscribed": True},
        )
        state = {
            "status": {
                "connection_state": "connected",
                "local_ok": True,
                "streams_count": 1,
                "receiver_id": "rx-e2e",
            },
            "streams": {
                "streams": [
                    {
                        "stream_id": "127.0.0.1:50001",
                        "subscribed": True,
                        "local_port": 50123,
                        "cursor_seq": 8,
                    }
                ],
                "degraded": False,
                "upstream_error": None,
            },
        }

        findings = bridge_agent.evaluate_bridge_state(
            scenario,
            state,
            expected_stream_id="127.0.0.1:50001",
            expected_local_port=50123,
        )

        self.assertTrue(findings["completed"])
        self.assertTrue(findings["passed"])
        self.assertEqual([check["name"] for check in findings["checks"]], [
            "status streams_count == 1",
            "expected stream is present",
            "expected stream is subscribed",
            "cursor_seq == 8",
            "local proxy port == 50123",
        ])

    def test_findings_report_failed_agent_judgment_without_incomplete_run(self) -> None:
        scenario = bridge_agent.BridgeScenario(
            goal="Inspect receiver bridge state",
            expected={"cursor_seq": 8, "streams_count": 1, "subscribed": True},
        )
        state = {
            "status": {"streams_count": 1},
            "streams": {"streams": [{"stream_id": "s1", "subscribed": True, "local_port": 50123, "cursor_seq": 7}]},
        }

        findings = bridge_agent.evaluate_bridge_state(scenario, state, expected_stream_id="s1")

        self.assertTrue(findings["completed"])
        self.assertFalse(findings["passed"])
        failed = [check for check in findings["checks"] if not check["ok"]]
        self.assertEqual(failed[0]["name"], "cursor_seq == 8")


if __name__ == "__main__":
    unittest.main()
