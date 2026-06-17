#!/usr/bin/env python3
"""Scripted bridge-state agent for the T5.5 exploratory E2E layer.

This module intentionally uses only the Python standard library. It reads a
natural-language goal/scenario, queries the receiver test bridge, and emits
human-triage artifacts. Its findings are advisory; the backend assertions in
``scripts/e2e/run_stack.py`` remain the hard gate.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class BridgeScenario:
    goal: str
    expected: dict[str, Any]


def load_scenario(path: Path | str) -> BridgeScenario:
    scenario_path = Path(path)
    data = json.loads(scenario_path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("scenario must be a JSON object")
    goal = data.get("goal")
    if not isinstance(goal, str) or not goal.strip():
        raise ValueError("scenario must contain a non-empty string `goal`")
    expected = data.get("expected")
    if not isinstance(expected, dict):
        raise ValueError("scenario must contain an object `expected`")
    return BridgeScenario(goal=goal, expected=expected)


def evaluate_bridge_state(
    scenario: BridgeScenario,
    state: dict[str, Any],
    *,
    expected_stream_id: str | None = None,
    expected_local_port: int | None = None,
    invoked_status: dict[str, Any] | None = None,
) -> dict[str, Any]:
    expected = scenario.expected
    status = _dict(state.get("status"))
    streams_container = _dict(state.get("streams"))
    streams = streams_container.get("streams", [])
    if not isinstance(streams, list):
        streams = []

    stream_id = expected_stream_id or _str_or_none(expected.get("stream_id"))
    target = _find_stream(streams, stream_id)

    checks: list[dict[str, Any]] = []

    expected_streams_count = expected.get("streams_count")
    if expected_streams_count is not None:
        _add_check(
            checks,
            f"status streams_count == {expected_streams_count}",
            status.get("streams_count") == expected_streams_count,
            actual=status.get("streams_count"),
            expected=expected_streams_count,
        )

    _add_check(
        checks,
        "expected stream is present",
        target is not None,
        actual=target.get("stream_id") if target else None,
        expected=stream_id or "single configured stream",
    )

    expected_subscribed = expected.get("subscribed", True)
    _add_check(
        checks,
        "expected stream is subscribed",
        target is not None and target.get("subscribed") == expected_subscribed,
        actual=target.get("subscribed") if target else None,
        expected=expected_subscribed,
    )

    expected_cursor_seq = expected.get("cursor_seq")
    if expected_cursor_seq is not None:
        _add_check(
            checks,
            f"cursor_seq == {expected_cursor_seq}",
            target is not None and target.get("cursor_seq") == expected_cursor_seq,
            actual=target.get("cursor_seq") if target else None,
            expected=expected_cursor_seq,
        )

    scenario_port = expected.get("local_port")
    port = expected_local_port if expected_local_port is not None else scenario_port
    if isinstance(port, int) and not isinstance(port, bool):
        _add_check(
            checks,
            f"local proxy port == {port}",
            target is not None and target.get("local_port") == port,
            actual=target.get("local_port") if target else None,
            expected=port,
        )
    else:
        actual_port = target.get("local_port") if target else None
        _add_check(
            checks,
            "local proxy port is present",
            isinstance(actual_port, int) and actual_port > 0,
            actual=actual_port,
            expected="positive integer",
        )

    passed = all(check["ok"] for check in checks)
    findings: dict[str, Any] = {
        "completed": True,
        "passed": passed,
        "goal": scenario.goal,
        "summary": "Agent judgment passed" if passed else "Agent judgment found mismatches",
        "checks": checks,
    }
    if invoked_status is not None:
        findings["invoked_status"] = invoked_status
    return findings


def run_bridge_goal(
    *,
    bridge_base_url: str,
    scenario_path: Path | str,
    artifacts_dir: Path | str,
    expected_stream_id: str | None = None,
    expected_local_port: int | None = None,
) -> dict[str, Any]:
    artifacts = Path(artifacts_dir)
    artifacts.mkdir(parents=True, exist_ok=True)

    scenario = load_scenario(scenario_path)
    (artifacts / "goal.md").write_text(_goal_markdown(scenario), encoding="utf-8")

    transcript: list[str] = [
        "# Bridge Agent Transcript",
        "",
        f"Goal: {scenario.goal}",
        "",
        f"Bridge: {bridge_base_url}",
        "",
    ]

    try:
        transcript.append("1. GET /bridge/state")
        state = _http_json(f"{bridge_base_url.rstrip('/')}/bridge/state")
        _write_json(artifacts / "state.json", state)

        invoked_status = None
        try:
            transcript.append("2. POST /bridge/invoke/get_status")
            invoked_status = _http_json(
                f"{bridge_base_url.rstrip('/')}/bridge/invoke/get_status",
                method="POST",
                body=b"{}",
            )
        except Exception as exc:  # noqa: BLE001 - optional diagnostic only
            transcript.append(f"   Optional get_status invoke failed: {type(exc).__name__}: {exc}")

        transcript.append("3. Evaluate bridge state against scripted expectations")
        findings = evaluate_bridge_state(
            scenario,
            state,
            expected_stream_id=expected_stream_id,
            expected_local_port=expected_local_port,
            invoked_status=invoked_status,
        )
    except Exception as exc:  # noqa: BLE001 - artifact mode must not gate backend assertions
        findings = {
            "completed": False,
            "passed": False,
            "goal": scenario.goal,
            "summary": "Agent did not complete bridge inspection",
            "error": f"{type(exc).__name__}: {exc}",
            "checks": [],
        }
        _write_json(artifacts / "state.json", {"error": findings["error"]})
        transcript.append(f"Agent failed before completion: {findings['error']}")

    _write_json(artifacts / "findings.json", findings)
    transcript.append("")
    transcript.append(f"Completed: {str(findings['completed']).lower()}")
    transcript.append(f"Agent judgment passed: {str(findings['passed']).lower()}")
    (artifacts / "transcript.md").write_text("\n".join(transcript) + "\n", encoding="utf-8")
    return findings


def _find_stream(streams: list[Any], stream_id: str | None) -> dict[str, Any] | None:
    dict_streams = [s for s in streams if isinstance(s, dict)]
    if stream_id is not None:
        for stream in dict_streams:
            if stream.get("stream_id") == stream_id:
                return stream
        return None
    if len(dict_streams) == 1:
        return dict_streams[0]
    return None


def _add_check(
    checks: list[dict[str, Any]],
    name: str,
    ok: bool,
    *,
    actual: Any,
    expected: Any,
) -> None:
    checks.append({"name": name, "ok": bool(ok), "actual": actual, "expected": expected})


def _http_json(url: str, *, method: str = "GET", body: bytes | None = None) -> dict[str, Any]:
    headers = {"Content-Type": "application/json"} if body is not None else {}
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            data = json.loads(response.read().decode())
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"{method} {url} returned HTTP {exc.code}") from exc
    if not isinstance(data, dict):
        raise RuntimeError(f"{method} {url} did not return a JSON object")
    return data


def _write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _goal_markdown(scenario: BridgeScenario) -> str:
    return (
        "# Bridge Agent Goal\n\n"
        f"{scenario.goal}\n\n"
        "## Expected\n\n"
        "```json\n"
        f"{json.dumps(scenario.expected, indent=2, sort_keys=True)}\n"
        "```\n"
    )


def _dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _str_or_none(value: Any) -> str | None:
    return value if isinstance(value, str) else None
