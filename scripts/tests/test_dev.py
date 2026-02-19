import argparse
import sys
import unittest
import urllib.error
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

    def test_port_below_range_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=0")
        self.assertIn("out of range", str(ctx.exception))

    def test_port_above_range_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=65536")
        self.assertIn("out of range", str(ctx.exception))

    def test_negative_delay_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=10001,delay=-1")
        self.assertIn("non-negative", str(ctx.exception))

    def test_invalid_type_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=10001,type=bogus")
        self.assertIn("Invalid type", str(ctx.exception))

    def test_fallback_port_above_range_raises(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError) as ctx:
            dev.parse_emulator_spec("port=65000")
        self.assertIn("fallback", str(ctx.exception))


class BuildForwarderTomlTests(unittest.TestCase):
    def test_build_forwarder_toml_contains_multiple_readers(self) -> None:
        text = dev.build_forwarder_toml(
            [
                dev.EmulatorSpec(port=10001, read_type="raw"),
                dev.EmulatorSpec(port=10002, read_type="fsls"),
            ]
        )
        self.assertIn('target              = "127.0.0.1:10001"', text)
        self.assertIn('target              = "127.0.0.1:10002"', text)
        self.assertNotIn("read_type", text)


class MainValidationTests(unittest.TestCase):
    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    def test_main_exits_on_emulator_port_collision(
        self, parse_args_mock, setup_mock, detect_mock
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001), dev.EmulatorSpec(port=10001)],
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()

    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    def test_main_exits_on_port_fallback_collision(
        self, parse_args_mock, setup_mock, detect_mock
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001), dev.EmulatorSpec(port=11001)],
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()

    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    @patch("scripts.dev.receiver_default_local_port", return_value=10001)
    def test_main_exits_on_receiver_default_port_collision(
        self, _receiver_port_mock, parse_args_mock, setup_mock, detect_mock
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001)],
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()


class ReceiverDefaultPortTests(unittest.TestCase):
    def test_reader_port_10000_uses_legacy_mapping(self) -> None:
        self.assertEqual(dev.receiver_default_local_port("10.0.0.1:10000"), 10001)

    def test_same_ip_different_reader_ports_map_to_different_defaults(self) -> None:
        p1 = dev.receiver_default_local_port("10.0.0.1:10001")
        p2 = dev.receiver_default_local_port("10.0.0.1:10002")
        self.assertIsNotNone(p1)
        self.assertIsNotNone(p2)
        self.assertNotEqual(p1, p2)


class ClearTests(unittest.TestCase):
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev.shutil.which")
    def test_clear_skips_docker_when_missing(self, which_mock, run_mock, _print_mock) -> None:
        which_mock.side_effect = lambda tool: None if tool in {"tmux", "docker"} else "/usr/bin/" + tool

        dev.clear()

        run_mock.assert_not_called()


class CheckPrereqsTests(unittest.TestCase):
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.sys.exit", side_effect=SystemExit)
    @patch("scripts.dev.shutil.which")
    def test_check_prereqs_exits_when_curl_missing(
        self, which_mock, _exit_mock, _print_mock
    ) -> None:
        which_mock.side_effect = lambda tool: None if tool == "curl" else f"/usr/bin/{tool}"

        with self.assertRaises(SystemExit):
            dev.check_prereqs()


class ConfigureReceiverDevTests(unittest.TestCase):
    @patch("scripts.dev.urllib.request.urlopen")
    def test_configure_receiver_dev_success_calls_expected_endpoints(self, urlopen_mock) -> None:
        urlopen_mock.side_effect = [object(), object(), object()]

        dev.configure_receiver_dev()

        self.assertEqual(urlopen_mock.call_count, 3)

        health_args, health_kwargs = urlopen_mock.call_args_list[0]
        self.assertEqual(health_args[0], "http://127.0.0.1:9090/healthz")
        self.assertEqual(health_kwargs["timeout"], 2)

        profile_req = urlopen_mock.call_args_list[1].args[0]
        self.assertEqual(profile_req.get_method(), "PUT")
        self.assertEqual(profile_req.full_url, "http://127.0.0.1:9090/api/v1/profile")
        self.assertEqual(profile_req.get_header("Content-type"), "application/json")
        self.assertEqual(profile_req.data, b'{"server_url": "ws://127.0.0.1:8080", "token": "rusty-dev-receiver", "log_level": "info"}')
        self.assertEqual(urlopen_mock.call_args_list[1].kwargs["timeout"], 5)

        connect_req = urlopen_mock.call_args_list[2].args[0]
        self.assertEqual(connect_req.get_method(), "POST")
        self.assertEqual(connect_req.full_url, "http://127.0.0.1:9090/api/v1/connect")
        self.assertEqual(connect_req.data, b"")
        self.assertEqual(urlopen_mock.call_args_list[2].kwargs["timeout"], 5)

    @patch("scripts.dev.time.sleep")
    @patch("scripts.dev.urllib.request.urlopen")
    def test_configure_receiver_dev_returns_after_health_timeout(
        self, urlopen_mock, sleep_mock
    ) -> None:
        urlopen_mock.side_effect = urllib.error.URLError("down")

        dev.configure_receiver_dev()

        self.assertEqual(urlopen_mock.call_count, 60)
        self.assertEqual(sleep_mock.call_count, 60)

    @patch("scripts.dev.urllib.request.urlopen")
    def test_configure_receiver_dev_stops_when_profile_fails(self, urlopen_mock) -> None:
        urlopen_mock.side_effect = [object(), urllib.error.URLError("bad profile")]

        dev.configure_receiver_dev()

        self.assertEqual(urlopen_mock.call_count, 2)


class DetectAndLaunchTests(unittest.TestCase):
    @patch("scripts.dev.launch_tmux")
    @patch("scripts.dev.start_receiver_auto_config")
    @patch("scripts.dev.shutil.which", return_value="/usr/bin/tmux")
    def test_detect_and_launch_tmux_starts_auto_config(
        self, _which_mock, auto_config_mock, launch_tmux_mock
    ) -> None:
        dev.detect_and_launch([dev.EmulatorSpec(port=10001)])

        auto_config_mock.assert_called_once_with()
        launch_tmux_mock.assert_called_once()

    @patch("scripts.dev.launch_iterm2")
    @patch("scripts.dev.Path.exists", return_value=True)
    @patch("scripts.dev.start_receiver_auto_config")
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_detect_and_launch_iterm_starts_auto_config(
        self, _which_mock, auto_config_mock, _exists_mock, launch_iterm2_mock
    ) -> None:
        dev.detect_and_launch([dev.EmulatorSpec(port=10001)])

        auto_config_mock.assert_called_once_with()
        launch_iterm2_mock.assert_called_once()


class SetupOrderingTests(unittest.TestCase):
    @patch("scripts.dev.seed_tokens")
    @patch("scripts.dev.write_config_files")
    @patch("scripts.dev.apply_migrations")
    @patch("scripts.dev.wait_for_postgres")
    @patch("scripts.dev.start_postgres")
    @patch("scripts.dev.check_prereqs")
    @patch("scripts.dev.npm_install")
    @patch("scripts.dev.build_rust")
    def test_setup_installs_npm_before_rust_build(
        self,
        build_rust_mock,
        npm_install_mock,
        _check_prereqs_mock,
        _start_postgres_mock,
        _wait_for_postgres_mock,
        _apply_migrations_mock,
        _write_config_files_mock,
        _seed_tokens_mock,
    ) -> None:
        call_order: list[str] = []
        npm_install_mock.side_effect = lambda: call_order.append("npm_install")
        build_rust_mock.side_effect = lambda skip_build: call_order.append(
            f"build_rust({skip_build})"
        )

        dev.setup(skip_build=False, emulators=[dev.EmulatorSpec(port=10001)])

        self.assertEqual(call_order[0], "npm_install")
        self.assertEqual(call_order[1], "build_rust(False)")


class StartReceiverAutoConfigTests(unittest.TestCase):
    @patch("threading.Thread")
    def test_start_receiver_auto_config_spawns_daemon_thread(self, thread_cls) -> None:
        thread_mock = thread_cls.return_value

        dev.start_receiver_auto_config()

        thread_cls.assert_called_once_with(
            target=dev.configure_receiver_dev,
            name="receiver-auto-config",
            daemon=True,
        )
        thread_mock.start.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
