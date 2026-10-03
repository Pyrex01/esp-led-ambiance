import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const esp32Host = env.ESP32_HOST || "http://192.168.4.1";

  return {
    plugins: [react(), viteSingleFile()],
    server: {
      proxy: {
        "/ws": {
          target: esp32Host,
          ws: true,
        },
      },
    },
    build: {
      target: "es2020",
      assetsInlineLimit: 100000000,
      chunkSizeWarningLimit: 100000000,
      cssCodeSplit: false,
      reportCompressedSize: false,
      rollupOptions: {
        output: {
          manualChunks: undefined,
        },
      },
    },
  };
});
