<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type JobPriority = "high" | "normal" | "low";
export type JobStatus =
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface JobInfo {
  id: string;
  kind: string;
  title: string;
  key?: string | null;
  priority: JobPriority;
  status: JobStatus;
  cancelable: boolean;
  message?: string | null;
  error?: string | null;
  createdAt: number;
  startedAt?: number | null;
  finishedAt?: number | null;
}

interface JobList {
  active: JobInfo[];
  history: JobInfo[];
}

const props = defineProps<{
  open: boolean;
}>();

const emit = defineEmits<{
  close: [];
}>();

const active = ref<JobInfo[]>([]);
const history = ref<JobInfo[]>([]);
const busy = ref(false);

const hasActive = computed(() => active.value.length > 0);

function priorityLabel(p: JobPriority) {
  if (p === "high") return "高";
  if (p === "low") return "低";
  return "中";
}

function statusLabel(s: JobStatus) {
  switch (s) {
    case "queued":
      return "排队";
    case "running":
      return "运行中";
    case "succeeded":
      return "成功";
    case "failed":
      return "失败";
    case "cancelled":
      return "已取消";
    default:
      return s;
  }
}

function formatTime(ms?: number | null) {
  if (!ms) return "";
  const d = new Date(ms);
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

async function refresh() {
  try {
    const list = await invoke<JobList>("list_jobs");
    active.value = list.active ?? [];
    history.value = list.history ?? [];
  } catch {
    /* ignore */
  }
}

function upsertActive(job: JobInfo) {
  const done =
    job.status === "succeeded" ||
    job.status === "failed" ||
    job.status === "cancelled";
  if (done) {
    active.value = active.value.filter((j) => j.id !== job.id);
    history.value = [job, ...history.value.filter((j) => j.id !== job.id)].slice(0, 50);
  } else {
    const i = active.value.findIndex((j) => j.id === job.id);
    if (i >= 0) active.value[i] = job;
    else active.value = [...active.value, job];
  }
}

async function cancel(id: string) {
  busy.value = true;
  try {
    await invoke("cancel_job", { jobId: id });
    await refresh();
  } catch (e) {
    console.warn("cancel_job", e);
  } finally {
    busy.value = false;
  }
}

async function clearHistory() {
  try {
    await invoke("clear_job_history");
    history.value = [];
  } catch {
    /* ignore */
  }
}

let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  await refresh();
  unlisten = await listen<JobInfo>("job-updated", (ev) => {
    upsertActive(ev.payload);
  });
});

onUnmounted(() => {
  if (unlisten) unlisten();
});

watch(
  () => props.open,
  (v) => {
    if (v) void refresh();
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="job-backdrop" @click.self="emit('close')">
      <aside class="job-panel" role="dialog" aria-label="任务中心">
        <header class="job-head">
          <h2>任务中心</h2>
          <button type="button" class="job-close" title="关闭" @click="emit('close')">×</button>
        </header>

        <section class="job-section">
          <div class="job-section-title">
            <span>进行中</span>
            <span class="muted">{{ active.length }}</span>
          </div>
          <p v-if="!hasActive" class="empty">暂无运行中的任务</p>
          <ul v-else class="job-list">
            <li v-for="j in active" :key="j.id" class="job-item">
              <div class="job-main">
                <div class="job-title">{{ j.title }}</div>
                <div class="job-meta">
                  <span class="pill prio">{{ priorityLabel(j.priority) }}</span>
                  <span class="pill" :data-status="j.status">{{ statusLabel(j.status) }}</span>
                  <span v-if="j.message" class="msg">{{ j.message }}</span>
                </div>
              </div>
              <button
                v-if="j.cancelable && (j.status === 'queued' || j.status === 'running')"
                type="button"
                class="job-cancel"
                :disabled="busy"
                @click="cancel(j.id)"
              >
                取消
              </button>
            </li>
          </ul>
        </section>

        <section class="job-section">
          <div class="job-section-title">
            <span>历史</span>
            <button
              v-if="history.length"
              type="button"
              class="linkish"
              @click="clearHistory"
            >
              清空
            </button>
          </div>
          <p v-if="!history.length" class="empty">暂无历史</p>
          <ul v-else class="job-list history">
            <li v-for="j in history" :key="j.id" class="job-item">
              <div class="job-main">
                <div class="job-title">{{ j.title }}</div>
                <div class="job-meta">
                  <span class="pill" :data-status="j.status">{{ statusLabel(j.status) }}</span>
                  <span class="muted">{{ formatTime(j.finishedAt || j.createdAt) }}</span>
                  <span v-if="j.error" class="err">{{ j.error }}</span>
                </div>
              </div>
            </li>
          </ul>
        </section>
      </aside>
    </div>
  </Teleport>
</template>

<style scoped>
.job-backdrop {
  position: fixed;
  inset: 0;
  z-index: 12000;
  background: rgba(0, 0, 0, 0.28);
  display: flex;
  justify-content: flex-end;
}
.job-panel {
  width: min(380px, 100vw);
  height: 100%;
  background: var(--panel, #1e1e1e);
  color: var(--fg, #e8e8e8);
  border-left: 1px solid var(--border, #333);
  display: flex;
  flex-direction: column;
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.25);
}
.job-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px;
  border-bottom: 1px solid var(--border, #333);
}
.job-head h2 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.job-close {
  border: none;
  background: transparent;
  color: inherit;
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
  opacity: 0.7;
}
.job-close:hover {
  opacity: 1;
}
.job-section {
  padding: 10px 12px;
  overflow: auto;
  flex: 1;
  min-height: 0;
}
.job-section + .job-section {
  border-top: 1px solid var(--border, #333);
  flex: 1.2;
}
.job-section-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
  font-weight: 600;
  margin-bottom: 8px;
  opacity: 0.9;
}
.empty {
  margin: 0;
  font-size: 12px;
  opacity: 0.55;
}
.job-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.job-item {
  display: flex;
  gap: 8px;
  align-items: flex-start;
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--panel-2, rgba(255, 255, 255, 0.04));
  border: 1px solid var(--border, #333);
}
.job-main {
  flex: 1;
  min-width: 0;
}
.job-title {
  font-size: 12.5px;
  font-weight: 500;
  word-break: break-word;
}
.job-meta {
  margin-top: 4px;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  font-size: 11px;
}
.pill {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.25);
}
.pill.prio {
  background: rgba(80, 140, 220, 0.25);
}
.pill[data-status="running"] {
  background: rgba(70, 160, 90, 0.3);
}
.pill[data-status="failed"] {
  background: rgba(200, 70, 70, 0.35);
}
.pill[data-status="cancelled"] {
  background: rgba(140, 140, 140, 0.3);
}
.pill[data-status="succeeded"] {
  background: rgba(70, 140, 200, 0.28);
}
.msg {
  opacity: 0.75;
}
.err {
  color: #e88;
  word-break: break-word;
}
.muted {
  opacity: 0.55;
}
.job-cancel {
  flex-shrink: 0;
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 4px;
  border: 1px solid var(--border, #555);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.job-cancel:hover:not(:disabled) {
  background: rgba(200, 70, 70, 0.2);
}
.job-cancel:disabled {
  opacity: 0.5;
  cursor: default;
}
.linkish {
  border: none;
  background: none;
  color: inherit;
  opacity: 0.65;
  font-size: 11px;
  cursor: pointer;
  text-decoration: underline;
}
.linkish:hover {
  opacity: 1;
}
</style>
