import { displayPreferencesPlugin } from "@assemble/display/vite";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type ProxyOptions } from "vite";

const localServer = "http://127.0.0.1:8787";

function proxy(): ProxyOptions {
  return {
    target: localServer,
    changeOrigin: true,
    configure(server) {
      server.on("proxyReq", (request, incoming) => {
        // 只改写明确的本机开发来源，不能替外部来源获得同源权限。
        if (
          incoming.headers.origin === "http://127.0.0.1:1422" ||
          incoming.headers.origin === "http://localhost:1422"
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
  base: "/movies/",
  plugins: [displayPreferencesPlugin(), react(), tailwindcss()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1422,
    strictPort: true,
    proxy: { "/v1": proxy(), "/web": proxy() },
  },
});
