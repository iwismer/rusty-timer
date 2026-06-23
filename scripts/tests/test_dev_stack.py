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
                    device_token_file=temp_dir / "forwarder-device-token",
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

    def test_forwarder_config_persists_minted_server_device_token_in_work_dir(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir_name:
            temp_dir = Path(temp_dir_name)
            config_path = temp_dir / "forwarder.toml"

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
                ],
                p2p_port=3333,
                server_url="http://127.0.0.1:8675",
                server_token_file=temp_dir / "server-token",
                device_token_file=temp_dir / "forwarder-device-token",
            )

            config = config_path.read_text()
            self.assertIn(
                f'device_token_file = "{temp_dir / "forwarder-device-token"}"',
                config,
            )

    def test_reader_count_cli_flag_is_documented(self) -> None:
        parser = dev_stack.build_parser()

        help_text = parser.format_help()

        self.assertIn("--readers", help_text)


class DevStackAuthTests(unittest.TestCase):
    def test_dev_stack_uses_distinct_enrollment_vouchers(self) -> None:
        self.assertNotEqual(dev_stack.FORWARDER_VOUCHER, dev_stack.RECEIVER_VOUCHER)
        self.assertTrue(dev_stack.FORWARDER_VOUCHER)
        self.assertTrue(dev_stack.RECEIVER_VOUCHER)

    def test_create_enrollment_token_posts_fixed_secret_to_admin_api(self) -> None:
        calls = []

        def fake_post_json(url: str, body: dict, *, headers: dict | None = None) -> dict:
            calls.append((url, body, headers))
            return {"token": body["token"]}

        original = dev_stack.post_json
        dev_stack.post_json = fake_post_json
        try:
            result = dev_stack.server_create_enrollment_token(
                "http://127.0.0.1:8675",
                "receiver",
                "receiver-voucher",
            )
        finally:
            dev_stack.post_json = original

        self.assertEqual(result, {"token": "receiver-voucher"})
        self.assertEqual(
            calls,
            [
                (
                    "http://127.0.0.1:8675/admin/enrollment-tokens",
                    {
                        "device_kind": "receiver",
                        "token": "receiver-voucher",
                        "display_name": "dev receiver",
                    },
                    {"Remote-User": "rt-dev-admin"},
                )
            ],
        )


if __name__ == "__main__":
    unittest.main()
