import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

export default defineConfig(({ mode }) => ({
  clearScreen: false,
  plugins: mode === "kylin" ? [{
    name: "kylin-content-policy",
    transformIndexHtml: html => html.replace("<head>", `<head><meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; frame-src 'none'" />`),
  }] : [],
  resolve: mode === "kylin" ? {
    alias: {
      "./platform": fileURLToPath(new URL("./src/platform-kylin.ts", import.meta.url)),
    },
  } : undefined,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: mode === "kylin" ? "es2019" : "es2021",
    sourcemap: false,
  },
}));
