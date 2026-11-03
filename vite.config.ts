import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import path from "node:path";
import { fileURLToPath } from "node:url";
// @ts-expect-error type error without @types/node package
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;
const rootDir = path.dirname(fileURLToPath(import.meta.url));

function manualChunkFor(id: string): string | undefined {
  if (!id.includes("node_modules")) return undefined;
  if (id.includes("monaco-editor")) return "monaco";
  if (id.includes("@xterm") || id.includes("/xterm/")) return "xterm";
  if (id.includes("naive-ui")) return "naive";
  if (
    id.includes("/vue/") ||
    id.includes("\\vue\\") ||
    id.includes("@vue/") ||
    id.includes("@tauri-apps/")
  ) {
    return "vendor";
  }
  return undefined;
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [vue()],

  // Monaco 0.57 package exports rewrite `monaco-editor/*` → esm `.js`; CSS needs an alias.
  resolve: {
    alias: {
      "monaco-editor-css": path.resolve(
        rootDir,
        "node_modules/monaco-editor/min/vs/editor/editor.main.css",
      ),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // Monaco + language workers are multi‑MB by design and only loaded via
    // defineAsyncComponent / worker import — not first paint (index ~70KB).
    chunkSizeWarningLimit: 8000,
    rollupOptions: {
      output: {
        manualChunks: manualChunkFor,
      },
    },
  },
  optimizeDeps: {
    include: ["monaco-editor"],
  },
  worker: {
    format: "es",
  },
}));
