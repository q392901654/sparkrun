<script setup lang="ts">
defineProps<{
  title?: string;
  content: string;
  emptyHint?: string;
}>();
</script>

<template>
  <div class="git-diff">
    <div v-if="title" class="git-diff-title muted">{{ title }}</div>
    <pre v-if="content" class="git-diff-body">{{ content }}</pre>
    <div v-else class="git-diff-empty muted">{{ emptyHint || "无 diff" }}</div>
  </div>
</template>

<style scoped>
.git-diff {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--panel);
}
.git-diff-title {
  flex-shrink: 0;
  padding: 0.35rem 0.55rem;
  font-size: 11px;
  border-bottom: 1px solid var(--border);
}
.git-diff-body {
  margin: 0;
  padding: 0.5rem 0.65rem;
  overflow: auto;
  flex: 1;
  min-height: 0;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 12px;
  line-height: 1.45;
  white-space: pre;
  tab-size: 2;
}
.git-diff-empty {
  padding: 1.5rem;
  text-align: center;
  font-size: 0.9em;
}
</style>
