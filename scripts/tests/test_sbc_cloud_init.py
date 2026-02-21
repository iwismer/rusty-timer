import tempfile
import unittest
from pathlib import Path

import scripts.sbc_cloud_init as sbc_cloud_init


class ValidationTests(unittest.TestCase):
    def test_validate_hostname_accepts_expected_format(self) -> None:
        self.assertEqual("rt-fwd-01", sbc_cloud_init.validate_hostname("rt-fwd-01"))

    def test_validate_hostname_rejects_uppercase(self) -> None:
        with self.assertRaises(ValueError):
            sbc_cloud_init.validate_hostname("RT-FWD-01")

    def test_parse_dns_servers_returns_trimmed_ipv4_values(self) -> None:
        dns = sbc_cloud_init.parse_dns_servers("1.1.1.1, 8.8.8.8")
        self.assertEqual(("1.1.1.1", "8.8.8.8"), dns)

    def test_parse_dns_servers_rejects_invalid_entry(self) -> None:
        with self.assertRaises(ValueError):
            sbc_cloud_init.parse_dns_servers("8.8.8.8,not-an-ip")


class RenderTests(unittest.TestCase):
    def test_render_user_data_contains_hostname_and_ssh_key(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-77",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.77/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("8.8.8.8",),
        )

        text = sbc_cloud_init.render_user_data(config)

        self.assertIn("hostname: rt-fwd-77", text)
        self.assertIn("ssh_authorized_keys:", text)
        self.assertIn("ssh-ed25519 AAAATEST user@test", text)

    def test_render_network_config_contains_all_network_fields(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-77",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.77/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("1.1.1.1", "8.8.8.8"),
        )

        text = sbc_cloud_init.render_network_config(config)

        self.assertIn("network:", text)
        self.assertIn("  version: 2", text)
        self.assertIn("  ethernets:", text)
        self.assertIn("      dhcp6: false", text)
        self.assertIn("      optional: true", text)
        self.assertIn("- 192.168.1.77/24", text)
        self.assertIn("via: 192.168.1.1", text)
        self.assertIn("- 1.1.1.1", text)
        self.assertIn("- 8.8.8.8", text)

    def test_render_network_config_includes_wifi_with_password(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-77",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.77/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("1.1.1.1", "8.8.8.8"),
            wifi_ssid="GranolaNet2.0",
            wifi_password="super-secret",
            wifi_country="CA",
        )

        text = sbc_cloud_init.render_network_config(config)

        self.assertIn("wifis:", text)
        self.assertIn("wlan0:", text)
        self.assertIn("optional: true", text)
        self.assertIn("regulatory-domain: 'CA'", text)
        self.assertIn("'GranolaNet2.0':", text)
        self.assertIn("password: 'super-secret'", text)

    def test_render_network_config_includes_open_wifi_without_password(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-77",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.77/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("1.1.1.1", "8.8.8.8"),
            wifi_ssid="OpenNet",
            wifi_password=None,
            wifi_country="US",
        )

        text = sbc_cloud_init.render_network_config(config)

        self.assertIn("'OpenNet': {}", text)
        self.assertNotIn("password:", text)

    def test_render_user_data_auto_first_boot_includes_setup_env_and_runcmd(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-90",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.90/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("8.8.8.8",),
            auto_first_boot=True,
            server_base_url="https://timing.example.com",
            auth_token="secret-token",
            reader_targets=("192.168.1.101:10000", "192.168.1.102:10000"),
            status_bind="0.0.0.0:8080",
        )

        text = sbc_cloud_init.render_user_data(config)

        self.assertIn("write_files:", text)
        self.assertIn("/etc/rusty-timer/rt-setup.env", text)
        self.assertIn("RT_SETUP_NONINTERACTIVE=1", text)
        self.assertIn("RT_SETUP_DISPLAY_NAME=rt-fwd-90", text)
        self.assertIn("RT_SETUP_SERVER_BASE_URL=https://timing.example.com", text)
        self.assertIn("RT_SETUP_AUTH_TOKEN=secret-token", text)
        self.assertIn("RT_SETUP_READER_TARGETS=192.168.1.101:10000,192.168.1.102:10000", text)
        self.assertIn("curl -fsSL", text)
        self.assertIn("rt-setup.sh", text)


class AutoModeValidationTests(unittest.TestCase):
    def test_validate_reader_targets_rejects_empty(self) -> None:
        with self.assertRaises(ValueError):
            sbc_cloud_init.parse_reader_targets("")

    def test_validate_reader_targets_parses_comma_separated_values(self) -> None:
        targets = sbc_cloud_init.parse_reader_targets("192.168.1.10:10000,192.168.1.11:10000")
        self.assertEqual(("192.168.1.10:10000", "192.168.1.11:10000"), targets)


class FileOutputTests(unittest.TestCase):
    def test_write_cloud_init_files_writes_expected_paths(self) -> None:
        config = sbc_cloud_init.SbcCloudInitConfig(
            hostname="rt-fwd-11",
            ssh_public_key="ssh-ed25519 AAAATEST user@test",
            static_ipv4_cidr="192.168.1.11/24",
            gateway_ipv4="192.168.1.1",
            dns_servers=("8.8.8.8", "8.8.4.4"),
        )

        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir)
            user_data_path, network_config_path = sbc_cloud_init.write_cloud_init_files(
                config, output_dir
            )

            self.assertEqual(output_dir / "user-data", user_data_path)
            self.assertEqual(output_dir / "network-config", network_config_path)
            self.assertTrue(user_data_path.exists())
            self.assertTrue(network_config_path.exists())
            self.assertIn("hostname: rt-fwd-11", user_data_path.read_text())
            self.assertIn("192.168.1.11/24", network_config_path.read_text())


if __name__ == "__main__":
    unittest.main()
