import type { Plugin } from "vite";
import { bootstrapDisplay, DISPLAY_STORAGE_KEY } from "./bootstrap.ts";

// 管理端 CSP 只允许同源脚本；用阻塞的外部脚本在 CSS 绘制前恢复外观。
export function displayPreferencesPlugin(): Plugin {
  const filename = "display-preferences.js";
  const source = `(${bootstrapDisplay.toString()})(window,document,${JSON.stringify(DISPLAY_STORAGE_KEY)});`;
  let base = "/";
  return {
    name: "assemble-display-preferences",
    configResolved(config) {
      base = config.base;
    },
    transformIndexHtml: {
      order: "post",
      handler() {
        return [
          {
            tag: "meta",
            attrs: { name: "color-scheme", content: "light dark" },
            injectTo: "head-prepend",
          },
          {
            tag: "script",
            attrs: { src: `${base}${filename}` },
            injectTo: "head-prepend",
          },
        ];
      },
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url?.split("?")[0] !== `${base}${filename}`) {
          next();
          return;
        }
        response.setHeader("Content-Type", "text/javascript; charset=utf-8");
        response.setHeader("Cache-Control", "no-store");
        response.end(source);
      });
    },
    generateBundle() {
      this.emitFile({ type: "asset", fileName: filename, source });
    },
  };
}
