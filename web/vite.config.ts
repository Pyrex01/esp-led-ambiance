import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  build: {
    assetsInlineLimit: 8192,
    cssCodeSplit: false,
    emptyOutDir: true,
    modulePreload: false,
    reportCompressedSize: true,
    rollupOptions: {
      output: {
        manualChunks: undefined
      }
    },
    sourcemap: false,
    target: "es2020"
  }
});
