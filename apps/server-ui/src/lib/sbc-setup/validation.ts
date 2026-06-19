const HOSTNAME_RE = /^[a-z0-9](?:[a-z0-9-]{0,62})$/;
const USERNAME_RE = /^[a-z_][a-z0-9_-]{0,31}$/;
const READER_TARGET_RE = /^(?:\d{1,3}\.){3}\d{1,3}(?:-\d{1,3})?:\d{1,5}$/;
const IPV4_RE = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;
const IPV4_CIDR_RE = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/;

function validIpv4(value: string): boolean {
  const match = IPV4_RE.exec(value);
  if (!match) return false;
  return match.slice(1).every((octet) => {
    const n = Number.parseInt(octet, 10);
    return n >= 0 && n <= 255;
  });
}

export function validateHostname(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Hostname is required");
  if (!HOSTNAME_RE.test(raw)) {
    return new Error(
      "Hostname must use lowercase letters, numbers, and hyphens only",
    );
  }
  return raw;
}

export function validateUsername(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Username is required");
  if (!USERNAME_RE.test(raw)) {
    return new Error(
      "Username must start with a lowercase letter or underscore",
    );
  }
  return raw;
}

export function validateSshKey(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("SSH public key is required");
  if (!raw.startsWith("ssh-")) {
    return new Error("SSH public key must start with ssh-");
  }
  return raw;
}

export function validateIpv4Cidr(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Static IPv4/CIDR is required");
  const match = IPV4_CIDR_RE.exec(raw);
  if (!match) return new Error("Invalid IPv4/CIDR value");
  const octetsValid = match.slice(1, 5).every((octet) => {
    const n = Number.parseInt(octet, 10);
    return n >= 0 && n <= 255;
  });
  const prefix = Number.parseInt(match[5], 10);
  if (!octetsValid || prefix < 0 || prefix > 32) {
    return new Error("Invalid IPv4/CIDR value");
  }
  return raw;
}

export function validateIpv4Address(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("IPv4 address is required");
  if (!validIpv4(raw)) return new Error("Invalid IPv4 address");
  return raw;
}

export function parseDnsServers(value: string): string[] | Error {
  const entries = value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
  if (entries.length === 0)
    return new Error("At least one DNS server is required");
  for (const entry of entries) {
    if (!validIpv4(entry)) return new Error(`Invalid DNS server: ${entry}`);
  }
  return entries;
}

export function validateBaseUrl(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Server URL is required");
  if (!raw.startsWith("http://") && !raw.startsWith("https://")) {
    return new Error("Server URL must start with http:// or https://");
  }
  try {
    const url = new URL(raw);
    if (!url.hostname) return new Error("Server URL must include a hostname");
  } catch {
    return new Error("Server URL must be a valid URL");
  }
  return raw;
}

export function validateReaderTarget(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Reader target is required");
  if (!READER_TARGET_RE.test(raw)) {
    return new Error("Reader target must look like IP:PORT or IP_RANGE:PORT");
  }
  const colon = raw.lastIndexOf(":");
  const ipPart = raw.slice(0, colon);
  const port = Number.parseInt(raw.slice(colon + 1), 10);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return new Error("Port must be between 1 and 65535");
  }
  if (ipPart.includes("-")) {
    const lastDot = ipPart.lastIndexOf(".");
    const base = ipPart.slice(0, lastDot);
    const [startRaw, endRaw] = ipPart.slice(lastDot + 1).split("-");
    const start = Number.parseInt(startRaw, 10);
    const end = Number.parseInt(endRaw, 10);
    if (!validIpv4(`${base}.${startRaw}`) || end < start || end > 255) {
      return new Error("Invalid IP range in reader target");
    }
  } else if (!validIpv4(ipPart)) {
    return new Error("Invalid IP address in reader target");
  }
  return raw;
}

export function parseReaderTargets(value: string): string[] | Error {
  const entries = value
    .replace(/[;\n]/g, ",")
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
  if (entries.length === 0)
    return new Error("At least one reader target is required");
  for (const entry of entries) {
    const result = validateReaderTarget(entry);
    if (result instanceof Error) return result;
  }
  return entries;
}

export function validateStatusBind(value: string): string | Error {
  const raw = value.trim();
  if (!raw) return new Error("Status bind is required");
  const colon = raw.lastIndexOf(":");
  if (colon === -1) return new Error("Status bind must be IP:PORT");
  const ip = raw.slice(0, colon);
  const port = Number.parseInt(raw.slice(colon + 1), 10);
  if (!validIpv4(ip)) return new Error("Invalid IP address in status bind");
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return new Error("Port must be between 1 and 65535");
  }
  return raw;
}

export function validateWifiCountry(value: string): string | Error {
  const raw = value.trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(raw)) {
    return new Error("Wi-Fi country code must be a 2-letter ISO code");
  }
  return raw;
}
