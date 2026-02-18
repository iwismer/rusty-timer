import sys
import unittest
from unittest.mock import patch

import scripts.dev as dev


class ParseArgsTests(unittest.TestCase):
    def test_parse_args_reads_emulator_flags(self) -> None:
        with patch.object(
            sys,
            "argv",
            ["dev.py", "--emulator-file", "data/reads.txt", "--emulator-delay", "500"],
        ):
            args = dev.parse_args()

        self.assertEqual(args.emulator_file, "data/reads.txt")
        self.assertEqual(args.emulator_delay, 500)
        self.assertFalse(args.no_build)
        self.assertFalse(args.clear)


class EmulatorCommandTests(unittest.TestCase):
    def test_build_emulator_cmd_quotes_file_path(self) -> None:
        cmd = dev.build_emulator_cmd(500, "data/my reads;echo pwned.txt")
        self.assertEqual(
            cmd,
            "cargo run -p emulator -- --port 10001 --delay 500 --type raw --file 'data/my reads;echo pwned.txt'",
        )


class ClearTests(unittest.TestCase):
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev.shutil.which")
    def test_clear_skips_docker_when_missing(self, which_mock, run_mock, _print_mock) -> None:
        which_mock.side_effect = lambda tool: None if tool in {"tmux", "docker"} else "/usr/bin/" + tool

        dev.clear()

        run_mock.assert_not_called()


if __name__ == "__main__":
    unittest.main()
