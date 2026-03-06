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
      "/api": "http://localhost:8080",
    },
  },
  test: {
    include: ["src/**/*.{test,spec}.{js,ts}"],
    environment: "jsdom",
    setupFiles: ["src/test-setup.ts"],
  },
});
