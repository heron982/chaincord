/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [react()],
  root,
  base: "./",
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    outDir: path.resolve(root, "../../dist/public"),
    emptyOutDir: true,
  },
  server: {
    host: "127.0.0.1",
    port: Number(process.env.CHAINCORD_UI_PORT ?? 1420),
    strictPort: true,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});

