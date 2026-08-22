import { defineConfig } from "vite";

// The frontend is a presentation layer only: it turns gestures into commands
// and renders state. Keep it dependency-light — no musical logic lives here.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust sources are watched by the Tauri CLI, not by Vite.
      ignored: ["**/src-tauri/**", "**/core/**", "**/target/**"],
    },
  },
  build: {
    outDir: "dist",
    target: "safari15",
    sourcemap: true,
  },
});
