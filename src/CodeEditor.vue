<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as monaco from "monaco-editor";
import "monaco-editor-css";
import { ensureMonacoEnv } from "./monacoEnv";

const props = defineProps<{
  path: string;
  content: string;
  dark: boolean;
  /** When false, editor is hidden / not focused */
  active?: boolean;
  /** 1-based line to reveal/select after open or when changed */
  revealLine?: number | null;
}>();

const emit = defineEmits<{
  change: [value: string];
  save: [];
}>();

const host = ref<HTMLDivElement | null>(null);
let editor: monaco.editor.IStandaloneCodeEditor | null = null;
/** Suppress change emit while programmatically setting doc */
let applying = false;
let contentDisposable: monaco.IDisposable | null = null;

function languageId(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith(".java")) return "java";
  if (lower.endsWith(".json")) return "json";
  if (lower.endsWith(".xml") || lower.endsWith(".pom") || lower.endsWith(".svg")) return "xml";
  if (lower.endsWith(".yml") || lower.endsWith(".yaml")) return "yaml";
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (lower.endsWith(".scss")) return "scss";
  if (lower.endsWith(".less")) return "less";
  if (lower.endsWith(".css")) return "css";
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  // Vue/WePY SFC: HTML monarch covers templates better than plain JS without Volar
  if (lower.endsWith(".vue") || lower.endsWith(".wpy")) return "html";
  if (lower.endsWith(".ts") || lower.endsWith(".tsx")) return "typescript";
  if (
    lower.endsWith(".js") ||
    lower.endsWith(".jsx") ||
    lower.endsWith(".mjs") ||
    lower.endsWith(".cjs")
  ) {
    return "javascript";
  }
  if (lower.endsWith(".properties") || lower.endsWith(".ini")) return "ini";
  if (lower.endsWith(".rs")) return "rust";
  if (lower.endsWith(".sql")) return "sql";
  if (lower.endsWith(".sh") || lower.endsWith(".bash") || lower.endsWith(".ps1")) return "shell";
  return "plaintext";
}

function themeName(dark: boolean) {
  return dark ? "vs-dark" : "vs";
}

onMounted(() => {
  if (!host.value) return;
  ensureMonacoEnv();
  editor = monaco.editor.create(host.value, {
    value: props.content ?? "",
    language: languageId(props.path),
    theme: themeName(props.dark),
    automaticLayout: true,
    fontSize: 13,
    fontFamily: '"Cascadia Mono", Consolas, "Courier New", monospace',
    minimap: { enabled: true, scale: 1 },
    scrollBeyondLastLine: false,
    renderLineHighlight: "line",
    tabSize: 2,
    insertSpaces: true,
    wordWrap: "off",
    padding: { top: 4, bottom: 24 },
    smoothScrolling: true,
    cursorBlinking: "smooth",
    bracketPairColorization: { enabled: true },
    fixedOverflowWidgets: true,
  });

  contentDisposable = editor.onDidChangeModelContent(() => {
    if (applying || !editor) return;
    emit("change", editor.getValue());
  });

  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
    emit("save");
  });

  if (props.active !== false) {
    requestAnimationFrame(() => editor?.focus());
  }
  applyRevealLine(props.revealLine);
});

function applyRevealLine(line?: number | null) {
  if (!editor || line == null || line < 1) return;
  const model = editor.getModel();
  if (!model) return;
  const max = model.getLineCount();
  const ln = Math.min(line, max);
  editor.revealLineInCenter(ln);
  editor.setSelection({
    startLineNumber: ln,
    startColumn: 1,
    endLineNumber: ln,
    endColumn: model.getLineMaxColumn(ln),
  });
}

watch(
  () => props.revealLine,
  (line) => applyRevealLine(line),
);

watch(
  () => props.content,
  (next) => {
    if (!editor) return;
    const cur = editor.getValue();
    if (cur === next) return;
    applying = true;
    editor.setValue(next ?? "");
    applying = false;
  },
);

watch(
  () => props.path,
  (path) => {
    if (!editor) return;
    const model = editor.getModel();
    if (model) {
      monaco.editor.setModelLanguage(model, languageId(path));
    }
  },
);

watch(
  () => props.dark,
  (dark) => {
    monaco.editor.setTheme(themeName(dark));
  },
);

watch(
  () => props.active,
  (v) => {
    if (v && editor) {
      requestAnimationFrame(() => {
        editor?.layout();
        editor?.focus();
      });
    }
  },
);

onBeforeUnmount(() => {
  contentDisposable?.dispose();
  contentDisposable = null;
  editor?.dispose();
  editor = null;
});

defineExpose({
  focus() {
    editor?.focus();
  },
});
</script>

<template>
  <div class="monaco-host" ref="host" />
</template>

<style scoped>
.monaco-host {
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
}
</style>
