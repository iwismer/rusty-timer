import argparse
import sys
import unittest
from unittest.mock import patch

import scripts.dev as dev


class EmulatorSpecToCmdTests(unittest.TestCase):
    def test_to_cmd_quotes_file_path_with_spaces(self) -> None:
        spec = dev.EmulatorSpec(port=10001, delay=500, file="data/my reads.txt")
        cmd = spec.to_cmd()
        self.assertEqual(
            cmd,
            "cargo run -p emulator -- --port 10001 --delay 500 --type raw"
            " --file 'data/my reads.txt'",
        )

    def test_to_cmd_quotes_file_path_with_special_chars(self) -> None:
        spec = dev.EmulatorSpec(port=10001, delay=500, file="data/reads;echo pwned.txt")
        cmd = spec.to_cmd()
        self.assertIn("'data/reads;echo pwned.txt'", cmd)

    def test_to_cmd_no_file(self) -> None:
        spec = dev.EmulatorSpec(port=10001, delay=2000)
        cmd = spec.to_cmd()
        self.assertEqual(
            cmd,
            "cargo run -p emulator -- --port 10001 --delay 2000 --type raw",
        )


class ParseArgsEmulatorFlagTests(unittest.TestCase):
    def test_parse_args_reads_emulator_spec(self) -> None:
        with patch.object(
            sys,
            "argv",
            ["dev.py", "--emulator", "port=10001,delay=500,file=data/reads.txt"],
        ):
            args = dev.parse_args()

        self.assertEqual(len(args.emulator), 1)
        spec = args.emulator[0]
        self.assertIsInstance(spec, dev.EmulatorSpec)
        self.assertEqual(spec.port, 10001)
        self.assertEqual(spec.delay, 500)
        self.assertEqual(spec.file, "data/reads.txt")
        self.assertEqual(spec.read_type, "raw")


class ParseEmulatorSpecErrorTests(unittest.TestCase):
    def test_missing_port_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("delay=500")
        self.assertIn("port", str(ctx.exception))

    def test_unknown_key_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=10001,bogus=xyz")
        self.assertIn("Unknown emulator key", str(ctx.exception))

    def test_invalid_port_value_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=abc")
        self.assertIn("Invalid port", str(ctx.exception))


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
