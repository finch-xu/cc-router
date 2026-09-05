import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // 不能写 false/localhost: Windows 上 Vite 会只绑定 ::1, 而 Tauri CLI 的
    // dev server 健康检查走 127.0.0.1, 导致永远卡在 "Waiting for your frontend
    // dev server". 必须与 tauri.conf.json::build.devUrl 同为 IPv4 字面量.
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
