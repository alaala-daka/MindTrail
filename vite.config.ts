import { defineConfig, defaultExclude } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  test: { exclude: [...defaultExclude, "**/.tools/**"] },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/.tools/**"] },
  },
});
