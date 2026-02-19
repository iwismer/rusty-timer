import tailwindcss from "@tailwindcss/vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8081",
      "/update": "http://127.0.0.1:8081",
    },
  },
  test: {
    include: ["src/**/*.{test,spec}.{js,ts}"],
  },
});
