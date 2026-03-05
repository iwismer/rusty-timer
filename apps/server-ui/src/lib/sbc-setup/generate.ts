// generate.ts
import type { SbcSetupFormData } from "./types";

const SETUP_SCRIPT_URL =
  "https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh";

function yamlQuote(value: string): string {
  return "'" + value.replace(/'/g, "''") + "'";
}

function shellQuote(value: string): string {
  return "'" + value.replace(/'/g, "'\\''") + "'";
}

function renderSetupEnv(config: SbcSetupFormData): string {
  const targets = config.readerTargets
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean)
    .join(",");

  const lines = [
    "RT_SETUP_NONINTERACTIVE=1",
    "RT_SETUP_ALLOW_POWER_ACTIONS=1",
    "RT_SETUP_OVERWRITE_CONFIG=0",
    "RT_SETUP_RESTART_IF_RUNNING=1",
    `RT_SETUP_DISPLAY_NAME=${shellQuote(config.displayName || config.hostname)}`,
    `RT_SETUP_SERVER_BASE_URL=${shellQuote(config.serverBaseUrl)}`,
    `RT_SETUP_AUTH_TOKEN=${shellQuote(config.authToken)}`,
    `RT_SETUP_READER_TARGETS=${shellQuote(targets)}`,
    `RT_SETUP_STATUS_BIND=${shellQuote(config.statusBind)}`,
    `RT_SETUP_DONE_MARKER=${shellQuote("/var/lib/rusty-timer/.first-boot-setup-done")}`,
  ];
  return lines.join("\n") + "\n";
}

export function generateUserData(config: SbcSetupFormData): string {
  const packages = [
    "avahi-daemon",
    "ca-certificates",
    "jq",
    "curl",
    "tar",
    "coreutils",
  ];
  const packageLines = packages.map((p) => `  - ${p}`).join("\n");

  const setupEnv = renderSetupEnv(config);
  const envContentLines = setupEnv
    .split("\n")
    .filter(Boolean)
    .map((line) => `      ${line}`)
    .join("\n");

  return (
    `#cloud-config\n` +
    `hostname: ${yamlQuote(config.hostname)}\n` +
    `manage_etc_hosts: true\n` +
    `enable_ssh: true\n` +
    `ssh_pwauth: false\n` +
    `\n` +
    `users:\n` +
    `  - name: ${yamlQuote(config.adminUsername)}\n` +
    `    groups: sudo\n` +
    `    shell: /bin/bash\n` +
    `    lock_passwd: true\n` +
    `    sudo: ALL=(ALL) NOPASSWD:ALL\n` +
    `    ssh_authorized_keys:\n` +
    `      - ${yamlQuote(config.sshPublicKey)}\n` +
    `  - name: rt-forwarder\n` +
    `    system: true\n` +
    `    shell: /bin/false\n` +
    `    homedir: /var/lib/rusty-timer\n` +
    `    no_create_home: false\n` +
    `\n` +
    `packages:\n` +
    `${packageLines}\n` +
    `\n` +
    `write_files:\n` +
    `  - path: /etc/rusty-timer/rt-setup.env\n` +
    `    owner: root:root\n` +
    `    permissions: '0600'\n` +
    `    content: |\n` +
    `${envContentLines}\n` +
    `\n` +
    `runcmd:\n` +
    `  - mkdir -p /etc/rusty-timer\n` +
    `  - mkdir -p /var/lib/rusty-timer\n` +
    `  - chown rt-forwarder:rt-forwarder /var/lib/rusty-timer\n` +
    `  - curl -fsSL ${yamlQuote(SETUP_SCRIPT_URL)} -o /var/tmp/rt-setup.sh\n` +
    `  - chmod 0755 /var/tmp/rt-setup.sh\n` +
    `  - bash -lc 'set -a; . /etc/rusty-timer/rt-setup.env; set +a; /var/tmp/rt-setup.sh'\n`
  );
}

export function generateNetworkConfig(config: SbcSetupFormData): string {
  const dnsEntries = config.dnsServers
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const dnsLines = dnsEntries
    .map((s) => `          - ${yamlQuote(s)}`)
    .join("\n");

  let text =
    `network:\n` +
    `  version: 2\n` +
    `  ethernets:\n` +
    `    eth0:\n` +
    `      dhcp4: false\n` +
    `      dhcp6: false\n` +
    `      optional: true\n` +
    `      addresses:\n` +
    `        - ${yamlQuote(config.staticIpv4Cidr)}\n` +
    `      routes:\n` +
    `        - to: default\n` +
    `          via: ${yamlQuote(config.gateway)}\n` +
    `      nameservers:\n` +
    `        addresses:\n` +
    `${dnsLines}\n`;

  if (!config.wifiEnabled || !config.wifiSsid) return text;

  const country = (config.wifiCountry || "US").toUpperCase();
  text +=
    `  wifis:\n` +
    `    wlan0:\n` +
    `      dhcp4: true\n` +
    `      optional: true\n` +
    `      regulatory-domain: ${yamlQuote(country)}\n` +
    `      access-points:\n`;

  if (config.wifiPassword) {
    text += `        ${yamlQuote(config.wifiSsid)}:\n`;
    text += `          password: ${yamlQuote(config.wifiPassword)}\n`;
  } else {
    text += `        ${yamlQuote(config.wifiSsid)}: {}\n`;
  }

  return text;
}
