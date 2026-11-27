import { displayPreferencesPlugin } from "@assemble/display/vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/download/",
  plugins: [displayPreferencesPlugin(), react(), tailwindcss()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1423,
    strictPort: true,
    // 这些都是匿名只读接口；不转发本站登录凭据或伪造浏览器来源。
    proxy: {
      "/v1/releases": {
        target: "http://127.0.0.1:8787",
        changeOrigin: true,
        configure(server) {
          server.on("proxyReq", (request) => {
            for (const key of [
              "cookie",
              "authorization",
              "forwarded",
              "x-forwarded-host",
              "x-forwarded-proto",
            ])
              request.removeHeader(key);
          });
        },
      },
    },
  },
});
