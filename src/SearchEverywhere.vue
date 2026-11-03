<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

export type SearchTab = "all" | "file" | "text" | "endpoint" | "action";

export interface SearchHitDto {
  kind: string;
  path: string;
  name: string;
  score: number;
  line?: number | null;
  preview?: string | null;
  httpMethod?: string | null;
  handlerMethod?: string | null;
  moduleHint?: string | null;
}

export interface SearchAction {
  id: string;
  label: string;
  hint?: string;
}

const props = defineProps<{
  open: boolean;
  projectId: string;
  unitPath?: string | null;
  actions: SearchAction[];
}>();

const emit = defineEmits<{
  close: [];
  openFile: [payload: { path: string; line?: number }];
  runAction: [id: string];
}>();

const query = ref("");
const tab = ref<SearchTab>("all");
const onlyUnit = ref(false);
const loading = ref(false);
const error = ref("");
const remoteHits = ref<SearchHitDto[]>([]);
const selected = ref(0);
const inputRef = ref<HTMLInputElement | null>(null);

let debounceTimer: ReturnType<typeof setTimeout> | undefined;
/** Ignore stale invoke results when the user keeps typing. */
let searchGen = 0;

const TABS: { id: SearchTab; label: string }[] = [
  { id: "all", label: "所有" },
  { id: "file", label: "文件" },
  { id: "text", label: "文本" },
  { id: "endpoint", label: "接口" },
  { id: "action", label: "操作" },
];

const actionHits = computed(() => {
  const q = query.value.trim().toLowerCase();
  return props.actions
    .filter((a) => !q || a.label.toLowerCase().includes(q) || a.id.toLowerCase().includes(q))
    .map(
      (a): SearchHitDto => ({
        kind: "action",
        path: a.id,
        name: a.label,
        score: 900,
        preview: a.hint || null,
      }),
    );
});

const displayHits = computed(() => {
  const t = tab.value;
  let list = remoteHits.value;
  if (t === "file") list = list.filter((h) => h.kind === "file");
  else if (t === "text") list = list.filter((h) => h.kind === "text");
  else if (t === "endpoint") list = list.filter((h) => h.kind === "endpoint");
  else if (t === "action") return actionHits.value;
  else {
    // all: merge actions when query matches
    list = [...list, ...actionHits.value];
  }
  return list;
});

watch(displayHits, () => {
  selected.value = 0;
});

function highlight(text: string, q: string): string {
  if (!q.trim()) return escapeHtml(text);
  const lower = text.toLowerCase();
  const n = q.toLowerCase();
  const i = lower.indexOf(n);
  if (i < 0) return escapeHtml(text);
  return (
    escapeHtml(text.slice(0, i)) +
    `<mark>${escapeHtml(text.slice(i, i + n.length))}</mark>` +
    escapeHtml(text.slice(i + n.length))
  );
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function kindIcon(kind: string): string {
  if (kind === "endpoint") return "🌐";
  if (kind === "text") return "T";
  if (kind === "action") return "⚡";
  return "📄";
}

function rightMeta(h: SearchHitDto): string {
  if (h.kind === "action") return h.preview || "";
  const bits: string[] = [];
  if (h.moduleHint) bits.push(h.moduleHint);
  if (h.kind === "endpoint" && h.httpMethod) bits.push(h.httpMethod);
  if (h.line) bits.push(`:${h.line}`);
  else if (h.name && h.kind === "file") bits.push(h.name);
  return bits.join(" ");
}

function primaryHtml(h: SearchHitDto): string {
  const q = query.value.trim();
  if (h.kind === "endpoint") {
    return highlight(h.name, q);
  }
  if (h.kind === "text") {
    return highlight(h.preview || h.name, q);
  }
  if (h.kind === "action") {
    return highlight(h.name, q);
  }
  return highlight(h.path.replace(/\\/g, "/"), q);
}

async function runSearch() {
  const q = query.value.trim();
  const gen = ++searchGen;
  if (!q || !props.projectId) {
    remoteHits.value = [];
    loading.value = false;
    return;
  }
  if (tab.value === "action") {
    remoteHits.value = [];
    loading.value = false;
    return;
  }
  loading.value = true;
  error.value = "";
  try {
    // Prefer cheaper scopes first when on「所有」: still all kinds, but backend early-exits
    const kinds =
      tab.value === "all"
        ? ["file", "text", "endpoint"]
        : tab.value === "file"
          ? ["file"]
          : tab.value === "text"
            ? ["text"]
            : tab.value === "endpoint"
              ? ["endpoint"]
              : ["file", "text", "endpoint"];
    const hits = await invoke<SearchHitDto[]>("search_everywhere", {
      projectId: props.projectId,
      query: q,
      kinds,
      basePath: onlyUnit.value && props.unitPath ? props.unitPath : null,
      limit: 40,
    });
    if (gen !== searchGen) return;
    remoteHits.value = hits;
  } catch (e) {
    if (gen !== searchGen) return;
    error.value = String(e);
    remoteHits.value = [];
  } finally {
    if (gen === searchGen) loading.value = false;
  }
}

function scheduleSearch() {
  if (debounceTimer) clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => void runSearch(), 280);
}

function activate(h: SearchHitDto) {
  if (h.kind === "action") {
    emit("runAction", h.path);
    emit("close");
    return;
  }
  emit("openFile", {
    path: h.path,
    line: h.line ?? undefined,
  });
  emit("close");
}

function onKey(ev: KeyboardEvent) {
  if (!props.open) return;
  const list = displayHits.value;
  if (ev.key === "Escape") {
    ev.preventDefault();
    emit("close");
    return;
  }
  if (ev.key === "ArrowDown") {
    ev.preventDefault();
    if (!list.length) return;
    selected.value = (selected.value + 1) % list.length;
    return;
  }
  if (ev.key === "ArrowUp") {
    ev.preventDefault();
    if (!list.length) return;
    selected.value = (selected.value - 1 + list.length) % list.length;
    return;
  }
  if (ev.key === "Enter") {
    ev.preventDefault();
    const h = list[selected.value];
    if (h) activate(h);
  }
}

watch(
  () => props.open,
  async (v) => {
    if (v) {
      query.value = "";
      remoteHits.value = [];
      error.value = "";
      selected.value = 0;
      await nextTick();
      inputRef.value?.focus();
      inputRef.value?.select();
    }
  },
);

watch([query, tab, onlyUnit], () => {
  if (!props.open) return;
  scheduleSearch();
});

onMounted(() => {
  window.addEventListener("keydown", onKey, true);
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKey, true);
  if (debounceTimer) clearTimeout(debounceTimer);
});
</script>

<template>
  <div v-if="open" class="se-backdrop" @click.self="emit('close')">
    <div class="se-modal" role="dialog" aria-label="搜索所有地方">
      <div class="se-tabs">
        <button
          v-for="t in TABS"
          :key="t.id"
          type="button"
          class="se-tab"
          :class="{ active: tab === t.id }"
          @click="tab = t.id"
        >
          {{ t.label }}
        </button>
        <label class="se-only">
          <input v-model="onlyUnit" type="checkbox" :disabled="!unitPath" />
          仅当前单元
        </label>
      </div>
      <div class="se-input-wrap">
        <span class="se-search-ico" aria-hidden="true">⌕</span>
        <input
          ref="inputRef"
          v-model="query"
          type="search"
          class="se-input"
          placeholder="搜索文件、文本、接口或操作…"
          autocomplete="off"
          spellcheck="false"
        />
        <span v-if="loading" class="se-loading muted">搜索中…</span>
      </div>
      <p v-if="error" class="se-err">{{ error }}</p>
      <div class="se-list" role="listbox">
        <div v-if="!query.trim()" class="se-empty muted">输入关键字开始搜索</div>
        <div v-else-if="!displayHits.length && !loading" class="se-empty muted">无结果</div>
        <button
          v-for="(h, i) in displayHits"
          :key="`${h.kind}:${h.path}:${h.line || 0}:${i}`"
          type="button"
          class="se-row"
          :class="{ active: i === selected }"
          role="option"
          :aria-selected="i === selected"
          @click="activate(h)"
          @mouseenter="selected = i"
        >
          <span class="se-ico" :data-kind="h.kind">{{ kindIcon(h.kind) }}</span>
          <span class="se-main">
            <span class="se-primary" v-html="primaryHtml(h)" />
            <span v-if="h.kind === 'endpoint' && h.handlerMethod" class="se-sub muted">
              ({{ h.handlerMethod }})
            </span>
          </span>
          <span class="se-meta muted">{{ rightMeta(h) }}</span>
        </button>
      </div>
      <div class="se-foot muted">
        ↑↓ 选择 · Enter 打开 · Esc 关闭 · Ctrl+Shift+F / 连按 Shift
      </div>
    </div>
  </div>
</template>

<style scoped>
.se-backdrop {
  position: fixed;
  inset: 0;
  z-index: 120;
  background: rgba(15, 23, 42, 0.5);
  display: grid;
  place-items: start center;
  padding: 12vh 1rem 1rem;
}
.se-modal {
  width: min(720px, 96vw);
  max-height: min(70vh, 560px);
  display: flex;
  flex-direction: column;
  background: var(--panel-elevated);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}
.se-tabs {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.25rem;
  padding: 0.45rem 0.65rem;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.se-tab {
  border: none;
  background: transparent;
  color: var(--muted);
  padding: 0.25rem 0.55rem;
  border-radius: 6px;
  font-size: 12.5px;
  cursor: pointer;
}
.se-tab.active {
  background: color-mix(in srgb, var(--primary) 22%, transparent);
  color: var(--text);
  font-weight: 600;
}
.se-only {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 0.3rem;
  font-size: 11.5px;
  color: var(--muted);
  user-select: none;
}
.se-input-wrap {
  display: flex;
  align-items: center;
  gap: 0.45rem;
  margin: 0.55rem 0.75rem;
  padding: 0.35rem 0.55rem;
  border: 1px solid color-mix(in srgb, var(--primary) 55%, var(--border));
  border-radius: 8px;
  background: var(--panel);
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--primary) 25%, transparent);
}
.se-search-ico {
  color: var(--muted);
  font-size: 14px;
}
.se-input {
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  outline: none;
  padding: 0.15rem 0;
}
.se-loading {
  font-size: 11px;
  flex-shrink: 0;
}
.se-err {
  margin: 0 0.75rem;
  color: var(--bad);
  font-size: 12px;
}
.se-list {
  flex: 1;
  min-height: 160px;
  overflow: auto;
  padding: 0.25rem 0;
}
.se-empty {
  padding: 2rem 1rem;
  text-align: center;
  font-size: 13px;
}
.se-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  width: 100%;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--text);
  text-align: left;
  padding: 0.4rem 0.75rem;
  font-size: 12.5px;
  cursor: pointer;
}
.se-row:hover,
.se-row.active {
  background: color-mix(in srgb, var(--primary) 18%, transparent);
}
.se-ico {
  width: 1.4rem;
  flex-shrink: 0;
  text-align: center;
  font-size: 11px;
  font-weight: 700;
  font-family: "IBM Plex Mono", Consolas, monospace;
}
.se-ico[data-kind="text"] {
  color: var(--muted);
  border: 1px solid var(--border);
  border-radius: 3px;
  line-height: 1.2;
}
.se-main {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.se-primary :deep(mark) {
  background: color-mix(in srgb, #eab308 55%, transparent);
  color: inherit;
  padding: 0 1px;
  border-radius: 2px;
}
.se-sub {
  margin-left: 0.35rem;
  font-size: 11px;
}
.se-meta {
  flex-shrink: 0;
  font-size: 11px;
  font-family: "IBM Plex Mono", Consolas, monospace;
  max-width: 36%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.se-foot {
  padding: 0.4rem 0.75rem;
  border-top: 1px solid var(--border);
  font-size: 11px;
  background: var(--panel);
}
</style>
