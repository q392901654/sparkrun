<script setup lang="ts">
import type { GitStatusEntry } from "../types";

const props = defineProps<{
  entries: GitStatusEntry[];
  selectedPath: string | null;
  selectedStaged: boolean;
}>();

const emit = defineEmits<{
  select: [entry: GitStatusEntry, preferStaged: boolean];
  stage: [paths: string[]];
  unstage: [paths: string[]];
  discard: [paths: string[]];
  openFile: [path: string];
  openConflict: [path: string];
}>();

const conflicts = () => props.entries.filter((e) => e.unmerged);
const staged = () => props.entries.filter((e) => e.staged && !e.unmerged);
const worktree = () =>
  props.entries.filter((e) => !e.unmerged && (e.unstaged || e.untracked));

function statusLabel(e: GitStatusEntry) {
  if (e.unmerged) return e.status;
  if (e.untracked) return "??";
  return e.status;
}

function isSelected(e: GitStatusEntry, preferStaged: boolean) {
  return props.selectedPath === e.path && props.selectedStaged === preferStaged;
}
</script>

<template>
  <div class="git-files">
    <div v-if="conflicts().length" class="git-group">
      <div class="git-group-head">冲突 ({{ conflicts().length }})</div>
      <button
        v-for="e in conflicts()"
        :key="'c-' + e.path"
        type="button"
        class="git-file conflict"
        :class="{ active: selectedPath === e.path }"
        @click="emit('openConflict', e.path)"
      >
        <span class="st">{{ statusLabel(e) }}</span>
        <span class="path" :title="e.path">{{ e.path }}</span>
        <span class="acts">
          <button type="button" class="mini" title="三方合并" @click.stop="emit('openConflict', e.path)">
            解决
          </button>
        </span>
      </button>
    </div>

    <div class="git-group">
      <div class="git-group-head">
        已暂存 ({{ staged().length }})
        <button
          v-if="staged().length"
          type="button"
          class="mini"
          @click="emit('unstage', staged().map((x) => x.path))"
        >
          全部取消
        </button>
      </div>
      <button
        v-for="e in staged()"
        :key="'s-' + e.path"
        type="button"
        class="git-file staged"
        :class="{ active: isSelected(e, true) }"
        @click="emit('select', e, true)"
        @dblclick="emit('openFile', e.path)"
      >
        <span class="st">{{ statusLabel(e) }}</span>
        <span class="path" :title="e.path">{{ e.path }}</span>
        <span class="acts">
          <button type="button" class="mini" @click.stop="emit('unstage', [e.path])">取消</button>
        </span>
      </button>
      <div v-if="!staged().length" class="git-empty muted">无已暂存更改</div>
    </div>

    <div class="git-group">
      <div class="git-group-head">
        工作区 ({{ worktree().length }})
        <button
          v-if="worktree().length"
          type="button"
          class="mini"
          @click="emit('stage', worktree().map((x) => x.path))"
        >
          全部暂存
        </button>
      </div>
      <button
        v-for="e in worktree()"
        :key="'w-' + e.path"
        type="button"
        class="git-file"
        :class="{ active: isSelected(e, false) }"
        @click="emit('select', e, false)"
        @dblclick="emit('openFile', e.path)"
      >
        <span class="st">{{ statusLabel(e) }}</span>
        <span class="path" :title="e.path">{{ e.path }}</span>
        <span class="acts">
          <button type="button" class="mini" @click.stop="emit('stage', [e.path])">暂存</button>
          <button
            type="button"
            class="mini danger"
            @click.stop="emit('discard', [e.path])"
          >
            丢弃
          </button>
        </span>
      </button>
      <div v-if="!worktree().length" class="git-empty muted">工作区干净</div>
    </div>
  </div>
</template>

<style scoped>
.git-files {
  overflow: auto;
  height: 100%;
  font-size: 12px;
}
.git-group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.35rem;
  padding: 0.35rem 0.5rem;
  font-size: 0.7em;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  color: var(--muted);
  background: var(--panel);
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 0;
  z-index: 1;
}
.git-file {
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
.git-file:hover {
  background: color-mix(in srgb, var(--primary) 10%, transparent);
}
.git-file.active {
  background: color-mix(in srgb, var(--primary) 18%, transparent);
  box-shadow: inset 2px 0 0 var(--primary);
}
.git-file.conflict .st {
  color: var(--bad, #ef4444);
  font-weight: 700;
}
.git-file.staged .st {
  color: #22c55e;
}
.st {
  flex: 0 0 1.6rem;
  font-family: Consolas, monospace;
  font-size: 11px;
  color: var(--muted);
}
.path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.acts {
  display: none;
  gap: 0.2rem;
  flex-shrink: 0;
}
.git-file:hover .acts {
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
.mini.danger:hover {
  color: var(--bad, #ef4444);
  border-color: var(--bad, #ef4444);
}
.git-empty {
  padding: 0.55rem 0.65rem;
  font-size: 0.85em;
}
</style>
