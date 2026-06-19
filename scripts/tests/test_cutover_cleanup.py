import re
import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class CutoverCleanupTests(unittest.TestCase):
    def test_legacy_rt_protocol_is_removed_from_workspace(self) -> None:
        # The central "server" component name is reused by the current P2P
        # registry service (services/server); only the legacy WebSocket data
        # plane (rt-protocol) stays guarded.
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())

        self.assertNotIn("crates/rt-protocol", workspace["workspace"]["members"])
        self.assertFalse((ROOT / "crates/rt-protocol").exists())

    def test_workspace_has_no_postgres_or_ws_data_plane_dependencies(self) -> None:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
        dev_dependencies = workspace.get("dev-dependencies", {})

        self.assertNotIn("rt-protocol", dev_dependencies)
        self.assertNotIn("sqlx", dev_dependencies)
        self.assertNotIn("testcontainers", dev_dependencies)
        self.assertNotIn("testcontainers-modules", dev_dependencies)

        for manifest in ROOT.glob("**/Cargo.toml"):
            if "target" in manifest.parts:
                continue
            text = manifest.read_text()
            self.assertNotIn("rt-protocol", text, f"stale rt-protocol reference in {manifest}")
            self.assertNotIn("tokio-tungstenite", text, f"stale websocket dependency in {manifest}")
            self.assertNotIn("postgres", text.lower(), f"stale postgres dependency in {manifest}")
            self.assertNotIn("testcontainers", text, f"stale Docker test dependency in {manifest}")

    def test_legacy_deploy_paths_are_removed(self) -> None:
        self.assertFalse((ROOT / "deploy/quickstart").exists())

        package = tomllib.loads((ROOT / "pyproject.toml").read_text()) if (ROOT / "pyproject.toml").exists() else None
        self.assertIsNone(package, "pyproject is not expected in this Node workspace")

    def test_docs_are_cutover_to_p2p_server_architecture(self) -> None:
        docs_readme = (ROOT / "docs/README.md").read_text()
        agent_notes = (ROOT / "AGENTS.md").read_text()

        for text, label in [(docs_readme, "docs/README.md"), (agent_notes, "AGENTS.md")]:
            self.assertIn("server", text, label)
            self.assertIn("P2P", text, label)
            self.assertNotIn("Postgres", text, label)
            self.assertNotIn("WebSocket", text, label)
            self.assertNotIn("rt-protocol", text, label)

    def test_docs_use_current_e2e_cli(self) -> None:
        for rel_path in [
            "README.md",
            "AGENTS.md",
            "CONTRIBUTING.md",
            "docs/README.md",
            "docs/local-testing.md",
            "scripts/README.md",
        ]:
            text = (ROOT / rel_path).read_text()
            self.assertIn("uv run scripts/e2e/run_stack.py", text, rel_path)
            for stale_flag in ["--assert", "--keep-artifacts", "--no-ui-agent"]:
                self.assertNotIn(stale_flag, text, rel_path)
            self.assertIsNone(re.search(r"--power-loss(?![-\w])", text), rel_path)

    def test_docs_do_not_reference_removed_dev_helper(self) -> None:
        for path in [
            ROOT / "README.md",
            ROOT / "AGENTS.md",
            ROOT / "CONTRIBUTING.md",
            *ROOT.glob("docs/**/*.md"),
            *ROOT.glob("scripts/**/*.md"),
        ]:
            if "plans" in path.parts:
                continue
            self.assertNotIn("scripts/dev.py", path.read_text(), str(path.relative_to(ROOT)))

    def test_ci_runs_cutover_guards(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        ci_lines = [line.strip() for line in ci.splitlines()]

        unittest_index = ci_lines.index("python -m unittest")
        self.assertEqual(
            [
                "scripts/tests/test_cutover_cleanup.py",
                "scripts/tests/test_release.py",
                "scripts/tests/test_sbc_cloud_init.py",
            ],
            ci_lines[unittest_index + 1 : unittest_index + 4],
        )
        self.assertIn("run: bash scripts/validate-packaging.sh", ci_lines)

        raw_lines = ci.splitlines()
        for event in ["push", "pull_request"]:
            event_index = raw_lines.index(f"  {event}:")
            event_block = []
            for line in raw_lines[event_index + 1 :]:
                if line.startswith("  ") and not line.startswith("    ") and line.strip().endswith(":"):
                    break
                event_block.append(line.strip())
            self.assertIn('- "**/*.md"', event_block)
            self.assertIn('- "scripts/tests/**"', event_block)
            self.assertIn('- "scripts/validate-packaging.sh"', event_block)

    def test_receiver_ui_removed_central_server_tabs_and_commands(self) -> None:
        for rel_path in [
            "apps/receiver-ui/src/lib/components/ForwardersTab.svelte",
            "apps/receiver-ui/src/lib/components/ForwardersTab.test.ts",
            "apps/receiver-ui/src/lib/components/RacesTab.svelte",
            "apps/receiver-ui/src/lib/components/AnnouncerTab.svelte",
            "apps/shared-ui/src/components/AnnouncerConfigForm.svelte",
            "apps/shared-ui/src/lib/announcer-types.ts",
            "apps/shared-ui/src/lib/help/server-help.ts",
        ]:
            self.assertFalse((ROOT / rel_path).exists(), rel_path)

        receiver_api = (ROOT / "apps/receiver-ui/src/lib/api.ts").read_text()
        receiver_store = (ROOT / "apps/receiver-ui/src/lib/store.svelte.ts").read_text()
        streams_tab = (ROOT / "apps/receiver-ui/src/lib/components/StreamsTab.svelte").read_text()
        receiver_sse = (ROOT / "apps/receiver-ui/src/lib/sse.ts").read_text()
        receiver_ui_events = (ROOT / "services/receiver/src/ui_events.rs").read_text()
        receiver_registry = (ROOT / "services/receiver/src/control_api.rs").read_text()
        bridge = (ROOT / "services/receiver/src/control_bridge.rs").read_text()
        tauri_main = (ROOT / "apps/receiver-ui/src-tauri/src/main.rs").read_text()
        for forbidden in [
            "getForwarders",
            "getRaces",
            "getServerStreams",
            "getAnnouncerConfig",
            "putAnnouncerConfig",
            "resetAnnouncer",
            "readerGetInfo",
            "readerSyncClock",
            "readerSetReadMode",
            "readerSetTto",
            "readerSetRecording",
            "readerClearRecords",
            "readerStartDownload",
            "readerStopDownload",
            "readerRefresh",
            "readerReconnect",
        ]:
            self.assertNotIn(forbidden, receiver_api)
            self.assertNotIn(forbidden, receiver_store)
            self.assertNotIn(forbidden, streams_tab)
        legacy_receiver_commands = [
            "get_races",
            "create_race",
            "delete_race",
            "get_participants",
            "upload_race_file",
            "get_forwarders",
            "get_forwarder_race",
            "set_forwarder_race",
            "get_forwarder_config",
            "set_forwarder_config",
            "restart_forwarder_service",
            "restart_forwarder_device",
            "shutdown_forwarder_device",
            "get_server_streams",
            "get_announcer_config",
            "put_announcer_config",
            "reset_announcer",
            "reader_get_info",
            "reader_sync_clock",
            "reader_set_read_mode",
            "reader_set_tto",
            "reader_set_recording",
            "reader_clear_records",
            "reader_start_download",
            "reader_stop_download",
            "reader_refresh",
            "reader_reconnect",
        ]
        for forbidden in legacy_receiver_commands:
            self.assertNotIn(forbidden, receiver_registry)
            self.assertNotIn(forbidden, bridge)
            self.assertNotIn(forbidden, tauri_main)
        self.assertNotIn("legacy_server_removed", receiver_registry)
        self.assertNotIn("ReaderControlPanel", streams_tab)
        for forbidden in [
            "ReaderInfoUpdated",
            "ReaderDownloadProgress",
            "reader_info_updated",
            "reader_download_progress",
            "onReaderInfoUpdated",
            "onReaderDownloadProgress",
            "readerInfos",
            "readerStates",
            "downloadProgress",
            "export type ReaderConnectionState",
            "export type DownloadState",
            "export interface ReaderInfo",
        ]:
            self.assertNotIn(forbidden, receiver_api)
            self.assertNotIn(forbidden, receiver_store)
            self.assertNotIn(forbidden, receiver_sse)
            self.assertNotIn(forbidden, receiver_ui_events)
            self.assertNotIn(forbidden, receiver_registry)
            self.assertNotIn(forbidden, bridge)
            self.assertNotIn(forbidden, tauri_main)
