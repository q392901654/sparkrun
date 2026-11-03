<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type {
  GitBranchInfo,
  GitCommitInfo,
  GitOpResult,
  GitRepoSummary,
  GitStashEntry,
  GitStatusEntry,
} from "./types";
import GitFiles from "./git/GitFiles.vue";
import GitBranches from "./git/GitBranches.vue";
import GitCommits from "./git/GitCommits.vue";
import GitStash from "./git/GitStash.vue";
import GitDiffView from "./git/GitDiffView.vue";

const ConflictMerge = defineAsyncComponent(() => import("./git/ConflictMerge.vue"));

const props = defineProps<{
  rootPath: string;
  projectId: string;
  dark?: boolean;
}>();

const emit = defineEmits<{
  openFile: [path: string];
}>();

type Tab = "files" | "branches" | "commits" | "stash";

const tab = ref<Tab>("files");
const busy = ref(false);
const statusMsg = ref("");
const errorMsg = ref("");

const summary = ref<GitRepoSummary | null>(null);
const entries = ref<GitStatusEntry[]>([]);
const branches = ref<GitBranchInfo[]>([]);
const commits = ref<GitCommitInfo[]>([]);
const stashes = ref<GitStashEntry[]>([]);

const selectedPath = ref<string | null>(null);
const selectedStaged = ref(false);
const selectedBranch = ref<string | null>(null);
const selectedCommit = ref<string | null>(null);
const selectedStash = ref<number | null>(null);

const diffText = ref("");
const diffTitle = ref("");
const commitMessage = ref("");
const amend = ref(false);

const conflictPath = ref<string | null>(null);
const showConflict = computed(() => !!conflictPath.value);

async function withBusy<T>(fn: () => Promise<T>): Promise<T | undefined> {
  busy.value = true;
  errorMsg.value = "";
  try {
    return await fn();
  } catch (e) {
    errorMsg.value = String(e);
    return undefined;
  } finally {
    busy.value = false;
  }
}

function setStatus(msg: string) {
  statusMsg.value = msg;
  if (msg) setTimeout(() => {
    if (statusMsg.value === msg) statusMsg.value = "";
  }, 4000);
}

async function refreshAll() {
  if (!props.rootPath) return;
  await withBusy(async () => {
    const s = await invoke<GitRepoSummary>("git_repo_summary", { rootPath: props.rootPath });
    summary.value = s;
    if (!s.isRepo) {
      entries.value = [];
      branches.value = [];
      commits.value = [];
      stashes.value = [];
      return;
    }
    const [st, br, log, sh] = await Promise.all([
      invoke<GitStatusEntry[]>("git_status", { rootPath: props.rootPath }),
      invoke<GitBranchInfo[]>("git_branches", { rootPath: props.rootPath }),
      invoke<GitCommitInfo[]>("git_log", { rootPath: props.rootPath, limit: 80 }),
      invoke<GitStashEntry[]>("git_stash_list", { rootPath: props.rootPath }),
    ]);
    entries.value = st;
    branches.value = br;
    commits.value = log;
    stashes.value = sh;

    if (s.mergeInProgress && !commitMessage.value) {
      try {
        commitMessage.value = await invoke<string>("git_default_merge_message", {
          rootPath: props.rootPath,
        });
      } catch {
        /* ignore */
      }
    }
  });
  if (selectedPath.value) {
    await loadDiff(selectedPath.value, selectedStaged.value);
  } else if (selectedCommit.value) {
    await loadShow(selectedCommit.value);
  }
}

async function loadDiff(path: string, staged: boolean) {
  try {
    diffTitle.value = `${staged ? "Staged" : "Unstaged"} · ${path}`;
    diffText.value = await invoke<string>("git_diff", {
      rootPath: props.rootPath,
      path,
      staged,
    });
  } catch (e) {
    diffText.value = String(e);
  }
}

async function loadShow(hash: string) {
  try {
    diffTitle.value = `Commit ${hash.slice(0, 8)}`;
    diffText.value = await invoke<string>("git_show", {
      rootPath: props.rootPath,
      rev: hash,
    });
  } catch (e) {
    diffText.value = String(e);
  }
}

function onSelectFile(entry: GitStatusEntry, preferStaged: boolean) {
  conflictPath.value = null;
  selectedPath.value = entry.path;
  selectedStaged.value = preferStaged;
  selectedCommit.value = null;
  void loadDiff(entry.path, preferStaged);
}

function onOpenConflict(path: string) {
  conflictPath.value = path;
  selectedPath.value = path;
}

async function onStage(paths: string[]) {
  await withBusy(() =>
    invoke("git_stage", { rootPath: props.rootPath, paths }),
  );
  await refreshAll();
}

async function onUnstage(paths: string[]) {
  await withBusy(() =>
    invoke("git_unstage", { rootPath: props.rootPath, paths }),
  );
  await refreshAll();
}

async function onDiscard(paths: string[]) {
  if (!confirm(`丢弃工作区更改？\n${paths.join("\n")}`)) return;
  await withBusy(() =>
    invoke("git_discard", { rootPath: props.rootPath, paths }),
  );
  await refreshAll();
}

async function doCommit() {
  const merging = summary.value?.mergeInProgress;
  if (merging) {
    if (summary.value && summary.value.conflictCount > 0) {
      errorMsg.value = `仍有 ${summary.value.conflictCount} 个冲突未解决`;
      return;
    }
    await withBusy(() =>
      invoke("git_merge_continue", {
        rootPath: props.rootPath,
        message: commitMessage.value.trim() || null,
      }),
    );
    commitMessage.value = "";
    setStatus("合并完成");
    await refreshAll();
    return;
  }
  if (!commitMessage.value.trim() && !amend.value) {
    errorMsg.value = "请填写提交说明";
    return;
  }
  await withBusy(() =>
    invoke("git_commit", {
      rootPath: props.rootPath,
      message: commitMessage.value,
      amend: amend.value,
    }),
  );
  commitMessage.value = "";
  amend.value = false;
  setStatus("已提交");
  await refreshAll();
}

async function handleOp(result: GitOpResult | undefined, okMsg: string) {
  if (!result) return;
  if (result.conflicts) {
    setStatus(`有冲突：${result.conflictPaths.length} 个文件`);
    tab.value = "files";
    if (result.conflictPaths[0]) conflictPath.value = result.conflictPaths[0];
  } else if (result.ok) {
    setStatus(okMsg);
  } else {
    errorMsg.value = result.stderr || result.stdout || "操作失败";
  }
  await refreshAll();
}

async function doFetch() {
  const r = await withBusy(() =>
    invoke<GitOpResult>("git_fetch", { rootPath: props.rootPath }),
  );
  await handleOp(r, "Fetch 完成");
}

async function doPull() {
  const r = await withBusy(() =>
    invoke<GitOpResult>("git_pull", { rootPath: props.rootPath }),
  );
  await handleOp(r, "Pull 完成");
}

async function doPush() {
  if (!confirm("确认 push 到远程？")) return;
  const r = await withBusy(() =>
    invoke<GitOpResult>("git_push", { rootPath: props.rootPath }),
  );
  await handleOp(r, "Push 完成");
}

async function onCheckout(name: string) {
  if (!confirm(`检出分支 ${name}？`)) return;
  await withBusy(() =>
    invoke("git_checkout", { rootPath: props.rootPath, branch: name }),
  );
  setStatus(`已检出 ${name}`);
  await refreshAll();
}

async function onMerge(name: string) {
  if (!confirm(`将 ${name} 合并到当前分支？`)) return;
  const r = await withBusy(() =>
    invoke<GitOpResult>("git_merge", { rootPath: props.rootPath, branch: name }),
  );
  await handleOp(r, `已合并 ${name}`);
}

async function onCreateBranch(name: string, checkout: boolean) {
  await withBusy(() =>
    invoke("git_create_branch", {
      rootPath: props.rootPath,
      name,
      checkout,
    }),
  );
  setStatus(`已创建分支 ${name}`);
  await refreshAll();
}

async function onMergeAbort() {
  if (!confirm("中止合并并回到合并前状态？")) return;
  await withBusy(() => invoke("git_merge_abort", { rootPath: props.rootPath }));
  conflictPath.value = null;
  commitMessage.value = "";
  setStatus("已中止合并");
  await refreshAll();
}

async function onSelectCommit(hash: string) {
  conflictPath.value = null;
  selectedCommit.value = hash;
  selectedPath.value = null;
  await loadShow(hash);
}

async function onStashPush(message: string) {
  await withBusy(() =>
    invoke("git_stash_push", {
      rootPath: props.rootPath,
      message: message || null,
    }),
  );
  setStatus("已 stash");
  await refreshAll();
}

async function onStashApply(index: number) {
  await withBusy(() =>
    invoke("git_stash_apply", { rootPath: props.rootPath, index }),
  );
  setStatus("已 apply stash");
  await refreshAll();
}

async function onStashDrop(index: number) {
  if (!confirm(`删除 stash@{${index}}？`)) return;
  await withBusy(() =>
    invoke("git_stash_drop", { rootPath: props.rootPath, index }),
  );
  await refreshAll();
}

function onConflictResolved() {
  conflictPath.value = null;
  setStatus("冲突已解决并暂存");
  void refreshAll();
}

function openAbsOrRel(path: string) {
  const root = summary.value?.toplevel || props.rootPath;
  const abs = path.match(/^[a-zA-Z]:[\\/]/) || path.startsWith("/")
    ? path
    : `${root.replace(/[\\/]$/, "")}/${path}`;
  emit("openFile", abs.replace(/\//g, "\\"));
}

const summaryLine = computed(() => {
  const s = summary.value;
  if (!s) return "…";
  if (!s.isRepo) return s.error || "不是 git 仓库";
  const parts = [s.branch || "(detached)"];
  if (s.upstream) {
    if (s.ahead) parts.push(`↑${s.ahead}`);
    if (s.behind) parts.push(`↓${s.behind}`);
  }
  if (s.mergeInProgress) {
    parts.push(`merging ${s.mergingInto || ""}`);
    if (s.conflictCount) parts.push(`${s.conflictCount} conflicts`);
  } else if (s.dirty) {
    parts.push("dirty");
  } else {
    parts.push("clean");
  }
  return parts.filter(Boolean).join(" · ");
});

function onKey(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  const tag = t?.tagName?.toLowerCase();
  if (tag === "input" || tag === "textarea") {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      void doCommit();
    }
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
    e.preventDefault();
    void doCommit();
  } else if (e.key === "r" || e.key === "R") {
    if (!e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      void refreshAll();
    }
  }
}

watch(
  () => props.rootPath,
  () => {
    conflictPath.value = null;
    selectedPath.value = null;
    commitMessage.value = "";
    void refreshAll();
  },
);

onMounted(() => {
  void refreshAll();
  window.addEventListener("keydown", onKey);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
});
</script>

<template>
  <section class="git-pane fade-in">
    <header class="git-status-bar">
      <div class="sum" :class="{ warn: summary?.mergeInProgress, bad: !summary?.isRepo && summary }">
        {{ summaryLine }}
      </div>
      <div class="ops">
        <button type="button" class="small" :disabled="busy" @click="refreshAll">刷新</button>
        <button type="button" class="small" :disabled="busy || !summary?.isRepo" @click="doFetch">
          Fetch
        </button>
        <button type="button" class="small" :disabled="busy || !summary?.isRepo" @click="doPull">
          Pull
        </button>
        <button type="button" class="small" :disabled="busy || !summary?.isRepo" @click="doPush">
          Push
        </button>
        <template v-if="summary?.mergeInProgress">
          <button type="button" class="small danger" :disabled="busy" @click="onMergeAbort">
            Abort
          </button>
          <button
            type="button"
            class="small primary"
            :disabled="busy || (summary?.conflictCount ?? 0) > 0"
            @click="doCommit"
          >
            完成合并
          </button>
        </template>
      </div>
    </header>

    <div v-if="errorMsg" class="git-banner err">{{ errorMsg }}</div>
    <div v-else-if="statusMsg" class="git-banner ok">{{ statusMsg }}</div>

    <div v-if="!summary?.isRepo" class="git-empty-repo muted">
      <p>{{ summary?.error || "当前项目根目录不是 git 仓库" }}</p>
      <p class="hint">请将项目根目录指向含有 .git 的仓库路径。</p>
    </div>

    <template v-else>
      <div class="git-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          :class="{ active: tab === 'files' }"
          @click="tab = 'files'"
        >
          Files
        </button>
        <button
          type="button"
          role="tab"
          :class="{ active: tab === 'branches' }"
          @click="tab = 'branches'"
        >
          Branches
        </button>
        <button
          type="button"
          role="tab"
          :class="{ active: tab === 'commits' }"
          @click="tab = 'commits'"
        >
          Commits
        </button>
        <button
          type="button"
          role="tab"
          :class="{ active: tab === 'stash' }"
          @click="tab = 'stash'"
        >
          Stash
        </button>
      </div>

      <div class="git-body">
        <aside class="git-side">
          <GitFiles
            v-if="tab === 'files'"
            :entries="entries"
            :selected-path="selectedPath"
            :selected-staged="selectedStaged"
            @select="onSelectFile"
            @stage="onStage"
            @unstage="onUnstage"
            @discard="onDiscard"
            @open-file="openAbsOrRel"
            @open-conflict="onOpenConflict"
          />
          <GitBranches
            v-else-if="tab === 'branches'"
            :branches="branches"
            :selected="selectedBranch"
            @select="selectedBranch = $event"
            @checkout="onCheckout"
            @merge="onMerge"
            @create="onCreateBranch"
          />
          <GitCommits
            v-else-if="tab === 'commits'"
            :commits="commits"
            :selected="selectedCommit"
            @select="onSelectCommit"
          />
          <GitStash
            v-else
            :entries="stashes"
            :selected="selectedStash"
            @select="selectedStash = $event"
            @push="onStashPush"
            @apply="onStashApply"
            @drop="onStashDrop"
          />
        </aside>

        <div class="git-main">
          <ConflictMerge
            v-if="showConflict && conflictPath"
            :root-path="rootPath"
            :path="conflictPath"
            @resolved="onConflictResolved"
            @open-file="openAbsOrRel"
            @back="conflictPath = null"
            @error="errorMsg = $event"
          />
          <GitDiffView
            v-else
            :title="diffTitle"
            :content="diffText"
            empty-hint="选择文件或提交以查看 diff"
          />
        </div>
      </div>

      <footer class="git-commit-bar">
        <textarea
          v-model="commitMessage"
          class="commit-msg"
          :placeholder="
            summary?.mergeInProgress
              ? '合并提交说明（可空则用默认 MERGE_MSG）'
              : '提交说明（Ctrl+Enter 提交）'
          "
          rows="2"
        />
        <div class="commit-actions">
          <label v-if="!summary?.mergeInProgress" class="amend">
            <input v-model="amend" type="checkbox" />
            Amend
          </label>
          <button
            type="button"
            class="primary small"
            :disabled="busy"
            @click="doCommit"
          >
            {{ summary?.mergeInProgress ? "完成合并" : amend ? "Amend 提交" : "提交" }}
          </button>
        </div>
      </footer>
    </template>
  </section>
</template>

<style scoped>
.git-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel-elevated);
  overflow: hidden;
  box-shadow: var(--shadow);
}
.git-status-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.5rem;
  padding: 0.45rem 0.65rem;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.sum {
  font-size: 12px;
  font-weight: 600;
  flex: 1;
  min-width: 12rem;
}
.sum.warn {
  color: #f59e0b;
}
.sum.bad {
  color: var(--bad, #ef4444);
}
.ops {
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
}
.git-banner {
  padding: 0.35rem 0.65rem;
  font-size: 12px;
  border-bottom: 1px solid var(--border);
}
.git-banner.err {
  background: color-mix(in srgb, var(--bad, #ef4444) 15%, transparent);
  color: var(--bad, #ef4444);
}
.git-banner.ok {
  background: color-mix(in srgb, #22c55e 15%, transparent);
  color: #16a34a;
}
.git-empty-repo {
  padding: 2.5rem 1.5rem;
  text-align: center;
}
.git-empty-repo .hint {
  font-size: 0.9em;
  margin-top: 0.35rem;
}
.git-tabs {
  display: flex;
  gap: 0;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  flex-shrink: 0;
}
.git-tabs button {
  padding: 0.4rem 0.85rem;
  border: none;
  border-right: 1px solid var(--border);
  background: transparent;
  color: var(--muted);
  font-size: 12px;
  font-weight: 600;
  border-radius: 0;
  cursor: pointer;
}
.git-tabs button.active {
  color: var(--text);
  background: var(--panel-elevated);
  box-shadow: inset 0 -2px 0 var(--primary);
}
.git-body {
  flex: 1;
  min-height: 0;
  display: flex;
}
.git-side {
  flex: 0 0 34%;
  max-width: 420px;
  min-width: 220px;
  border-right: 1px solid var(--border);
  min-height: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}
.git-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.git-commit-bar {
  display: flex;
  gap: 0.5rem;
  align-items: flex-end;
  padding: 0.45rem 0.55rem;
  border-top: 1px solid var(--border);
  background: var(--panel);
  flex-shrink: 0;
}
.commit-msg {
  flex: 1;
  min-width: 0;
  resize: vertical;
  max-height: 120px;
  padding: 0.35rem 0.45rem;
  font-size: 12px;
  font-family: inherit;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
}
.commit-actions {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 0.25rem;
}
.amend {
  font-size: 11px;
  color: var(--muted);
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
}
.small {
  font-size: 11px;
  padding: 0.22rem 0.5rem;
  border-radius: 5px;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
  cursor: pointer;
}
.small:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.small.primary,
button.primary.small {
  background: color-mix(in srgb, var(--primary) 18%, var(--panel-elevated));
  border-color: color-mix(in srgb, var(--primary) 45%, var(--border));
  color: var(--primary);
  font-weight: 650;
}
.small.danger {
  color: var(--bad, #ef4444);
  border-color: color-mix(in srgb, var(--bad, #ef4444) 45%, var(--border));
}
</style>
