import argparse
import subprocess
import unittest
from unittest.mock import patch

import scripts.release as release


class VersionUpdateTests(unittest.TestCase):
    def test_update_package_version_handles_authors_before_version(self) -> None:
        cargo_toml = (
            "[package]\n"
            "name = \"streamer\"\n"
            "authors = [\"iwismer <isaac@iwismer.ca>\"]\n"
            "version = \"1.2.3\"\n"
            "edition = \"2021\"\n"
            "\n"
            "[dependencies]\n"
            "tokio = \"1\"\n"
        )

        updated = release.update_package_version(cargo_toml, "1.2.4")

        self.assertIn('version = "1.2.4"', updated)
        self.assertNotIn('version = "1.2.3"', updated)

    def test_update_tauri_conf_version_replaces_root_semver(self) -> None:
        raw = (
            '{\n  "productName": "Rusty Timer Receiver",\n'
            '  "version": "0.9.0",\n  "build": {}\n}\n'
        )
        updated = release.update_tauri_conf_version(raw, "1.0.0")
        self.assertIn('"version": "1.0.0"', updated)
        self.assertNotIn('"version": "0.9.0"', updated)


class OutputStyleTests(unittest.TestCase):
    def test_style_wraps_text_with_ansi_codes_when_color_enabled(self) -> None:
        styled = release.style("hello", role="step", color_enabled=True)
        self.assertTrue(styled.startswith("\x1b["))
        self.assertIn("hello", styled)
        self.assertTrue(styled.endswith("\x1b[0m"))

    def test_style_returns_plain_text_when_color_disabled(self) -> None:
        styled = release.style("hello", role="step", color_enabled=False)
        self.assertEqual(styled, "hello")


class TransactionTests(unittest.TestCase):
    @patch("scripts.release.write_receiver_tauri_version")
    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_rolls_back_commits_and_tags_when_later_service_fails(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
        _write_tauri_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["forwarder", "receiver"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.side_effect = ["0.1.0", "0.1.0"]
        compute_new_version_mock.side_effect = ["0.1.1", "0.1.1"]

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            if cmd == [
                "cargo",
                "build",
                "--release",
                "--package",
                "receiver",
                "--bin",
                "receiver",
                "--features",
                "embed-ui",
            ]:
                raise subprocess.CalledProcessError(1, cmd, stderr="boom")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            with self.assertRaises(SystemExit):
                release.main()

        self.assertIn(["git", "tag", "-d", "forwarder-v0.1.1"], calls)
        self.assertIn(["git", "reset", "--hard", "abc123"], calls)


class PushTests(unittest.TestCase):
    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_pushes_branch_then_each_tag_separately(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["forwarder"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertIn(
            ["git", "add", "services/forwarder/Cargo.toml", "Cargo.lock"],
            calls,
        )
        self.assertIn(["git", "push", "origin", "master"], calls)
        self.assertIn(["git", "push", "origin", "forwarder-v0.1.1"], calls)


class ReleaseWorkflowParityTests(unittest.TestCase):
    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_forwarder_runs_ui_checks_and_embed_ui_release_build(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["forwarder"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertIn(["npm", "ci"], calls)
        self.assertIn(
            ["npm", "run", "lint", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "run", "check", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "test", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            [
                "cargo",
                "build",
                "--release",
                "--package",
                "forwarder",
                "--bin",
                "forwarder",
                "--features",
                "embed-ui",
            ],
            calls,
        )

    @patch("scripts.release.write_receiver_tauri_version")
    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_receiver_stages_tauri_conf_and_pushes_receiver_ui_tag(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
        _write_tauri_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["receiver"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertIn(
            [
                "git",
                "add",
                "services/receiver/Cargo.toml",
                "Cargo.lock",
                "apps/receiver-ui/src-tauri/tauri.conf.json",
            ],
            calls,
        )
        self.assertIn(["git", "tag", "receiver-v0.1.1"], calls)
        self.assertIn(["git", "tag", "receiver-ui-v0.1.1"], calls)
        self.assertIn(["git", "push", "origin", "receiver-v0.1.1"], calls)
        self.assertIn(["git", "push", "origin", "receiver-ui-v0.1.1"], calls)

    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_streamer_runs_release_build_without_ui_checks(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["streamer"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertNotIn(["npm", "ci"], calls)
        self.assertIn(
            [
                "cargo",
                "build",
                "--release",
                "--package",
                "streamer",
                "--bin",
                "streamer",
            ],
            calls,
        )

    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_server_runs_ui_checks_and_optional_local_docker_build(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["server"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=True,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertIn(["npm", "ci"], calls)
        self.assertIn(
            ["npm", "run", "lint", "--workspace", "apps/server-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "run", "check", "--workspace", "apps/server-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "test", "--workspace", "apps/server-ui"],
            calls,
        )
        self.assertIn(
            [
                "docker",
                "build",
                "-t",
                "iwismer/rt-server:v0.1.1",
                "-t",
                "iwismer/rt-server:latest",
                "-f",
                "services/server/Dockerfile",
                ".",
            ],
            calls,
        )
        self.assertIn(["git", "push", "origin", "master"], calls)
        self.assertIn(["git", "push", "origin", "server-v0.1.1"], calls)
        self.assertNotIn(["docker", "push", "iwismer/rt-server:v0.1.1"], calls)
        self.assertNotIn(["docker", "push", "iwismer/rt-server:latest"], calls)

    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_server_without_local_docker_build_runs_release_binary_build(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        _write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["server"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        self.assertIn(
            [
                "cargo",
                "build",
                "--release",
                "--package",
                "server",
                "--bin",
                "server",
            ],
            calls,
        )
        self.assertNotIn(
            [
                "docker",
                "build",
                "-t",
                "iwismer/rt-server:v0.1.1",
                "-t",
                "iwismer/rt-server:latest",
                "-f",
                "services/server/Dockerfile",
                ".",
            ],
            calls,
        )


class DryRunBehaviorTests(unittest.TestCase):
    @patch("scripts.release.write_version")
    @patch("scripts.release.compute_new_version")
    @patch("scripts.release.read_version")
    @patch("scripts.release.git_current_branch", return_value="master")
    @patch("scripts.release.git_is_dirty", return_value=False)
    def test_dry_run_executes_checks_but_skips_mutating_release_commands(
        self,
        _dirty_mock,
        _branch_mock,
        read_version_mock,
        compute_new_version_mock,
        write_version_mock,
    ) -> None:
        args = argparse.Namespace(
            services=["forwarder"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=True,
            yes=True,
            server_docker_image="iwismer/rt-server",
            server_local_docker_build=False,
        )

        read_version_mock.return_value = "0.1.0"
        compute_new_version_mock.return_value = "0.1.1"

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("scripts.release.parse_args", return_value=args), patch(
            "scripts.release.run", side_effect=fake_run
        ):
            release.main()

        write_version_mock.assert_not_called()

        self.assertIn(["npm", "ci"], calls)
        self.assertIn(
            ["npm", "run", "lint", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "run", "check", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            ["npm", "test", "--workspace", "apps/forwarder-ui"],
            calls,
        )
        self.assertIn(
            [
                "cargo",
                "build",
                "--release",
                "--package",
                "forwarder",
                "--bin",
                "forwarder",
                "--features",
                "embed-ui",
            ],
            calls,
        )

        self.assertNotIn(
            ["git", "add", "services/forwarder/Cargo.toml", "Cargo.lock"],
            calls,
        )
        self.assertNotIn(
            ["git", "commit", "-m", "chore(forwarder): bump version to 0.1.1"],
            calls,
        )
        self.assertNotIn(["git", "tag", "forwarder-v0.1.1"], calls)
        self.assertNotIn(["git", "push", "origin", "master"], calls)

if __name__ == "__main__":
    unittest.main()
