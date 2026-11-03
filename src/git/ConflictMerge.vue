<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { GitConflictSides } from "../types";

const props = defineProps<{
  rootPath: string;
  path: string;
}>();

const emit = defineEmits<{
  resolved: [];
  openFile: [path: string];
  back: [];
  error: [msg: string];
}>();

const loading = ref(false);
const saving = ref(false);
const sides = ref<GitConflictSides | null>(null);
const result = ref("");

const placeholder = "(无此侧内容)";

async function load() {
  loading.value = true;
  try {
    const s = await invoke<GitConflictSides>("git_conflict_sides", {
      rootPath: props.rootPath,
      path: props.path,
    });
    sides.value = s;
    result.value = s.worktree ?? s.ours ?? s.theirs ?? s.base ?? "";
  } catch (e) {
    emit("error", String(e));
  } finally {
    loading.value = false;
  }
}

watch(
  () => [props.rootPath, props.path],
  () => {
    void load();
  },
  { immediate: true },
);

function accept(side: "base" | "ours" | "theirs") {
  const s = sides.value;
  if (!s) return;
  const v = side === "base" ? s.base : side === "ours" ? s.ours : s.theirs;
  result.value = v ?? "";
}

function stripMarkersKeep(side: "ours" | "theirs") {
  const text = result.value;
  if (!text.includes("<<<<<<<")) {
    accept(side);
    return;
  }
  // Naive strip: keep ours or theirs sections between markers
  const re =
    /<<<<<<<[^\n]*\n([\s\S]*?)=======\n([\s\S]*?)>>>>>>>[^\n]*\n?/g;
  result.value = text.replace(re, (_m, ours, theirs) =>
    side === "ours" ? ours : theirs,
  );
}

const canSave = computed(() => !!props.path && !saving.value);

async function saveAndStage() {
  if (!canSave.value) return;
  saving.value = true;
  try {
    await invoke("git_write_file", {
      rootPath: props.rootPath,
      path: props.path,
      content: result.value,
    });
    await invoke("git_stage", {
      rootPath: props.rootPath,
      paths: [props.path],
    });
    emit("resolved");
  } catch (e) {
    emit("error", String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="conflict-merge">
    <div class="cm-toolbar">
      <button type="button" class="mini" @click="emit('back')">← 返回</button>
      <span class="cm-path" :title="path">{{ path }}</span>
      <div class="cm-actions">
        <button type="button" class="mini" :disabled="!sides?.hasStage1" @click="accept('base')">
          Accept Base
        </button>
        <button type="button" class="mini" :disabled="!sides?.hasStage2" @click="accept('ours')">
          Accept Ours
        </button>
        <button type="button" class="mini" :disabled="!sides?.hasStage3" @click="accept('theirs')">
          Accept Theirs
        </button>
        <button type="button" class="mini" @click="stripMarkersKeep('ours')">清标记·留 Ours</button>
        <button type="button" class="mini" @click="emit('openFile', path)">工作区打开</button>
        <button
          type="button"
          class="mini primary"
          :disabled="!canSave"
          @click="saveAndStage"
        >
          {{ saving ? "保存中…" : "写回并 Stage" }}
        </button>
      </div>
    </div>

    <div v-if="loading" class="cm-loading muted">加载冲突内容…</div>
    <div v-else class="cm-grid">
      <div class="cm-col">
        <div class="cm-label">Base</div>
        <textarea class="cm-ta" readonly :value="sides?.base ?? placeholder" />
      </div>
      <div class="cm-col">
        <div class="cm-label">Ours</div>
        <textarea class="cm-ta" readonly :value="sides?.ours ?? placeholder" />
      </div>
      <div class="cm-col">
        <div class="cm-label">Theirs</div>
        <textarea class="cm-ta" readonly :value="sides?.theirs ?? placeholder" />
      </div>
      <div class="cm-col result">
        <div class="cm-label">Result（可编辑）</div>
        <textarea v-model="result" class="cm-ta edit" spellcheck="false" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.conflict-merge {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--panel);
}
.cm-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.35rem;
  padding: 0.4rem 0.5rem;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.cm-path {
  font-weight: 600;
  font-size: 12px;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cm-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.25rem;
  margin-left: auto;
}
.cm-loading {
  padding: 2rem;
  text-align: center;
}
.cm-grid {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: 1fr 1fr;
  grid-template-rows: 1fr 1fr;
  gap: 1px;
  background: var(--border);
}
.cm-col {
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--panel);
}
.cm-col.result {
  grid-column: 1 / -1;
}
.cm-label {
  flex-shrink: 0;
  padding: 0.25rem 0.45rem;
  font-size: 10px;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--muted);
  border-bottom: 1px solid var(--border);
  background: var(--panel-elevated);
}
.cm-ta {
  flex: 1;
  min-height: 0;
  width: 100%;
  border: none;
  resize: none;
  padding: 0.4rem 0.5rem;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 12px;
  line-height: 1.4;
  background: transparent;
  color: var(--text);
  tab-size: 2;
}
.cm-ta.edit {
  background: color-mix(in srgb, var(--primary) 4%, var(--panel));
}
.mini {
  font-size: 10px;
  padding: 0.15rem 0.4rem;
  border-radius: 4px;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
  cursor: pointer;
}
.mini:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.mini.primary {
  color: var(--primary);
  border-color: color-mix(in srgb, var(--primary) 50%, var(--border));
  font-weight: 600;
}

@media (min-width: 1100px) {
  .cm-grid {
    grid-template-columns: 1fr 1fr 1fr;
    grid-template-rows: 1fr 1.2fr;
  }
  .cm-col.result {
    grid-column: 1 / -1;
  }
}
</style>
