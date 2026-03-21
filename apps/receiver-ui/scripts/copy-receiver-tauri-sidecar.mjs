#!/usr/bin/env node
/**
 * Stage the receiver sidecar next to receiver-tauri in target/debug so
 * tauri-plugin-shell `sidecar("receiver")` can find it (matches NSIS layout).
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(__dirname, "..");
const binDir = path.join(appRoot, "src-tauri", "binaries");
const targetDebug = path.join(appRoot, "..", "..", "target", "debug");
const destName = process.platform === "win32" ? "receiver.exe" : "receiver";

let entries;
try {
  entries = fs.readdirSync(binDir);
} catch {
  process.exit(0);
}

const srcName = entries.find(
  (n) => n.startsWith("receiver-") && !n.endsWith(".sha256"),
);
if (!srcName) {
  process.exit(0);
}

const src = path.join(binDir, srcName);
const dest = path.join(targetDebug, destName);
fs.mkdirSync(targetDebug, { recursive: true });
fs.copyFileSync(src, dest);
