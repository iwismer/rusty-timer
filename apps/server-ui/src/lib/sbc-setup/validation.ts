const HOSTNAME_RE = /^[a-z0-9](?:[a-z0-9-]{0,62})$/;
const USERNAME_RE = /^[a-z_][a-z0-9_-]{0,31}$/;
const READER_TARGET_RE = /^(?:\d{1,3}\.){3}\d{1,3}(?:-\d{1,3})?:\d{1,5}$/;
const IPV4_RE = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;
const IPV4_CIDR_RE = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/;
const WIFI_COUNTRY_RE = /^[A-Z]{2}$/;

function isValidIpv4(ip: string): boolean {
  const m = IPV4_RE.exec(ip);
  if (!m) return false;
  return m.slice(1).every((octet) => {
    const n = parseInt(octet, 10);
    return n >= 0 && n <= 255;
  });
}

export function validateHostname(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("Hostname is required");
  if (!HOSTNAME_RE.test(v))
    return new Error(
      "Hostname must use lowercase letters, numbers, and hyphens only",
    );
  return v;
}

export function validateUsername(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("Username is required");
  if (!USERNAME_RE.test(v))
    return new Error(
      "Username must start with a lowercase letter or underscore",
    );
  return v;
}

const SSH_KEY_PREFIXES = ["ssh-", "ecdsa-sha2-", "sk-ssh-", "sk-ecdsa-"];

export function validateSshKey(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("SSH public key is required");
  if (!SSH_KEY_PREFIXES.some((p) => v.startsWith(p)))
    return new Error(
      "SSH key must start with ssh-, ecdsa-sha2-, sk-ssh-, or sk-ecdsa-",
    );
  return v;
}

export function validateIpv4Cidr(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("Static IPv4/CIDR is required");
  const m = IPV4_CIDR_RE.exec(v);
  if (!m) return new Error("Invalid IPv4/CIDR format (e.g. 192.168.1.50/24)");
  const octetsValid = m.slice(1, 5).every((o) => {
    const n = parseInt(o, 10);
    return n >= 0 && n <= 255;
  });
  const prefix = parseInt(m[5], 10);
  if (!octetsValid || prefix < 0 || prefix > 32)
    return new Error("Invalid IPv4/CIDR format (e.g. 192.168.1.50/24)");
  return v;
}

export function validateIpv4Address(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("IPv4 address is required");
  if (!isValidIpv4(v)) return new Error("Invalid IPv4 address");
  return v;
}

export function parseDnsServers(value: string): string[] | Error {
  const entries = value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (entries.length === 0)
    return new Error("At least one DNS server is required");
  for (const entry of entries) {
    if (!isValidIpv4(entry)) return new Error(`Invalid DNS server: ${entry}`);
  }
  return entries;
}

export function validateBaseUrl(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("Server base URL is required");
  try {
    const url = new URL(v);
    if (url.protocol !== "http:" && url.protocol !== "https:")
      return new Error("Server base URL must start with http:// or https://");
    if (!url.hostname)
      return new Error("Server base URL must include a hostname");
    return v;
  } catch {
    return new Error("Server base URL must be a valid URL");
  }
}

export function validateReaderTarget(value: string): string | Error {
  const v = value.trim();
  if (!v) return new Error("Reader target is required");
  if (!READER_TARGET_RE.test(v))
    return new Error(
      "Reader target must be IP:PORT or IP_RANGE:PORT (e.g. 192.168.1.10:10000)",
    );

  const colonIdx = v.lastIndexOf(":");
  const ipPart = v.substring(0, colonIdx);
  const portStr = v.substring(colonIdx + 1);
  const port = parseInt(portStr, 10);
  if (port < 1 || port > 65535)
    return new Error("Port must be between 1 and 65535");

  if (ipPart.includes("-")) {
    const lastDot = ipPart.lastIndexOf(".");
    const baseIp = ipPart.substring(0, lastDot);
    const rangePart = ipPart.substring(lastDot + 1);
    const [startStr, endStr] = rangePart.split("-");
    const start = parseInt(startStr, 10);
    const end = parseInt(endStr, 10);
    if (!isValidIpv4(baseIp + "." + startStr))
      return new Error("Invalid IP address in reader target");
    if (end < 0 || end > 255 || end < start)
      return new Error("Invalid IP range in reader target");
  } else {
    if (!isValidIpv4(ipPart))
      return new Error("Invalid IP address in reader target");
  }

  return v;
}

export function parseReaderTargets(value: string): string[] | Error {
  const entries = value
    .split("\n")
    .map((s) => s.trim())
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
  const v = value.trim();
  if (!v) return new Error("Status bind address is required");
  const colonIdx = v.lastIndexOf(":");
  if (colonIdx === -1)
    return new Error("Status bind must be IP:PORT (e.g. 0.0.0.0:80)");
  const ip = v.substring(0, colonIdx);
  const portStr = v.substring(colonIdx + 1);
  if (!isValidIpv4(ip)) return new Error("Invalid IP address in status bind");
  const port = parseInt(portStr, 10);
  if (isNaN(port) || port < 1 || port > 65535)
    return new Error("Port must be between 1 and 65535");
  return v;
}

export function validateWifiCountry(value: string): string | Error {
  const v = value.trim().toUpperCase();
  if (!WIFI_COUNTRY_RE.test(v))
    return new Error("Wi-Fi country code must be a 2-letter ISO code");
  return v;
}
