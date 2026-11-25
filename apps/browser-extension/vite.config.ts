import { displayPreferencesPlugin } from "@assemble/display/vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
export default defineConfig({
  base: "./",
  plugins: [displayPreferencesPlugin(), react(), tailwindcss()],
  build: { rollupOptions: { input: "popup.html" } },
  clearScreen: false,
});
