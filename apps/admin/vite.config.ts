import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type ProxyOptions } from "vite";

const localServer = "http://127.0.0.1:8787";

// 开发代理只改写来自本机管理端的 Origin，外部来源仍由服务端拒绝。
function proxy(): ProxyOptions {
  return {
    target: localServer,
    changeOrigin: true,
    configure(server) {
      server.on("proxyReq", (request, incoming) => {
        const origin = incoming.headers.origin;
        if (
          origin === "http://127.0.0.1:1421" ||
          origin === "http://localhost:1421"
        ) {
          request.setHeader("origin", localServer);
        }
        request.removeHeader("forwarded");
        request.removeHeader("x-forwarded-host");
        request.removeHeader("x-forwarded-proto");
      });
    },
  };
}

export default defineConfig({
  base: "/admin/",
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1421,
    strictPort: true,
    proxy: { "/v1": proxy(), "/web": proxy() },
  },
});
