<script setup lang="ts">
import { computed, ref } from "vue";
import type { GitBranchInfo } from "../types";

const props = defineProps<{
  branches: GitBranchInfo[];
  selected: string | null;
}>();

const emit = defineEmits<{
  select: [name: string];
  checkout: [name: string];
  merge: [name: string];
  create: [name: string, checkout: boolean];
}>();

const filter = ref("");
const newName = ref("");
const showCreate = ref(false);

const filtered = computed(() => {
  const q = filter.value.trim().toLowerCase();
  const list = props.branches;
  if (!q) return list;
  return list.filter((b) => b.name.toLowerCase().includes(q));
});

const locals = computed(() => filtered.value.filter((b) => !b.isRemote));
const remotes = computed(() => filtered.value.filter((b) => b.isRemote));

function trackHint(b: GitBranchInfo) {
  const parts: string[] = [];
  if (b.ahead) parts.push(`↑${b.ahead}`);
  if (b.behind) parts.push(`↓${b.behind}`);
  return parts.join(" ");
}

function submitCreate() {
  const n = newName.value.trim();
  if (!n) return;
  emit("create", n, true);
  newName.value = "";
  showCreate.value = false;
}
</script>

<template>
  <div class="git-branches">
    <div class="bar">
      <input v-model="filter" class="filter" placeholder="过滤分支…" />
      <button type="button" class="mini" @click="showCreate = !showCreate">新建</button>
    </div>
    <div v-if="showCreate" class="create-row">
      <input v-model="newName" placeholder="分支名" @keydown.enter="submitCreate" />
      <button type="button" class="mini primary" @click="submitCreate">创建并检出</button>
    </div>

    <div class="git-group-head">本地 ({{ locals.length }})</div>
    <button
      v-for="b in locals"
      :key="b.name"
      type="button"
      class="git-row"
      :class="{ active: selected === b.name, current: b.isCurrent }"
      @click="emit('select', b.name)"
      @dblclick="!b.isCurrent && emit('checkout', b.name)"
    >
      <span class="name">{{ b.isCurrent ? "* " : "" }}{{ b.name }}</span>
      <span class="track muted">{{ trackHint(b) }}</span>
      <span class="acts">
        <button
          v-if="!b.isCurrent"
          type="button"
          class="mini"
          @click.stop="emit('checkout', b.name)"
        >
          检出
        </button>
        <button
          v-if="!b.isCurrent"
          type="button"
          class="mini primary"
          title="合并到当前分支"
          @click.stop="emit('merge', b.name)"
        >
          合并
        </button>
      </span>
    </button>

    <div class="git-group-head">远程 ({{ remotes.length }})</div>
    <button
      v-for="b in remotes"
      :key="b.name"
      type="button"
      class="git-row"
      :class="{ active: selected === b.name }"
      @click="emit('select', b.name)"
    >
      <span class="name">{{ b.name }}</span>
      <span class="acts">
        <button type="button" class="mini primary" @click.stop="emit('merge', b.name)">合并</button>
      </span>
    </button>
  </div>
</template>

<style scoped>
.git-branches {
  overflow: auto;
  height: 100%;
  font-size: 12px;
}
.bar {
  display: flex;
  gap: 0.35rem;
  padding: 0.35rem;
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 0;
  background: var(--panel);
  z-index: 1;
}
.filter {
  flex: 1;
  min-width: 0;
  padding: 0.22rem 0.4rem;
  font-size: 12px;
}
.create-row {
  display: flex;
  gap: 0.35rem;
  padding: 0.35rem;
  border-bottom: 1px solid var(--border);
}
.create-row input {
  flex: 1;
  padding: 0.22rem 0.4rem;
  font-size: 12px;
}
.git-group-head {
  padding: 0.35rem 0.5rem;
  font-size: 0.7em;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  color: var(--muted);
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.git-row {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  width: 100%;
  text-align: left;
  padding: 0.28rem 0.45rem;
  border: none;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  background: transparent;
  color: var(--text);
  border-radius: 0;
  cursor: pointer;
}
.git-row:hover {
  background: color-mix(in srgb, var(--primary) 10%, transparent);
}
.git-row.active {
  background: color-mix(in srgb, var(--primary) 18%, transparent);
  box-shadow: inset 2px 0 0 var(--primary);
}
.git-row.current .name {
  font-weight: 700;
  color: var(--primary);
}
.name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.track {
  flex-shrink: 0;
  font-size: 11px;
}
.acts {
  display: none;
  gap: 0.2rem;
}
.git-row:hover .acts {
  display: inline-flex;
}
.mini {
  font-size: 10px;
  padding: 0.1rem 0.35rem;
  border-radius: 4px;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
  cursor: pointer;
}
.mini.primary {
  border-color: color-mix(in srgb, var(--primary) 50%, var(--border));
  color: var(--primary);
}
</style>
