<script setup lang="ts">
import { nextTick, onMounted, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import FileTreeNode from "./FileTreeNode.vue";
import type { TreeNode } from "./fileTreeTypes";

interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
}

const props = defineProps<{
  projectId: string;
  rootPath: string;
}>();

const emit = defineEmits<{
  open: [path: string];
  pathFocus: [path: string];
}>();

const rootNodes = ref<TreeNode[]>([]);
const selectedPath = ref("");
const loadingRoot = ref(false);
const error = ref("");
const filter = ref("");
const expandedMap = reactive<Record<string, boolean>>({});
const treeScroll = ref<HTMLElement | null>(null);

function norm(p: string): string {
  return p.replace(/\\/g, "/");
}

function normKey(p: string): string {
  return norm(p).toLowerCase();
}

async function loadChildren(dirPath: string): Promise<TreeNode[]> {
  const entries = await invoke<DirEntry[]>("list_dir", {
    projectId: props.projectId,
    path: dirPath,
    basePath: null,
  });
  return entries.map((e) => ({
    ...e,
    expanded: !!expandedMap[e.path],
    loaded: false,
    children: e.isDir ? [] : undefined,
  }));
}

async function refreshRoot() {
  if (!props.projectId || !props.rootPath) return;
  loadingRoot.value = true;
  error.value = "";
  try {
    rootNodes.value = await loadChildren(props.rootPath);
  } catch (e) {
    error.value = String(e);
    rootNodes.value = [];
  } finally {
    loadingRoot.value = false;
  }
}

function findNode(nodes: TreeNode[], path: string): TreeNode | null {
  const key = normKey(path);
  for (const n of nodes) {
    if (normKey(n.path) === key) return n;
    if (n.children?.length) {
      const hit = findNode(n.children, path);
      if (hit) return hit;
    }
  }
  return null;
}

async function ensureExpanded(node: TreeNode) {
  if (!node.isDir) return;
  node.expanded = true;
  expandedMap[node.path] = true;
  if (!node.loaded) {
    node.loading = true;
    try {
      node.children = await loadChildren(node.path);
      node.loaded = true;
    } catch (e) {
      error.value = String(e);
      node.expanded = false;
      expandedMap[node.path] = false;
    } finally {
      node.loading = false;
    }
  }
}

async function toggleDir(node: TreeNode) {
  if (!node.isDir) return;
  if (node.expanded) {
    node.expanded = false;
    expandedMap[node.path] = false;
    return;
  }
  await ensureExpanded(node);
}

function selectNode(node: TreeNode) {
  emit("pathFocus", node.path);
  if (node.isDir) {
    void toggleDir(node);
    return;
  }
  selectedPath.value = node.path;
  emit("open", node.path);
}

function matchesFilter(node: TreeNode): boolean {
  const q = filter.value.trim().toLowerCase();
  if (!q) return true;
  if (node.name.toLowerCase().includes(q)) return true;
  if (node.children?.some((c) => matchesFilter(c))) return true;
  return false;
}

function relativeSegments(filePath: string): string[] | null {
  const root = norm(props.rootPath).replace(/\/+$/, "");
  const file = norm(filePath);
  const rk = root.toLowerCase();
  const fk = file.toLowerCase();
  if (fk === rk) return [];
  if (!fk.startsWith(rk + "/")) return null;
  const rel = file.slice(root.length).replace(/^\/+/, "");
  return rel ? rel.split("/") : [];
}

async function scrollSelectedIntoView() {
  await nextTick();
  const el = treeScroll.value?.querySelector(
    `.ftp-row.selected`,
  ) as HTMLElement | null;
  el?.scrollIntoView({ block: "nearest", inline: "nearest" });
}

/** Expand ancestors and select the file (IDEA-style reveal in project view). */
async function revealPath(filePath: string) {
  if (!filePath || !props.rootPath || !props.projectId) return;
  if (!rootNodes.value.length && !loadingRoot.value) {
    await refreshRoot();
  }
  const segments = relativeSegments(filePath);
  if (!segments || !segments.length) {
    selectedPath.value = filePath;
    await scrollSelectedIntoView();
    return;
  }

  let parentPath = norm(props.rootPath).replace(/\/+$/, "");
  let nodes = rootNodes.value;

  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i];
    const isLast = i === segments.length - 1;
    // Prefer matching by name under current nodes (paths may differ in slash style)
    let node = nodes.find((n) => n.name === seg) ?? null;
    if (!node) {
      const guess = `${parentPath}/${seg}`;
      node = findNode(rootNodes.value, guess);
    }
    if (!node) {
      // Reload parent once in case tree is stale
      if (i === 0) {
        await refreshRoot();
        nodes = rootNodes.value;
        node = nodes.find((n) => n.name === seg) ?? null;
      }
    }
    if (!node) break;

    if (!isLast && node.isDir) {
      await ensureExpanded(node);
      parentPath = norm(node.path);
      nodes = node.children || [];
    } else {
      selectedPath.value = node.path;
      emit("pathFocus", node.path);
      await scrollSelectedIntoView();
      return;
    }
  }

  // Fallback: select by absolute path even if not fully expanded
  selectedPath.value = filePath;
  emit("pathFocus", filePath);
  await scrollSelectedIntoView();
}

watch(
  () => [props.projectId, props.rootPath] as const,
  () => {
    selectedPath.value = "";
    for (const k of Object.keys(expandedMap)) delete expandedMap[k];
    void refreshRoot();
  },
);

onMounted(() => {
  void refreshRoot();
});

defineExpose({
  revealPath,
  refreshRoot,
});
</script>

<template>
  <div class="pft">
    <div class="pft-toolbar">
      <input
        v-model="filter"
        type="search"
        class="pft-filter"
        placeholder="过滤已展开节点"
        aria-label="过滤文件树"
      />
      <button type="button" class="small" :disabled="loadingRoot" title="刷新" @click="refreshRoot">
        ↻
      </button>
    </div>
    <p v-if="error" class="pft-err">{{ error }}</p>
    <div ref="treeScroll" class="pft-tree">
      <div v-if="loadingRoot" class="pft-hint muted">加载中…</div>
      <div v-else-if="!rootNodes.length" class="pft-hint muted">空目录</div>
      <FileTreeNode
        v-else
        :nodes="rootNodes"
        :depth="0"
        :selected-path="selectedPath"
        :filter-fn="matchesFilter"
        @select="selectNode"
        @dbl="selectNode"
      />
    </div>
  </div>
</template>

<style scoped>
.pft {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
}
.pft-toolbar {
  display: flex;
  gap: 0.35rem;
  padding: 0.35rem 0.5rem;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.pft-filter {
  flex: 1;
  min-width: 0;
  padding: 0.2rem 0.4rem;
  font-size: 12px;
}
.pft-err {
  margin: 0;
  padding: 0.3rem 0.5rem;
  color: var(--bad);
  font-size: 11px;
}
.pft-tree {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 0.15rem 0;
}
.pft-hint {
  padding: 1rem 0.75rem;
  text-align: center;
  font-size: 12px;
}
</style>
