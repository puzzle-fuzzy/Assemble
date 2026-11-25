import { fileURLToPath, URL } from "node:url";
import { displayPreferencesPlugin } from "@assemble/display/vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { readBuildInfo } from "../../scripts/release/build-info.mts";

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  define: {
    __ASSEMBLE_BUILD__: JSON.stringify(
      readBuildInfo(fileURLToPath(new URL("../../", import.meta.url))),
    ),
  },
  plugins: [displayPreferencesPlugin(), react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  // 端口固定：Tauri 开发时按固定地址加载前端，被占用时直接失败。
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["TAURI_ENV_*"],
  build: {
    // 构建目标与压缩沿用 Vite 8 的默认值（Oxc，面向 Safari 16 / Chromium 107 起的内核）。
    // Tauri 模板里的 `target: "safari13"` 与 `minify: "esbuild"` 是给旧版 Vite 的写法；
    // 在 Vite 8 里它们会走一条已弃用、需要单独安装 esbuild 的路径。
    // 安装包声明的最低系统版本要与这里的实际目标一致，见 tauri.conf.json。
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});
