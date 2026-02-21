# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

"""
SBC cloud-init file generator.

Prompts for deployment values, then writes ready-to-copy cloud-init files:
- user-data
- network-config
"""

from __future__ import annotations

import argparse
import ipaddress
import re
import shlex
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT_DIR = REPO_ROOT / "deploy" / "sbc" / "generated"
DEFAULT_HOSTNAME = "rt-fwd-01"
DEFAULT_STATIC_IPV4_CIDR = "192.168.1.50/24"
DEFAULT_GATEWAY = "192.168.1.1"
DEFAULT_DNS = "8.8.8.8,8.8.4.4"
DEFAULT_WIFI_COUNTRY = "US"
DEFAULT_STATUS_BIND = "0.0.0.0:8080"
DEFAULT_SETUP_SCRIPT_URL = (
    "https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh"
)
DEFAULT_DONE_MARKER = "/var/lib/rusty-timer/.first-boot-setup-done"
HOSTNAME_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,62})$")
READER_TARGET_RE = re.compile(
    r"^(?:\d{1,3}\.){3}\d{1,3}(?:-\d{1,3})?:\d{1,5}$"
)


@dataclass(frozen=True)
class SbcCloudInitConfig:
    hostname: str
    ssh_public_key: str
    static_ipv4_cidr: str
    gateway_ipv4: str
    dns_servers: tuple[str, ...]
    wifi_ssid: str | None = None
    wifi_password: str | None = None
    wifi_country: str | None = None
    auto_first_boot: bool = False
    server_base_url: str | None = None
    auth_token: str | None = None
    reader_targets: tuple[str, ...] = ()
    status_bind: str = DEFAULT_STATUS_BIND
    setup_script_url: str = DEFAULT_SETUP_SCRIPT_URL
    setup_done_marker: str = DEFAULT_DONE_MARKER


def yaml_quote(value: str) -> str:
    """Return a YAML-safe single-quoted scalar."""
    return "'" + value.replace("'", "''") + "'"


def shell_quote(value: str) -> str:
    return shlex.quote(value)


def validate_hostname(value: str) -> str:
    hostname = value.strip()
    if not hostname:
        raise ValueError("hostname is required")
    if not HOSTNAME_RE.fullmatch(hostname):
        raise ValueError(
            "hostname must use lowercase letters, numbers, and hyphens only"
        )
    return hostname


def validate_ssh_key(value: str) -> str:
    key = value.strip()
    if not key:
        raise ValueError("SSH public key is required")
    if not key.startswith("ssh-"):
        raise ValueError("SSH key must start with ssh- (for example ssh-ed25519 ...)")
    return key


def validate_ipv4_interface(value: str) -> str:
    raw = value.strip()
    if not raw:
        raise ValueError("static IPv4/CIDR is required")
    try:
        interface = ipaddress.ip_interface(raw)
    except ValueError as exc:
        raise ValueError("invalid static IPv4/CIDR value") from exc
    if interface.version != 4:
        raise ValueError("only IPv4 CIDR values are supported")
    return raw


def validate_ipv4_address(value: str) -> str:
    raw = value.strip()
    if not raw:
        raise ValueError("IPv4 address is required")
    try:
        address = ipaddress.ip_address(raw)
    except ValueError as exc:
        raise ValueError("invalid IPv4 address") from exc
    if address.version != 4:
        raise ValueError("only IPv4 addresses are supported")
    return raw


def parse_dns_servers(value: str) -> tuple[str, ...]:
    entries = [part.strip() for part in value.split(",") if part.strip()]
    if not entries:
        raise ValueError("at least one DNS server is required")
    validated = [validate_ipv4_address(item) for item in entries]
    return tuple(validated)


def validate_base_url(value: str) -> str:
    url = value.strip()
    if not url:
        raise ValueError("server base URL is required")
    if not (url.startswith("http://") or url.startswith("https://")):
        raise ValueError("server base URL must start with http:// or https://")
    return url


def validate_non_empty(value: str, label: str) -> str:
    text = value.strip()
    if not text:
        raise ValueError(f"{label} is required")
    return text


def validate_reader_target(value: str) -> str:
    target = value.strip()
    if not target:
        raise ValueError("reader target is required")
    if not READER_TARGET_RE.fullmatch(target):
        raise ValueError(
            "reader target must look like IP:PORT or IP_RANGE:PORT "
            "(e.g. 192.168.1.10:10000 or 192.168.1.150-160:10000)"
        )
    return target


def validate_wifi_country(value: str) -> str:
    country = value.strip().upper()
    if not re.fullmatch(r"[A-Z]{2}", country):
        raise ValueError("Wi-Fi country code must be a 2-letter ISO code (for example US)")
    return country


def parse_reader_targets(value: str) -> tuple[str, ...]:
    normalized = value.replace("\n", ",").replace(";", ",")
    entries = [part.strip() for part in normalized.split(",") if part.strip()]
    if not entries:
        raise ValueError("at least one reader target is required")
    return tuple(validate_reader_target(item) for item in entries)


def ask_with_default(label: str, default: str) -> str:
    response = input(f"{label} [{default}]: ").strip()
    return response if response else default


def ask_required(label: str) -> str:
    return input(f"{label}: ").strip()


def ask_yes_no(label: str, default: bool = False) -> bool:
    suffix = "[Y/n]" if default else "[y/N]"
    while True:
        answer = input(f"{label} {suffix}: ").strip().lower()
        if not answer:
            return default
        if answer in {"y", "yes"}:
            return True
        if answer in {"n", "no"}:
            return False
        print("Please answer y or n.")


def prompt_until_valid(prompt_fn, validator, error_label: str) -> str:
    while True:
        value = prompt_fn()
        try:
            return validator(value)
        except ValueError as exc:
            print(f"Invalid {error_label}: {exc}")


def collect_config(auto_first_boot: bool) -> SbcCloudInitConfig:
    print("Rusty Timer SBC cloud-init file generator")
    print("")

    hostname = prompt_until_valid(
        lambda: ask_with_default("Hostname", DEFAULT_HOSTNAME),
        validate_hostname,
        "hostname",
    )
    ssh_public_key = prompt_until_valid(
        lambda: ask_required("SSH public key"),
        validate_ssh_key,
        "SSH public key",
    )
    static_ipv4_cidr = prompt_until_valid(
        lambda: ask_with_default("Static IPv4/CIDR for eth0", DEFAULT_STATIC_IPV4_CIDR),
        validate_ipv4_interface,
        "static IPv4/CIDR",
    )
    gateway_ipv4 = prompt_until_valid(
        lambda: ask_with_default("Default gateway", DEFAULT_GATEWAY),
        validate_ipv4_address,
        "default gateway",
    )
    dns_servers = prompt_until_valid(
        lambda: ask_with_default("DNS servers (comma-separated)", DEFAULT_DNS),
        parse_dns_servers,
        "DNS server list",
    )
    wifi_ssid: str | None = None
    wifi_password: str | None = None
    wifi_country: str | None = None
    if ask_yes_no("Configure Wi-Fi (wlan0) in network-config?", default=False):
        wifi_ssid = prompt_until_valid(
            lambda: ask_required("Wi-Fi SSID"),
            lambda value: validate_non_empty(value, "Wi-Fi SSID"),
            "Wi-Fi SSID",
        )
        entered_password = input("Wi-Fi password (leave blank for open network): ")
        wifi_password = entered_password if entered_password else None
        wifi_country = prompt_until_valid(
            lambda: ask_with_default("Wi-Fi country code", DEFAULT_WIFI_COUNTRY),
            validate_wifi_country,
            "Wi-Fi country code",
        )

    if not auto_first_boot:
        return SbcCloudInitConfig(
            hostname=hostname,
            ssh_public_key=ssh_public_key,
            static_ipv4_cidr=static_ipv4_cidr,
            gateway_ipv4=gateway_ipv4,
            dns_servers=dns_servers,
            wifi_ssid=wifi_ssid,
            wifi_password=wifi_password,
            wifi_country=wifi_country,
            auto_first_boot=False,
        )

    print("")
    print("Automatic first-boot forwarder setup")
    server_base_url = prompt_until_valid(
        lambda: ask_required("Server base URL"),
        validate_base_url,
        "server base URL",
    )
    auth_token = prompt_until_valid(
        lambda: ask_required("Forwarder auth token"),
        lambda value: validate_non_empty(value, "forwarder auth token"),
        "forwarder auth token",
    )
    reader_targets = prompt_until_valid(
        lambda: ask_required("Reader targets (comma-separated)"),
        parse_reader_targets,
        "reader targets",
    )
    status_bind = prompt_until_valid(
        lambda: ask_with_default("Status HTTP bind", DEFAULT_STATUS_BIND),
        lambda value: validate_non_empty(value, "status bind"),
        "status bind",
    )

    return SbcCloudInitConfig(
        hostname=hostname,
        ssh_public_key=ssh_public_key,
        static_ipv4_cidr=static_ipv4_cidr,
        gateway_ipv4=gateway_ipv4,
        dns_servers=dns_servers,
        wifi_ssid=wifi_ssid,
        wifi_password=wifi_password,
        wifi_country=wifi_country,
        auto_first_boot=True,
        server_base_url=server_base_url,
        auth_token=auth_token,
        reader_targets=reader_targets,
        status_bind=status_bind,
    )


def render_setup_env_content(config: SbcCloudInitConfig) -> str:
    if not config.auto_first_boot:
        return ""
    if not config.server_base_url or not config.auth_token or not config.reader_targets:
        raise ValueError("auto-first-boot config is missing setup values")

    reader_targets_csv = ",".join(config.reader_targets)
    lines = (
        "RT_SETUP_NONINTERACTIVE=1",
        "RT_SETUP_OVERWRITE_CONFIG=0",
        "RT_SETUP_RESTART_IF_RUNNING=1",
        f"RT_SETUP_DISPLAY_NAME={shell_quote(config.hostname)}",
        f"RT_SETUP_SERVER_BASE_URL={shell_quote(config.server_base_url)}",
        f"RT_SETUP_AUTH_TOKEN={shell_quote(config.auth_token)}",
        f"RT_SETUP_READER_TARGETS={shell_quote(reader_targets_csv)}",
        f"RT_SETUP_STATUS_BIND={shell_quote(config.status_bind)}",
        f"RT_SETUP_DONE_MARKER={shell_quote(config.setup_done_marker)}",
    )
    return "\n".join(lines) + "\n"


def render_user_data(config: SbcCloudInitConfig) -> str:
    packages = ["ca-certificates", "jq"]
    if config.auto_first_boot:
        packages.extend(["curl", "tar", "coreutils"])

    package_lines = "\n".join(f"  - {pkg}" for pkg in packages)
    text = (
        "#cloud-config\n"
        f"hostname: {config.hostname}\n"
        "manage_etc_hosts: true\n"
        "enable_ssh: true\n"
        "ssh_pwauth: false\n"
        "\n"
        "users:\n"
        "  - default\n"
        "  - name: rt-forwarder\n"
        "    system: true\n"
        "    shell: /bin/false\n"
        "    homedir: /var/lib/rusty-timer\n"
        "    no_create_home: false\n"
        "\n"
        "packages:\n"
        f"{package_lines}\n"
        "\n"
        "ssh_authorized_keys:\n"
        f"  - {yaml_quote(config.ssh_public_key)}\n"
        "\n"
    )

    if config.auto_first_boot:
        setup_env = render_setup_env_content(config)
        setup_script_url = yaml_quote(config.setup_script_url)
        text += (
            "write_files:\n"
            "  - path: /etc/rusty-timer/rt-setup.env\n"
            "    owner: root:root\n"
            "    permissions: '0600'\n"
            "    content: |\n"
            + "".join(f"      {line}\n" for line in setup_env.splitlines())
            + "\n"
        )
        text += (
            "runcmd:\n"
            "  - mkdir -p /etc/rusty-timer\n"
            "  - mkdir -p /var/lib/rusty-timer\n"
            "  - chown rt-forwarder:rt-forwarder /var/lib/rusty-timer\n"
            f"  - curl -fsSL {setup_script_url} -o /var/tmp/rt-setup.sh\n"
            "  - chmod 0755 /var/tmp/rt-setup.sh\n"
            "  - bash -lc 'set -a; . /etc/rusty-timer/rt-setup.env; set +a; /var/tmp/rt-setup.sh'\n"
        )
        return text

    text += (
        "runcmd:\n"
        "  - mkdir -p /etc/rusty-timer\n"
        "  - mkdir -p /var/lib/rusty-timer\n"
        "  - chown rt-forwarder:rt-forwarder /var/lib/rusty-timer\n"
    )
    return text


def render_network_config(config: SbcCloudInitConfig) -> str:
    dns_lines = "\n".join(f"          - {server}" for server in config.dns_servers)
    text = (
        "network:\n"
        "  version: 2\n"
        "  ethernets:\n"
        "    eth0:\n"
        "      dhcp4: false\n"
        "      dhcp6: false\n"
        "      optional: true\n"
        "      addresses:\n"
        f"        - {config.static_ipv4_cidr}\n"
        "      routes:\n"
        "        - to: default\n"
        f"          via: {config.gateway_ipv4}\n"
        "      nameservers:\n"
        "        addresses:\n"
        f"{dns_lines}\n"
    )
    if not config.wifi_ssid:
        return text

    wifi_country = config.wifi_country or DEFAULT_WIFI_COUNTRY
    wifi_ssid = yaml_quote(config.wifi_ssid)
    text += (
        "  wifis:\n"
        "    wlan0:\n"
        "      dhcp4: true\n"
        "      optional: true\n"
        f"      regulatory-domain: {yaml_quote(wifi_country)}\n"
        "      access-points:\n"
    )
    if config.wifi_password:
        text += (
            f"        {wifi_ssid}:\n"
            f"          password: {yaml_quote(config.wifi_password)}\n"
        )
    else:
        text += f"        {wifi_ssid}: {{}}\n"
    return text


def write_cloud_init_files(
    config: SbcCloudInitConfig, output_dir: Path
) -> tuple[Path, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    user_data_path = output_dir / "user-data"
    network_config_path = output_dir / "network-config"

    user_data_path.write_text(render_user_data(config), encoding="utf-8")
    network_config_path.write_text(render_network_config(config), encoding="utf-8")
    return user_data_path, network_config_path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Generate SBC cloud-init files (user-data + network-config) "
            "for Rusty Timer deployments."
        )
    )
    parser.add_argument(
        "--output-dir",
        default=str(DEFAULT_OUTPUT_DIR),
        help="Directory where user-data and network-config will be written",
    )
    parser.add_argument(
        "--auto-first-boot",
        action="store_true",
        help=(
            "Embed a non-interactive first-boot setup run so the SBC installs "
            "and configures rt-forwarder automatically."
        ),
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    output_dir = Path(args.output_dir).expanduser()

    config = collect_config(auto_first_boot=args.auto_first_boot)
    user_data_path, network_config_path = write_cloud_init_files(config, output_dir)

    print("")
    print("Generated files:")
    print(f"  {user_data_path}")
    print(f"  {network_config_path}")
    print("")
    if args.auto_first_boot:
        print("First boot mode: automatic forwarder install/config is enabled.")
    print("Next step: copy both files to the SD card boot partition.")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nCancelled by user.")
        sys.exit(130)
