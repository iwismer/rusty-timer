import type { SbcSetupFormData } from "./types";

const KEY = "sbcSetup";

export type SbcSetupStored = SbcSetupFormData & { ipBaseOctet: number };

export function readSbcSetupPreference(): SbcSetupStored | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    return JSON.parse(raw) as SbcSetupStored;
  } catch {
    return null;
  }
}

export function writeSbcSetupPreference(data: SbcSetupStored): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(KEY, JSON.stringify(data));
  } catch {
    // silently ignore
  }
}

function parseHostnameNumber(
  hostname: string,
): { prefix: string; num: number; width: number } | null {
  const match = /^(.*?)(\d+)$/.exec(hostname);
  if (!match) return null;
  return {
    prefix: match[1],
    num: parseInt(match[2], 10),
    width: match[2].length,
  };
}

function replaceLastOctet(cidr: string, newOctet: number): string {
  return cidr.replace(/\d+(?=\/\d+$)/, String(newOctet));
}

export function autoIncrement(current: {
  hostname: string;
  staticIpv4Cidr: string;
  ipBaseOctet: number;
}): { hostname: string; staticIpv4Cidr: string; ipBaseOctet: number } {
  const parsed = parseHostnameNumber(current.hostname);
  if (!parsed) return { ...current };

  const nextNum = parsed.num + 1;
  const newHostname =
    parsed.prefix + String(nextNum).padStart(parsed.width, "0");
  const newOctet = current.ipBaseOctet + nextNum;
  const newCidr = replaceLastOctet(current.staticIpv4Cidr, newOctet);

  return {
    hostname: newHostname,
    staticIpv4Cidr: newCidr,
    ipBaseOctet: current.ipBaseOctet,
  };
}

export function computeBaseOctet(hostname: string, cidr: string): number {
  const parsed = parseHostnameNumber(hostname);
  if (!parsed) return 0;
  const lastOctetMatch = /(\d+)\/\d+$/.exec(cidr);
  if (!lastOctetMatch) return 0;
  const lastOctet = parseInt(lastOctetMatch[1], 10);
  return lastOctet - parsed.num;
}
