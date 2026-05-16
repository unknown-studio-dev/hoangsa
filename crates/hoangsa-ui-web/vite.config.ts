import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// Build output goes to ./dist which is embedded into hoangsa-ui-server's
// binary via rust-embed. The Rust build does NOT trigger npm — run
// `make ui` from the repo root before `cargo build` to refresh the bundle.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    // Dev: proxy /api to a locally-running hoangsa-ui server. Token still
    // required — append `?t=...` manually in dev or set the env var below.
    proxy: {
      "/api": "http://127.0.0.1:8000",
    },
  },
});
