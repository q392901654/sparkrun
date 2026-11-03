<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const props = withDefaults(
  defineProps<{
    active?: boolean;
    /** When true, new writes scroll to bottom */
    follow?: boolean;
  }>(),
  { active: true, follow: true },
);

const emit = defineEmits<{
  followChange: [value: boolean];
}>();

const host = ref<HTMLDivElement | null>(null);
let term: Terminal | null = null;
let fit: FitAddon | null = null;
let ro: ResizeObserver | undefined;
let ignoreScroll = false;

function doFit() {
  if (!term || !fit || !host.value) return;
  try {
    fit.fit();
  } catch {
    /* ignore */
  }
}

function scrollToBottom() {
  if (!term) return;
  ignoreScroll = true;
  term.scrollToBottom();
  requestAnimationFrame(() => {
    ignoreScroll = false;
  });
}

function onScroll() {
  if (!term || ignoreScroll || !props.follow) return;
  const buf = term.buffer.active;
  const atBottom = buf.viewportY + term.rows >= buf.length - 1;
  if (!atBottom) {
    emit("followChange", false);
  }
}

async function copySelection() {
  if (!term?.hasSelection()) return;
  const text = term.getSelection();
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    /* WebView clipboard may deny; ignore */
  }
}

onMounted(() => {
  // disableStdin=true disables the helper textarea → Ctrl+C never reaches handlers.
  // Keep stdin "open" for shortcuts, but swallow all keys so nothing is typed into the log.
  term = new Terminal({
    cursorBlink: false,
    disableStdin: false,
    fontFamily: '"JetBrains Mono", "IBM Plex Mono", Consolas, monospace',
    fontSize: 12,
    lineHeight: 1.25,
    scrollback: 5000,
    rightClickSelectsWord: true,
    theme: {
      background: "#1e1e1e",
      foreground: "#cccccc",
      cursor: "#1e1e1e",
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
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return false;
    const mod = ev.ctrlKey || ev.metaKey;
    const key = ev.key.toLowerCase();
    if (mod && key === "c") {
      if (term?.hasSelection()) {
        ev.preventDefault();
        void copySelection();
      }
      return false;
    }
    if (mod && key === "a") {
      ev.preventDefault();
      term?.selectAll();
      return false;
    }
    // Allow scroll keys; block typing into read-only log
    if (
      ev.key === "ArrowUp" ||
      ev.key === "ArrowDown" ||
      ev.key === "PageUp" ||
      ev.key === "PageDown" ||
      ev.key === "Home" ||
      ev.key === "End"
    ) {
      return true;
    }
    return false;
  });
  if (host.value) {
    term.open(host.value);
    doFit();
  }
  term.onScroll(() => onScroll());
  ro = new ResizeObserver(() => doFit());
  if (host.value) ro.observe(host.value);
});

watch(
  () => props.active,
  (v) => {
    if (v) requestAnimationFrame(() => doFit());
  },
);

watch(
  () => props.follow,
  (v) => {
    if (v) scrollToBottom();
  },
);

onBeforeUnmount(() => {
  ro?.disconnect();
  term?.dispose();
  term = null;
  fit = null;
});

defineExpose({
  write(chunk: string) {
    if (!term || !chunk) return;
    term.write(chunk);
    if (props.follow) scrollToBottom();
  },
  clear() {
    term?.clear();
    term?.reset();
  },
  /** Replace view with pre-formatted ANSI chunks (already include \\r\\n). */
  replay(chunks: string[]) {
    if (!term) return;
    term.reset();
    if (!chunks.length) return;
    ignoreScroll = true;
    term.write(chunks.join(""));
    requestAnimationFrame(() => {
      if (props.follow) term?.scrollToBottom();
      ignoreScroll = false;
    });
  },
  fit() {
    doFit();
  },
});
</script>

<template>
  <div class="xterm-host log-xterm" ref="host" />
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
