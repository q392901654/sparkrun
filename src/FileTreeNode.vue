<script setup lang="ts">
import { fileIconMeta, type TreeNode } from "./fileTreeTypes";

defineProps<{
  nodes: TreeNode[];
  depth: number;
  selectedPath: string;
  filterFn: (n: TreeNode) => boolean;
}>();

defineEmits<{
  select: [node: TreeNode];
  dbl: [node: TreeNode];
}>();
</script>

<template>
  <template v-for="node in nodes" :key="node.path">
    <div v-if="filterFn(node)" class="ftp-row-wrap">
      <button
        type="button"
        class="ftp-row"
        :class="{ selected: selectedPath === node.path, dir: node.isDir }"
        :data-path="node.path"
        :style="{ paddingLeft: `${10 + depth * 14}px` }"
        @click="$emit('select', node)"
        @dblclick.prevent="$emit('dbl', node)"
      >
        <span class="ftp-twist" aria-hidden="true">{{
          node.isDir ? (node.expanded ? "▾" : "▸") : "·"
        }}</span>
        <span
          v-if="!node.isDir"
          class="ftp-kind"
          :data-kind="fileIconMeta(node.name, node.isDir).kind"
          :title="fileIconMeta(node.name, node.isDir).label"
          >{{ fileIconMeta(node.name, node.isDir).label }}</span
        >
        <span v-else class="ftp-folder" aria-hidden="true" title="文件夹"></span>
        <span class="ftp-name">{{ node.name }}</span>
        <span v-if="node.loading" class="ftp-loading muted">…</span>
      </button>
      <FileTreeNode
        v-if="node.isDir && node.expanded && node.children?.length"
        :nodes="node.children"
        :depth="depth + 1"
        :selected-path="selectedPath"
        :filter-fn="filterFn"
        @select="$emit('select', $event)"
        @dbl="$emit('dbl', $event)"
      />
    </div>
  </template>
</template>

<style scoped>
.ftp-row {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  width: 100%;
  border: none;
  border-radius: 0;
  border-left: 2px solid transparent;
  background: transparent;
  color: var(--text);
  text-align: left;
  padding: 0.28rem 0.5rem 0.28rem 0.4rem;
  min-height: 24px;
  font-size: 12.5px;
  cursor: pointer;
}
.ftp-row:hover {
  background: color-mix(in srgb, var(--primary) 8%, transparent);
  border-color: transparent;
  border-left-color: transparent;
}
.ftp-row.selected {
  background: color-mix(in srgb, var(--primary) 12%, transparent);
  border-left-color: var(--primary);
}
.ftp-twist {
  width: 12px;
  flex-shrink: 0;
  color: var(--muted);
  font-size: 10px;
  text-align: center;
}
.ftp-folder {
  flex-shrink: 0;
  width: 12px;
  height: 10px;
  margin: 0 2px 0 0;
  border: 1px solid color-mix(in srgb, var(--muted) 55%, transparent);
  border-radius: 1px 2px 2px 2px;
  background: color-mix(in srgb, var(--muted) 12%, transparent);
  position: relative;
  opacity: 0.7;
}
.ftp-folder::before {
  content: "";
  position: absolute;
  left: 0;
  top: -3px;
  width: 5px;
  height: 3px;
  border-radius: 1px 1px 0 0;
  background: color-mix(in srgb, var(--muted) 45%, transparent);
}
.ftp-kind {
  flex-shrink: 0;
  min-width: 2.2em;
  text-align: center;
  font-size: 8.5px;
  font-weight: 600;
  letter-spacing: 0.02em;
  padding: 0.06rem 0.22rem;
  border-radius: 3px;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  background: color-mix(in srgb, var(--muted) 12%, transparent);
  color: var(--muted);
  opacity: 0.85;
}
.ftp-kind[data-kind="dir"] {
  display: none;
}
.ftp-kind[data-kind="java"],
.ftp-kind[data-kind="js"],
.ftp-kind[data-kind="ts"],
.ftp-kind[data-kind="vue"],
.ftp-kind[data-kind="wpy"],
.ftp-kind[data-kind="json"],
.ftp-kind[data-kind="xml"],
.ftp-kind[data-kind="yaml"],
.ftp-kind[data-kind="md"],
.ftp-kind[data-kind="html"],
.ftp-kind[data-kind="css"],
.ftp-kind[data-kind="img"],
.ftp-kind[data-kind="cfg"],
.ftp-kind[data-kind="sql"],
.ftp-kind[data-kind="sh"],
.ftp-kind[data-kind="lock"],
.ftp-kind[data-kind="txt"],
.ftp-kind[data-kind="file"] {
  background: color-mix(in srgb, var(--muted) 10%, transparent);
  color: var(--muted);
}
.ftp-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ftp-loading {
  font-size: 11px;
}
</style>
