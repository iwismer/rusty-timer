import tempfile
import unittest
from pathlib import Path

from scripts import dev_stack


class DevStackChipTests(unittest.TestCase):
    def test_generated_reads_match_large_bibchip_chip_series(self) -> None:
        bibchip_path = (
            Path(__file__).resolve().parents[2]
            / "test_assets"
            / "bibchip"
            / "large.txt"
        )
        expected_chips = [
            line.split(",", 1)[1].strip()
            for line in bibchip_path.read_text().splitlines()
            if line and line[0].isdigit()
        ][: dev_stack.NUM_FRAMES]

        generated_chips = [
            dev_stack.build_frame(chip_index)[4:16]
            for chip_index in range(1, dev_stack.NUM_FRAMES + 1)
        ]

        self.assertEqual(expected_chips, generated_chips)


class DevStackForwarderConfigTests(unittest.TestCase):
    def test_forwarder_config_accepts_multiple_reader_targets(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir_name:
            temp_dir = Path(temp_dir_name)
            config_path = temp_dir / "forwarder.toml"
            try:
                dev_stack.write_forwarder_config(
                    config_path,
                    auth_token_file=temp_dir / "auth-token",
                    journal_path=temp_dir / "journal.sqlite3",
                    status_port=8787,
                    readers=[
                        dev_stack.ForwarderReaderPorts(
                            emulator_port=1111,
                            fanout_port=2111,
                        ),
                        dev_stack.ForwarderReaderPorts(
                            emulator_port=1112,
                            fanout_port=2112,
                        ),
                    ],
                    p2p_port=3333,
                    server_url="http://127.0.0.1:8675",
                    server_token_file=temp_dir / "server-token",
                )
            except TypeError as exc:
                self.fail(
                    f"write_forwarder_config should accept multiple readers: {exc}"
                )
            except AttributeError as exc:
                self.fail(f"dev_stack should expose ForwarderReaderPorts: {exc}")

            config = config_path.read_text()
            self.assertEqual(config.count("[[readers]]"), 2)
            self.assertIn('target = "127.0.0.1:1111"', config)
            self.assertIn("local_fallback_port = 2111", config)
            self.assertIn('target = "127.0.0.1:1112"', config)
            self.assertIn("local_fallback_port = 2112", config)

    def test_reader_count_cli_flag_is_documented(self) -> None:
        parser = dev_stack.build_parser()

        help_text = parser.format_help()

        self.assertIn("--readers", help_text)


if __name__ == "__main__":
    unittest.main()
