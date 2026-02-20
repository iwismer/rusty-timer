import argparse
import shlex
import subprocess
import tempfile
import sys
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch

import scripts.dev as dev


class EmulatorSpecToCmdTests(unittest.TestCase):
    def test_to_cmd_quotes_file_path_with_spaces(self) -> None:
        spec = dev.EmulatorSpec(port=10001, delay=500, file="data/my reads.txt")
        cmd = spec.to_cmd()
        self.assertEqual(
            cmd,
            "./target/debug/emulator --port 10001 --delay 500 --type raw"
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
            "./target/debug/emulator --port 10001 --delay 2000 --type raw",
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
    @patch("scripts.dev.check_existing_instance")
    @patch("scripts.dev.parse_args")
    @patch("scripts.dev.receiver_default_local_port", return_value=12001)
    def test_main_checks_existing_instance_after_validation_for_valid_args(
        self,
        _receiver_port_mock,
        parse_args_mock,
        check_existing_instance_mock,
        setup_mock,
        detect_mock,
    ) -> None:
        events: list[str] = []
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001)],
        )
        check_existing_instance_mock.side_effect = lambda: events.append("check")
        setup_mock.side_effect = lambda **_kwargs: events.append("setup")
        detect_mock.side_effect = lambda _emulators: events.append("launch")

        dev.main()

        self.assertEqual(events, ["check", "setup", "launch"])
        check_existing_instance_mock.assert_called_once_with()

    @patch("scripts.dev.console.input")
    @patch("scripts.dev.check_existing_instance")
    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    @patch("scripts.dev.receiver_default_local_port", return_value=12001)
    def test_main_exits_on_emulator_port_collision(
        self,
        _receiver_port_mock,
        parse_args_mock,
        setup_mock,
        detect_mock,
        check_existing_instance_mock,
        input_mock,
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001), dev.EmulatorSpec(port=10001)],
            bibchip=None,
            ppl=None,
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()
        check_existing_instance_mock.assert_not_called()
        input_mock.assert_not_called()

    @patch("scripts.dev.check_existing_instance")
    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    def test_main_exits_on_port_fallback_collision(
        self, parse_args_mock, setup_mock, detect_mock, check_existing_instance_mock
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001), dev.EmulatorSpec(port=11001)],
            bibchip=None,
            ppl=None,
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()
        check_existing_instance_mock.assert_not_called()

    @patch("scripts.dev.check_existing_instance")
    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    @patch("scripts.dev.receiver_default_local_port", return_value=10001)
    def test_main_exits_on_receiver_default_port_collision(
        self,
        _receiver_port_mock,
        parse_args_mock,
        setup_mock,
        detect_mock,
        check_existing_instance_mock,
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001)],
            bibchip=None,
            ppl=None,
        )
        with self.assertRaises(SystemExit):
            dev.main()
        setup_mock.assert_not_called()
        detect_mock.assert_not_called()
        check_existing_instance_mock.assert_not_called()


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


class CheckExistingInstanceTests(unittest.TestCase):
    @patch("scripts.dev.ITERM_WINDOW_ID_PATH")
    @patch("scripts.dev.console.input")
    @patch("scripts.dev._listener_pids", return_value=[])
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_no_tmux_no_server_returns_silently(
        self, _which_mock, _listener_pids_mock, input_mock, iterm_path_mock
    ) -> None:
        dev.check_existing_instance()
        input_mock.assert_not_called()

    @patch("scripts.dev.ITERM_WINDOW_ID_PATH")
    @patch("scripts.dev._listener_pids", return_value=[])
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_stale_iterm_file_cleaned_when_nothing_running(
        self, _which_mock, _listener_pids_mock, iterm_path_mock
    ) -> None:
        dev.check_existing_instance()
        iterm_path_mock.unlink.assert_called_once_with(missing_ok=True)

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="y")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._listener_pids", return_value=[])
    @patch("scripts.dev.shutil.which", return_value="/usr/bin/tmux")
    def test_tmux_session_detected_and_killed_on_yes(
        self,
        _which_mock,
        _listener_pids_mock,
        run_mock,
        kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        def run_side_effect(cmd, **kwargs):
            if cmd == ["tmux", "has-session", "-t", "rusty-dev"]:
                return subprocess.CompletedProcess(cmd, returncode=0)
            return subprocess.CompletedProcess(cmd, returncode=0)

        run_mock.side_effect = run_side_effect
        dev.check_existing_instance()

        input_mock.assert_called_once()
        kill_calls = [c for c in run_mock.call_args_list if c.args[0] == ["tmux", "kill-session", "-t", "rusty-dev"]]
        self.assertEqual(len(kill_calls), 1)
        kill_pids_mock.assert_not_called()

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="n")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._listener_pids", return_value=[])
    @patch("scripts.dev.shutil.which", return_value="/usr/bin/tmux")
    def test_tmux_session_detected_but_skipped_on_no(
        self, _which_mock, _listener_pids_mock, run_mock, input_mock, _print_mock, close_mock
    ) -> None:
        def run_side_effect(cmd, **kwargs):
            if cmd == ["tmux", "has-session", "-t", "rusty-dev"]:
                return subprocess.CompletedProcess(cmd, returncode=0)
            return subprocess.CompletedProcess(cmd, returncode=0)

        run_mock.side_effect = run_side_effect
        dev.check_existing_instance()

        input_mock.assert_called_once()
        kill_calls = [c for c in run_mock.call_args_list if c.args[0] == ["tmux", "kill-session", "-t", "rusty-dev"]]
        self.assertEqual(len(kill_calls), 0)
        close_mock.assert_not_called()

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="y")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._pid_command", return_value="BIND_ADDR=0.0.0.0:8080 ./target/debug/server")
    @patch("scripts.dev._listener_pids", return_value=[4242])
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_server_port_detected_and_processes_killed(
        self,
        _which_mock,
        _listener_pids_mock,
        _pid_command_mock,
        run_mock,
        kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        run_mock.return_value = subprocess.CompletedProcess([], returncode=0)
        dev.check_existing_instance()

        input_mock.assert_called_once()
        kill_pids_mock.assert_called_once_with([4242])
        pkill_calls = [c for c in run_mock.call_args_list if c.args[0][0] == "pkill"]
        self.assertEqual(len(pkill_calls), len(dev.DEV_BINARIES))
        close_mock.assert_called_once()

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="y")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._has_saved_iterm_window_id", return_value=True)
    @patch("scripts.dev._pid_command", return_value="server")
    @patch("scripts.dev._listener_pids", return_value=[5555])
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_server_process_name_without_path_is_detected_as_dev(
        self,
        _which_mock,
        _listener_pids_mock,
        _pid_command_mock,
        _iterm_id_mock,
        run_mock,
        kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        run_mock.return_value = subprocess.CompletedProcess([], returncode=0)
        dev.check_existing_instance()

        input_mock.assert_called_once()
        kill_pids_mock.assert_called_once_with([5555])
        close_mock.assert_called_once()

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="y")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._pid_command", return_value="BIND_ADDR=0.0.0.0:8080 ./target/debug/server")
    @patch("scripts.dev._listener_pids", return_value=[4242])
    @patch("scripts.dev.shutil.which")
    def test_kill_stops_docker_container(
        self,
        which_mock,
        _listener_pids_mock,
        _pid_command_mock,
        run_mock,
        _kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        which_mock.side_effect = lambda tool: "/usr/bin/docker" if tool == "docker" else None
        run_mock.return_value = subprocess.CompletedProcess([], returncode=0)
        dev.check_existing_instance()

        docker_calls = [
            c for c in run_mock.call_args_list
            if c.args[0][:2] == ["docker", "rm"]
        ]
        self.assertEqual(len(docker_calls), 1)
        self.assertIn(dev.PG_CONTAINER, docker_calls[0].args[0])

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="n")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._pid_command", return_value="python -m http.server 8080")
    @patch("scripts.dev._listener_pids", return_value=[7777])
    @patch("scripts.dev.shutil.which", return_value=None)
    def test_foreign_server_listener_is_not_killed(
        self,
        _which_mock,
        _listener_pids_mock,
        _pid_command_mock,
        run_mock,
        kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        run_mock.return_value = subprocess.CompletedProcess([], returncode=0)
        dev.check_existing_instance()

        input_mock.assert_called_once()
        kill_pids_mock.assert_not_called()
        pkill_calls = [c for c in run_mock.call_args_list if c.args[0][0] == "pkill"]
        self.assertEqual(len(pkill_calls), 0)
        close_mock.assert_not_called()

    @patch("scripts.dev.close_iterm2_window")
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.console.input", return_value="y")
    @patch("scripts.dev._kill_pids")
    @patch("scripts.dev.subprocess.run")
    @patch("scripts.dev._pid_command", return_value="python -m http.server 8080")
    @patch("scripts.dev._listener_pids", return_value=[7777])
    @patch("scripts.dev.shutil.which", return_value="/usr/bin/tmux")
    def test_tmux_plus_foreign_listener_is_not_killed(
        self,
        _which_mock,
        _listener_pids_mock,
        _pid_command_mock,
        run_mock,
        kill_pids_mock,
        input_mock,
        _print_mock,
        close_mock,
    ) -> None:
        def run_side_effect(cmd, **kwargs):
            if cmd == ["tmux", "has-session", "-t", "rusty-dev"]:
                return subprocess.CompletedProcess(cmd, returncode=0)
            return subprocess.CompletedProcess(cmd, returncode=0)

        run_mock.side_effect = run_side_effect
        dev.check_existing_instance()

        input_mock.assert_called_once()
        tmux_kill_calls = [
            c for c in run_mock.call_args_list
            if c.args[0] == ["tmux", "kill-session", "-t", "rusty-dev"]
        ]
        self.assertEqual(len(tmux_kill_calls), 0)
        kill_pids_mock.assert_not_called()
        pkill_calls = [c for c in run_mock.call_args_list if c.args[0][0] == "pkill"]
        self.assertEqual(len(pkill_calls), 0)
        close_mock.assert_not_called()


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

    @patch("scripts.dev.start_race_data_setup")
    @patch("scripts.dev.launch_tmux")
    @patch("scripts.dev.start_receiver_auto_config")
    @patch("scripts.dev.shutil.which", return_value="/usr/bin/tmux")
    def test_detect_and_launch_starts_race_data_setup_when_paths_provided(
        self,
        _which_mock,
        auto_config_mock,
        launch_tmux_mock,
        race_setup_mock,
    ) -> None:
        bibchip = Path("/tmp/test.bibchip")
        ppl = Path("/tmp/test.ppl")
        dev.detect_and_launch(
            [dev.EmulatorSpec(port=10001)],
            bibchip_path=bibchip,
            ppl_path=ppl,
        )

        auto_config_mock.assert_called_once_with()
        launch_tmux_mock.assert_called_once()
        race_setup_mock.assert_called_once_with(bibchip, ppl)


class SetupOrderingTests(unittest.TestCase):
    @patch("scripts.dev.seed_tokens")
    @patch("scripts.dev.write_config_files")
    @patch("scripts.dev.apply_migrations")
    @patch("scripts.dev.wait_for_postgres")
    @patch("scripts.dev.start_postgres")
    @patch("scripts.dev.check_prereqs")
    @patch("scripts.dev.npm_install")
    @patch("scripts.dev.build_dashboard")
    @patch("scripts.dev.build_rust")
    def test_setup_installs_npm_before_rust_build(
        self,
        build_rust_mock,
        build_dashboard_mock,
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
        build_dashboard_mock.side_effect = lambda skip_build: call_order.append(
            f"build_dashboard({skip_build})"
        )
        build_rust_mock.side_effect = lambda skip_build: call_order.append(
            f"build_rust({skip_build})"
        )

        dev.setup(skip_build=False, emulators=[dev.EmulatorSpec(port=10001)])

        self.assertEqual(call_order[0], "npm_install")
        self.assertEqual(call_order[1], "build_dashboard(False)")
        self.assertEqual(call_order[2], "build_rust(False)")


class NpmInstallTests(unittest.TestCase):
    @patch("scripts.dev.console.print")
    @patch("scripts.dev.subprocess.run")
    def test_npm_install_runs_once_in_workspace_root_when_missing(
        self, run_mock, _print_mock
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)
            with patch.object(dev, "REPO_ROOT", repo_root):
                dev.npm_install()

        run_mock.assert_called_once_with(["npm", "install"], check=True, cwd=repo_root)

    @patch("scripts.dev.console.print")
    @patch("scripts.dev.subprocess.run")
    def test_npm_install_runs_when_workspace_node_modules_exists(
        self, run_mock, _print_mock
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo_root = Path(tmp)
            (repo_root / "node_modules").mkdir()
            with patch.object(dev, "REPO_ROOT", repo_root):
                dev.npm_install()

        run_mock.assert_called_once_with(["npm", "install"], check=True, cwd=repo_root)


class BuildDashboardTests(unittest.TestCase):
    @patch("scripts.dev.subprocess.run")
    def test_build_dashboard_installs_workspace_before_build(self, run_mock) -> None:
        dev.build_dashboard(skip_build=False)

        self.assertEqual(run_mock.call_count, 2)
        self.assertEqual(
            run_mock.call_args_list[0].args[0],
            ["npm", "install", "--workspace=apps/server-ui"],
        )
        self.assertEqual(
            run_mock.call_args_list[1].args[0],
            ["npm", "run", "build", "--workspace=apps/server-ui"],
        )
        for call in run_mock.call_args_list:
            self.assertTrue(call.kwargs["check"])
            self.assertEqual(call.kwargs["cwd"], dev.REPO_ROOT)

    @patch("scripts.dev.subprocess.run")
    def test_build_dashboard_skips_subprocess_when_no_build(self, run_mock) -> None:
        dev.build_dashboard(skip_build=True)
        run_mock.assert_not_called()


class BuildPanesTests(unittest.TestCase):
    def test_server_pane_uses_current_repo_root_with_shell_safe_dashboard_dir(self) -> None:
        with tempfile.TemporaryDirectory(prefix="repo with spaces ") as tmp:
            repo_root = Path(tmp)
            (repo_root / "apps" / "server-ui" / "build").mkdir(parents=True)
            with patch.object(dev, "REPO_ROOT", repo_root):
                panes = dev.build_panes([dev.EmulatorSpec(port=10001)])

        server_cmd = next(cmd for title, cmd in panes if title == "Server")
        expected_dashboard_dir = shlex.quote(str(repo_root / "apps" / "server-ui" / "build"))
        self.assertIn(f"DASHBOARD_DIR={expected_dashboard_dir}", server_cmd)

    def test_server_pane_omits_dashboard_dir_when_static_build_missing(self) -> None:
        with tempfile.TemporaryDirectory(prefix="repo without dashboard build ") as tmp:
            repo_root = Path(tmp)
            with patch.object(dev, "REPO_ROOT", repo_root):
                panes = dev.build_panes([dev.EmulatorSpec(port=10001)])

        server_cmd = next(cmd for title, cmd in panes if title == "Server")
        self.assertNotIn("DASHBOARD_DIR=", server_cmd)


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


class ParseArgsRaceDataFlagTests(unittest.TestCase):
    def test_parse_args_reads_bibchip_and_ppl_paths(self) -> None:
        with patch.object(
            sys,
            "argv",
            ["dev.py", "--bibchip", "test_assets/bibchip/large.txt", "--ppl", "test_assets/ppl/large.ppl"],
        ):
            args = dev.parse_args()

        self.assertEqual(args.bibchip, Path("test_assets/bibchip/large.txt"))
        self.assertEqual(args.ppl, Path("test_assets/ppl/large.ppl"))


class GenerateReadsFromBibchipTests(unittest.TestCase):
    def test_generate_reads_from_bibchip_writes_ipico_reads(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            bibchip = tmp_path / "sample.txt"
            bibchip.write_text("BIB,CHIP\n1,058003700001\n2,058003700002\n", encoding="utf-8")
            with patch.object(dev, "TMP_DIR", tmp_path / "out"):
                reads_path = dev.generate_reads_from_bibchip(bibchip)

            lines = reads_path.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 2)
            self.assertTrue(lines[0].startswith("aa00"))
            self.assertEqual(len(lines[0]), 36)


class StartRaceDataSetupTests(unittest.TestCase):
    @patch("threading.Thread")
    def test_start_race_data_setup_spawns_daemon_thread(self, thread_cls) -> None:
        thread_mock = thread_cls.return_value
        bibchip = Path("/tmp/sample.bibchip")
        ppl = Path("/tmp/sample.ppl")

        dev.start_race_data_setup(bibchip, ppl)

        thread_cls.assert_called_once_with(
            target=dev.setup_race_data,
            args=(bibchip, ppl),
            name="race-data-setup",
            daemon=True,
        )
        thread_mock.start.assert_called_once_with()


class MainRacePathValidationTests(unittest.TestCase):
    @patch("scripts.dev.detect_and_launch")
    @patch("scripts.dev.setup")
    @patch("scripts.dev.parse_args")
    def test_main_exits_when_bibchip_path_missing(
        self, parse_args_mock, setup_mock, detect_mock
    ) -> None:
        parse_args_mock.return_value = argparse.Namespace(
            no_build=False,
            clear=False,
            emulator=[dev.EmulatorSpec(port=10001)],
            bibchip=Path("/tmp/does-not-exist.bibchip"),
            ppl=None,
        )

        with self.assertRaises(SystemExit):
            dev.main()

        setup_mock.assert_not_called()
        detect_mock.assert_not_called()


if __name__ == "__main__":
    unittest.main()
