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


class TransactionTests(unittest.TestCase):
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
    ) -> None:
        args = argparse.Namespace(
            services=["forwarder", "receiver"],
            major=False,
            minor=False,
            patch=True,
            version=None,
            dry_run=False,
            yes=True,
        )

        read_version_mock.side_effect = ["0.1.0", "0.1.0"]
        compute_new_version_mock.side_effect = ["0.1.1", "0.1.1"]

        calls: list[list[str]] = []

        def fake_run(cmd: list[str], **kwargs):  # noqa: ANN003
            calls.append(cmd)
            if cmd == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(cmd, 0, stdout="abc123\n", stderr="")
            if cmd == ["cargo", "check", "-p", "receiver"]:
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
    def test_pushes_branch_and_tags_atomically(
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
            ["git", "push", "--atomic", "origin", "master", "forwarder-v0.1.1"],
            calls,
        )


if __name__ == "__main__":
    unittest.main()
