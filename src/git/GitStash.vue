<script setup lang="ts">
import { ref } from "vue";
import type { GitStashEntry } from "../types";

defineProps<{
  entries: GitStashEntry[];
  selected: number | null;
}>();

const emit = defineEmits<{
  select: [index: number];
  push: [message: string];
  apply: [index: number];
  drop: [index: number];
}>();

const msg = ref("");

function doPush() {
  emit("push", msg.value.trim());
  msg.value = "";
}
</script>

<template>
  <div class="git-stash">
    <div class="bar">
      <input v-model="msg" placeholder="可选说明" @keydown.enter="doPush" />
      <button type="button" class="mini primary" @click="doPush">Stash</button>
    </div>
    <button
      v-for="e in entries"
      :key="e.index"
      type="button"
      class="git-row"
      :class="{ active: selected === e.index }"
      @click="emit('select', e.index)"
    >
      <span class="idx">stash@{{ "{" + e.index + "}" }}</span>
      <span class="msg">{{ e.message }}</span>
      <span class="acts">
        <button type="button" class="mini" @click.stop="emit('apply', e.index)">应用</button>
        <button type="button" class="mini danger" @click.stop="emit('drop', e.index)">删除</button>
      </span>
    </button>
    <div v-if="!entries.length" class="empty muted">暂无 stash</div>
  </div>
</template>

<style scoped>
.git-stash {
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
.bar input {
  flex: 1;
  padding: 0.22rem 0.4rem;
  font-size: 12px;
}
.git-row {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  width: 100%;
  text-align: left;
  padding: 0.35rem 0.5rem;
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
.idx {
  flex-shrink: 0;
  font-family: Consolas, monospace;
  font-size: 11px;
  color: var(--primary);
}
.msg {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
  color: var(--primary);
  border-color: color-mix(in srgb, var(--primary) 50%, var(--border));
}
.mini.danger:hover {
  color: var(--bad, #ef4444);
}
.empty {
  padding: 1rem;
  text-align: center;
}
</style>
