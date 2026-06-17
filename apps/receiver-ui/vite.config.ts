import tailwindcss from "@tailwindcss/vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { svelteTesting } from "@testing-library/svelte/vite";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

// In `--mode e2e`, redirect the Tauri IPC packages to the HTTP/SSE bridge shim
// so the SPA can run against the headless host without a Tauri webview. Every
// other mode (dev/build/test) keeps using the real Tauri package.
//
// Implemented as a `resolveId` plugin rather than `resolve.alias` so the
// replacement can be an absolute path derived from Vite's `config.root`
// (a plain string) — the repo ships no `@types/node`, so Node path/url
// helpers are unavailable in this config.
function tauriBridgeShimPlugin(): Plugin {
  const aliased = new Set(["@tauri-apps/api/core", "@tauri-apps/api/event"]);
  let shimPath = "";
  return {
    name: "tauri-bridge-shim-e2e",
    enforce: "pre",
    configResolved(config) {
      shimPath = `${config.root}/src/lib/tauri-bridge-shim.ts`;
    },
    resolveId(id) {
      return aliased.has(id) ? shimPath : null;
    },
  };
}

export default defineConfig(({ mode }) => ({
  server: {
    host: "127.0.0.1",
  },
  plugins: [
    tailwindcss(),
    sveltekit(),
    svelteTesting(),
    ...(mode === "e2e" ? [tauriBridgeShimPlugin()] : []),
  ],
  define: {
    __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.{test,spec}.{js,ts}"],
    setupFiles: ["src/test-setup.ts"],
  },
}));
