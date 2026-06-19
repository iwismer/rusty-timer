import type { SbcSetupFormData } from "./types";

const KEY = "rusty-timer-sbc-setup";

type PersistedForm = Omit<SbcSetupFormData, "authToken" | "wifiPassword"> & {
  authToken?: never;
  wifiPassword?: never;
};

export interface SbcSetupStored {
  form: SbcSetupFormData;
  ipBaseOctet: number;
}

interface StoredPayload {
  form: PersistedForm;
  ipBaseOctet: number;
}

export function writeSbcSetupPreference(data: SbcSetupStored): boolean {
  if (typeof localStorage === "undefined") return false;
  const {
    authToken: _authToken,
    wifiPassword: _wifiPassword,
    ...safeForm
  } = data.form;
  const payload: StoredPayload = {
    form: safeForm,
    ipBaseOctet: data.ipBaseOctet,
  };
  try {
    localStorage.setItem(KEY, JSON.stringify(payload));
    return true;
  } catch (error) {
    console.warn("sbc setup: failed to write preferences", error);
    return false;
  }
}

export function readSbcSetupPreference(): SbcSetupStored | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredPayload>;
    if (typeof parsed.form?.hostname !== "string") return null;
    return {
      form: {
        ...parsed.form,
        authToken: "",
        wifiPassword: "",
      } as SbcSetupFormData,
      ipBaseOctet:
        typeof parsed.ipBaseOctet === "number" ? parsed.ipBaseOctet : 0,
    };
  } catch (error) {
    console.warn("sbc setup: failed to read preferences", error);
    return null;
  }
}

function parseHostnameNumber(
  hostname: string,
): { prefix: string; num: number; width: number } | null {
  const match = /^(.*?)(\d+)$/.exec(hostname);
  if (!match) return null;
  return {
    prefix: match[1],
    num: Number.parseInt(match[2], 10),
    width: match[2].length,
  };
}

export function computeBaseOctet(hostname: string, cidr: string): number {
  const parsed = parseHostnameNumber(hostname);
  const lastOctetMatch = /(\d+)\/\d+$/.exec(cidr);
  if (!parsed || !lastOctetMatch) return 0;
  return Math.max(0, Number.parseInt(lastOctetMatch[1], 10) - parsed.num);
}

function replaceLastOctet(cidr: string, lastOctet: number): string {
  return cidr.replace(/\d+(?=\/\d+$)/, String(lastOctet));
}

export function autoIncrement(current: {
  hostname: string;
  staticIpv4Cidr: string;
  ipBaseOctet: number;
}): { hostname: string; staticIpv4Cidr: string; ipBaseOctet: number } {
  const parsed = parseHostnameNumber(current.hostname);
  if (!parsed) return { ...current };
  const nextNum = parsed.num + 1;
  const nextOctet = current.ipBaseOctet + nextNum;
  if (nextOctet > 255) return { ...current };
  return {
    hostname: `${parsed.prefix}${String(nextNum).padStart(parsed.width, "0")}`,
    staticIpv4Cidr: replaceLastOctet(current.staticIpv4Cidr, nextOctet),
    ipBaseOctet: current.ipBaseOctet,
  };
}
