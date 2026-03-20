import { getVersion } from "@tauri-apps/api/app";
import { check } from "@tauri-apps/plugin-updater";

export type DesktopUpdateInfo = {
  currentVersion: string;
  version: string;
  notes: string | null;
  publishedAt: string | null;
};

type DesktopVersionInfo = {
  supported: boolean;
  version: string | null;
};

type DesktopUpdateCheckResult = {
  supported: boolean;
  update: DesktopUpdateInfo | null;
};

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: unknown;
};

function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in (window as TauriWindow)
  );
}

export async function loadDesktopVersion(): Promise<DesktopVersionInfo> {
  if (!isTauriRuntime()) {
    return { supported: false, version: null };
  }

  return { supported: true, version: await getVersion() };
}

export async function checkForDesktopUpdate(): Promise<DesktopUpdateCheckResult> {
  if (!isTauriRuntime()) {
    return { supported: false, update: null };
  }

  const currentVersion = await getVersion();
  const update = await check();
  if (!update) {
    return { supported: true, update: null };
  }

  return {
    supported: true,
    update: {
      currentVersion,
      version: update.version,
      notes: update.body ?? null,
      publishedAt: update.date ?? null,
    },
  };
}

export async function installDesktopUpdate(): Promise<void> {
  if (!isTauriRuntime()) return;

  const update = await check();
  if (!update) return;

  await update.downloadAndInstall();
}
