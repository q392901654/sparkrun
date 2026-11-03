<script setup lang="ts">
import type { GitCommitInfo } from "../types";

defineProps<{
  commits: GitCommitInfo[];
  selected: string | null;
}>();

const emit = defineEmits<{
  select: [hash: string];
}>();
</script>

<template>
  <div class="git-commits">
    <button
      v-for="c in commits"
      :key="c.hash"
      type="button"
      class="git-row"
      :class="{ active: selected === c.hash }"
      :title="c.hash"
      @click="emit('select', c.hash)"
    >
      <span class="hash">{{ c.shortHash }}</span>
      <span class="subj">{{ c.subject }}</span>
      <span class="meta muted">{{ c.author }} · {{ c.relativeTime }}</span>
    </button>
    <div v-if="!commits.length" class="empty muted">暂无提交</div>
  </div>
</template>

<style scoped>
.git-commits {
  overflow: auto;
  height: 100%;
  font-size: 12px;
}
.git-row {
  display: grid;
  grid-template-columns: 4.2rem 1fr;
  grid-template-rows: auto auto;
  column-gap: 0.4rem;
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
.hash {
  grid-row: 1 / 3;
  align-self: center;
  font-family: Consolas, monospace;
  color: var(--primary);
  font-size: 11px;
}
.subj {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 560;
}
.meta {
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.empty {
  padding: 1rem;
  text-align: center;
}
</style>
