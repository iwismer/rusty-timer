import tailwindcss from "@tailwindcss/vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { svelteTesting } from "@testing-library/svelte/vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [tailwindcss(), sveltekit(), svelteTesting()],
  define: {
    __BUILD_DATE__: JSON.stringify(new Date().toISOString().split("T")[0]),
  },
  server: {
    proxy: {
      "/api": "http://127.0.0.1:9090",
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.{test,spec}.{js,ts}"],
    setupFiles: ["src/test-setup.ts"],
  },
});
