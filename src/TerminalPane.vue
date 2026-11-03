<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const props = defineProps<{
  unitId: string;
  active: boolean;
}>();

const host = ref<HTMLDivElement | null>(null);
let term: Terminal | null = null;
let fit: FitAddon | null = null;
let unlisten: UnlistenFn | undefined;
let ro: ResizeObserver | undefined;

function decodeB64(data: string): Uint8Array {
  const bin = atob(data);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function doFit() {
  if (!term || !fit || !host.value) return;
  try {
    fit.fit();
    const dims = fit.proposeDimensions();
    if (dims && props.unitId) {
      void invoke("term_resize", {
        unitId: props.unitId,
        cols: dims.cols,
        rows: dims.rows,
      });
    }
  } catch {
    /* ignore */
  }
}

onMounted(async () => {
  term = new Terminal({
    cursorBlink: true,
    fontFamily: '"JetBrains Mono", "IBM Plex Mono", Consolas, monospace',
    fontSize: 12,
    lineHeight: 1.25,
    theme: {
      background: "#1e1e1e",
      foreground: "#cccccc",
      cursor: "#aeafad",
      selectionBackground: "#264f78",
      black: "#000000",
      red: "#cd3131",
      green: "#0dbc79",
      yellow: "#e5e510",
      blue: "#2472c8",
      magenta: "#bc3fbc",
      cyan: "#11a8cd",
      white: "#e5e5e5",
      brightBlack: "#666666",
      brightRed: "#f14c4c",
      brightGreen: "#23d18b",
      brightYellow: "#f5f543",
      brightBlue: "#3b8eea",
      brightMagenta: "#d670d6",
      brightCyan: "#29b8db",
      brightWhite: "#ffffff",
    },
    convertEol: true,
  });
  fit = new FitAddon();
  term.loadAddon(fit);
  // Selection + Ctrl+C → copy; bare Ctrl+C (no selection) → still interrupt (\\x03) via onData.
  // Ctrl+Shift+C / Ctrl+Shift+V → copy / paste (Windows Terminal style).
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown" || !term) return true;
    const mod = ev.ctrlKey || ev.metaKey;
    const key = ev.key.toLowerCase();
    if (mod && key === "c" && !ev.shiftKey && term.hasSelection()) {
      ev.preventDefault();
      void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
      return false;
    }
    if (mod && ev.shiftKey && key === "c") {
      ev.preventDefault();
      if (term.hasSelection()) {
        void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
      }
      return false;
    }
    if (mod && ev.shiftKey && key === "v") {
      ev.preventDefault();
      void navigator.clipboard.readText().then((t) => {
        if (t && props.unitId) void invoke("term_write", { unitId: props.unitId, data: t });
      });
      return false;
    }
    return true;
  });
  if (host.value) {
    term.open(host.value);
    doFit();
  }

  term.onData((data) => {
    if (!props.unitId) return;
    void invoke("term_write", { unitId: props.unitId, data });
  });

  unlisten = await listen<{ unitId: string; data: string }>("module-term", (ev) => {
    if (ev.payload.unitId !== props.unitId || !term) return;
    term.write(decodeB64(ev.payload.data));
  });

  ro = new ResizeObserver(() => doFit());
  if (host.value) ro.observe(host.value);
});

watch(
  () => props.active,
  (v) => {
    if (v) {
      requestAnimationFrame(() => {
        doFit();
        term?.focus();
      });
    }
  },
);

watch(
  () => props.unitId,
  () => {
    term?.reset();
    doFit();
  },
);

onBeforeUnmount(() => {
  unlisten?.();
  ro?.disconnect();
  term?.dispose();
  term = null;
  fit = null;
});

defineExpose({
  clear() {
    term?.clear();
  },
  focus() {
    term?.focus();
  },
});
</script>

<template>
  <div class="xterm-host" ref="host" />
</template>

<style scoped>
.xterm-host {
  width: 100%;
  height: 100%;
  min-height: 120px;
  padding: 4px 6px;
  box-sizing: border-box;
  background: #1e1e1e;
}
.xterm-host :deep(.xterm) {
  height: 100%;
}
.xterm-host :deep(.xterm-viewport) {
  overflow-y: auto !important;
}
</style>
