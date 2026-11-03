<script setup lang="ts">
import {
  computed,
  defineAsyncComponent,
  nextTick,
  onMounted,
  onUnmounted,
  reactive,
  ref,
  toRef,
  watch,
} from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  BuildBeforeRun,
  GlobalSettings,
  Launchable,
  LogEvent,
  LogLevelFilter,
  ModuleStatus,
  Project,
  RunHealth,
  ScanCache,
  ScanProgress,
  StatusEvent,
  ToolchainInfo,
  UiAccent,
  UiDensity,
  UiPreferences,
  UiTheme,
} from "./types";
import {
  ACCENT_COLORS,
  applyUiPreferences,
  buildStrategyLabel,
  DEFAULT_UI,
  kindLabel,
  naivePrimaryCommon,
  resolvePrimaryColor,
  resolvedThemeMode,
} from "./types";
import {
  classifyLogLevel,
  detectHealthSignal,
  formatLogLineForXterm,
  linePassesFilter,
  stripAnsi,
  type ClassifiedLevel,
} from "./logAnsi";
import {
  TipDedupe,
  classifyLaunchTip,
  summarizeError,
  type LaunchTip,
  type TipAction,
} from "./launchTips";
import {
  NButton,
  NCheckbox,
  NColorPicker,
  NConfigProvider,
  NInput,
  NInputNumber,
  NSelect,
  NSlider,
  NSwitch,
  darkTheme,
  type GlobalThemeOverrides,
  zhCN,
  dateZhCN,
} from "naive-ui";
import type { SearchAction } from "./SearchEverywhere.vue";

/** Heavy UI — split from first paint (Monaco / xterm / tree / search). */
const CodeEditor = defineAsyncComponent(() => import("./CodeEditor.vue"));
const ImagePreview = defineAsyncComponent(() => import("./ImagePreview.vue"));
const LogXterm = defineAsyncComponent(() => import("./LogXterm.vue"));
const TerminalPane = defineAsyncComponent(() => import("./TerminalPane.vue"));
const SearchEverywhere = defineAsyncComponent(() => import("./SearchEverywhere.vue"));
const ProjectFileTree = defineAsyncComponent(() => import("./ProjectFileTree.vue"));
const GitPane = defineAsyncComponent(() => import("./GitPane.vue"));

/** Prefetch editor/terminal chunks after first paint (does not block shell). */
function prefetchHeavyChunks() {
  const run = () => {
    void import("./CodeEditor.vue");
    void import("./LogXterm.vue");
    void import("./ProjectFileTree.vue");
    // Warm toolchain discover disk/memory cache (no UI busy spinner).
    void loadDiscoverHomes(false).catch(() => {});
  };
  const ric = (
    window as Window & {
      requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number;
    }
  ).requestIdleCallback;
  if (typeof ric === "function") {
    ric(run, { timeout: 2500 });
  } else {
    setTimeout(run, 800);
  }
}

/** Cold-start overlay — tracks real boot steps (not fake progress). */
const bootVisible = ref(true);
const bootStep = ref("准备中…");
const bootProgress = ref(8);

function setBoot(step: string, progress: number) {
  bootStep.value = step;
  bootProgress.value = Math.min(100, Math.max(0, progress));
}

async function finishBoot(quick: boolean) {
  setBoot("就绪", 100);
  await nextTick();
  await new Promise((r) => setTimeout(r, quick ? 100 : 200));
  bootVisible.value = false;
}

interface OpenFile {
  path: string;
  name: string;
  content: string;
  dirty: boolean;
  /** text = Monaco; image = read-only preview */
  kind?: "text" | "image";
  /** data: URL for image preview */
  previewUrl?: string;
}

function isImagePath(path: string): boolean {
  return /\.(png|jpe?g|gif|webp|bmp|svg|ico)$/i.test(path);
}

const EDITOR_SPLIT_KEY = "pl.editorSplitPct";
const SIDE_SPLIT_KEY = "pl.sideSplitPct";
const openFiles = ref<OpenFile[]>([]);
const activeFilePath = ref("");
const editorRevealLine = ref<number | null>(null);
const editorSplitPct = ref(
  Math.min(75, Math.max(25, Number(localStorage.getItem(EDITOR_SPLIT_KEY)) || 60)),
);
/** Left module list width % of workspace split row */
const sideSplitPct = ref(
  Math.min(42, Math.max(16, Number(localStorage.getItem(SIDE_SPLIT_KEY)) || 28)),
);
const editorBusy = ref(false);
const searchEverywhereOpen = ref(false);
const fileTreeRef = ref<{
  revealPath: (path: string) => Promise<void>;
  refreshRoot?: () => Promise<void>;
} | null>(null);

const activeOpenFile = computed(
  () => openFiles.value.find((f) => f.path === activeFilePath.value) ?? null,
);

const isDarkUi = computed(() => {
  const t = ui.theme;
  if (t === "dark") return true;
  if (t === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
});

const statusFileLabel = computed(() => {
  const f = activeOpenFile.value;
  if (!f) return "";
  return f.dirty ? `${f.name} *` : f.name;
});

function fileBaseName(p: string): string {
  const norm = p.replace(/\\/g, "/");
  const i = norm.lastIndexOf("/");
  return i >= 0 ? norm.slice(i + 1) : norm;
}

/** Match longest unit path prefix → set active unit for run controls / starred files. */
function syncActiveUnitFromPath(filePath: string) {
  const norm = filePath.replace(/\\/g, "/").toLowerCase();
  let best: Launchable | null = null;
  let bestLen = -1;
  for (const u of units.value) {
    const up = u.path.replace(/\\/g, "/").toLowerCase();
    if (norm === up || norm.startsWith(up + "/")) {
      if (up.length > bestLen) {
        best = u;
        bestLen = up.length;
      }
    }
  }
  if (best) activeUnitId.value = best.id;
}

async function revealInFileTree(path: string) {
  await nextTick();
  try {
    await fileTreeRef.value?.revealPath(path);
  } catch {
    /* ignore */
  }
}

async function loadFileIntoEditor(target: string, line?: number | null) {
  if (!activeProject.value) {
    showErrorWithTip("请先选择项目", { force: true });
    return;
  }
  const existing = openFiles.value.find((f) => f.path === target);
  if (existing) {
    activeFilePath.value = existing.path;
    editorRevealLine.value = line && line > 0 ? line : null;
    syncActiveUnitFromPath(target);
    void revealInFileTree(target);
    return;
  }
  editorBusy.value = true;
  try {
    if (isImagePath(target)) {
      const previewUrl = await invoke<string>("read_image_data_url", {
        projectId: activeProject.value.id,
        path: target,
      });
      openFiles.value.push({
        path: target,
        name: fileBaseName(target),
        content: "",
        dirty: false,
        kind: "image",
        previewUrl,
      });
    } else {
      const content = await invoke<string>("read_text_file", {
        projectId: activeProject.value.id,
        path: target,
      });
      openFiles.value.push({
        path: target,
        name: fileBaseName(target),
        content,
        dirty: false,
        kind: "text",
      });
    }
    activeFilePath.value = target;
    editorRevealLine.value = line && line > 0 ? line : null;
    syncActiveUnitFromPath(target);
    void revealInFileTree(target);
  } catch (e) {
    showErrorWithTip(e, { force: true });
  } finally {
    editorBusy.value = false;
  }
}

function onTreeOpenFile(path: string) {
  void loadFileIntoEditor(path);
}

function onTreePathFocus(path: string) {
  syncActiveUnitFromPath(path);
}

function onSearchOpenFile(payload: { path: string; line?: number }) {
  void loadFileIntoEditor(payload.path, payload.line);
}

function openSearchEverywhere() {
  if (!activeProject.value) {
    showErrorWithTip("请先选择项目", { force: true });
    return;
  }
  searchEverywhereOpen.value = true;
}

function onEditorChange(value: string) {
  const f = activeOpenFile.value;
  if (!f) return;
  f.content = value;
  f.dirty = true;
}

function isHotReloadablePath(path: string): boolean {
  const lower = path.toLowerCase();
  return (
    lower.endsWith(".java") ||
    lower.endsWith(".kt") ||
    lower.endsWith(".yml") ||
    lower.endsWith(".yaml") ||
    lower.endsWith(".properties")
  );
}

/** Pick a running Boot unit that should receive DevTools restart for this file. */
function findHotReloadTarget(filePath: string): Launchable | null {
  const proj = activeProject.value;
  if (!proj?.hotReload) return null;
  const running = units.value.filter((u) => u.kind === "springBoot" && isRunning(u.id));
  if (!running.length) return null;
  const norm = filePath.replace(/\\/g, "/").toLowerCase();
  const byPath = running.find((u) =>
    norm.startsWith(u.path.replace(/\\/g, "/").toLowerCase()),
  );
  if (byPath) return byPath;
  if (activeUnit.value?.kind === "springBoot" && isRunning(activeUnit.value.id)) {
    return activeUnit.value;
  }
  return running[0];
}

async function triggerHotReloadCompile(filePath: string) {
  const proj = activeProject.value;
  const target = findHotReloadTarget(filePath);
  if (!proj || !target || !isHotReloadablePath(filePath) || hotReloadBusy.value) return;
  hotReloadBusy.value = true;
  pendingHotReloadToast.value = true;
  setStatusTask("热更新编译中…", true, true);
  try {
    await invoke<string>("hot_reload_compile", {
      projectId: proj.id,
      unit: target,
      changedPath: filePath,
    });
    if (proj.hotReloadRestart) {
      flash("已编译，等待热重启…");
      clearStatusSoon("已编译，等待 DevTools 热重启…");
    } else {
      pendingHotReloadToast.value = false;
      flash("已增量编译（未重启进程）");
      clearStatusSoon("已增量编译 · 改逻辑需手动 Restart 或开「上下文重启」");
    }
  } catch (e) {
    pendingHotReloadToast.value = false;
    showErrorWithTip(e, { unitId: target.id, force: true });
    clearStatusSoon("热更新编译失败");
  } finally {
    hotReloadBusy.value = false;
  }
}

/** External IDE (Cursor/IDEA) saves — Launcher editor save already calls triggerHotReloadCompile. */
async function syncHotReloadFsWatch() {
  const proj = activeProject.value;
  const anyBootRunning = units.value.some(
    (u) => u.kind === "springBoot" && isRunning(u.id),
  );
  try {
    if (proj?.hotReload && anyBootRunning) {
      await invoke("start_hot_reload_watch", {
        projectId: proj.id,
        rootPath: proj.rootPath,
      });
    } else {
      await invoke("stop_hot_reload_watch", {
        projectId: proj?.id ?? null,
      });
    }
  } catch (e) {
    console.warn("hot-reload watch", e);
  }
}

async function saveActiveFile() {
  const f = activeOpenFile.value;
  if (!f || !activeProject.value || !f.dirty || f.kind === "image") return;
  editorBusy.value = true;
  try {
    await invoke("write_text_file", {
      projectId: activeProject.value.id,
      path: f.path,
      content: f.content,
    });
    f.dirty = false;
    flash(`已保存 ${f.name}`);
    // Vue/前端自带 HMR；Boot 在项目开启热更新且单元运行时走 DevTools
    if (isHotReloadablePath(f.path) && activeProject.value.hotReload) {
      void triggerHotReloadCompile(f.path);
    }
  } catch (e) {
    showErrorWithTip(e, { force: true });
  } finally {
    editorBusy.value = false;
  }
}

function selectEditorTab(path: string) {
  activeFilePath.value = path;
  editorRevealLine.value = null;
  syncActiveUnitFromPath(path);
  void revealInFileTree(path);
}

function closeEditorTab(path: string, ev?: Event) {
  ev?.stopPropagation();
  const idx = openFiles.value.findIndex((f) => f.path === path);
  if (idx < 0) return;
  const file = openFiles.value[idx];
  if (file.dirty && !confirm(`「${file.name}」未保存，确定关闭？`)) return;
  openFiles.value.splice(idx, 1);
  if (activeFilePath.value === path) {
    const next = openFiles.value[Math.min(idx, openFiles.value.length - 1)];
    activeFilePath.value = next?.path || "";
  }
}

let vSplitDragging = false;
let splitStackEl: HTMLElement | null = null;
function onSplitPointerDown(ev: PointerEvent) {
  vSplitDragging = true;
  splitStackEl = (ev.currentTarget as HTMLElement).closest(".main-stack");
  (ev.target as HTMLElement).setPointerCapture?.(ev.pointerId);
  window.addEventListener("pointermove", onSplitPointerMove);
  window.addEventListener("pointerup", onSplitPointerUp);
  window.addEventListener("pointercancel", onSplitPointerUp);
  ev.preventDefault();
}
function onSplitPointerMove(ev: PointerEvent) {
  if (!vSplitDragging || !splitStackEl) return;
  const rect = splitStackEl.getBoundingClientRect();
  if (rect.height <= 0) return;
  const pct = ((ev.clientY - rect.top) / rect.height) * 100;
  editorSplitPct.value = Math.min(75, Math.max(25, pct));
}
function onSplitPointerUp() {
  if (!vSplitDragging) return;
  vSplitDragging = false;
  splitStackEl = null;
  window.removeEventListener("pointermove", onSplitPointerMove);
  window.removeEventListener("pointerup", onSplitPointerUp);
  window.removeEventListener("pointercancel", onSplitPointerUp);
  localStorage.setItem(EDITOR_SPLIT_KEY, String(Math.round(editorSplitPct.value)));
}

let hSplitDragging = false;
let splitRowEl: HTMLElement | null = null;
function onHSplitPointerDown(ev: PointerEvent) {
  hSplitDragging = true;
  splitRowEl = (ev.currentTarget as HTMLElement).closest(".split");
  (ev.target as HTMLElement).setPointerCapture?.(ev.pointerId);
  window.addEventListener("pointermove", onHSplitPointerMove);
  window.addEventListener("pointerup", onHSplitPointerUp);
  window.addEventListener("pointercancel", onHSplitPointerUp);
  ev.preventDefault();
}
function onHSplitPointerMove(ev: PointerEvent) {
  if (!hSplitDragging || !splitRowEl) return;
  const rect = splitRowEl.getBoundingClientRect();
  if (rect.width <= 0) return;
  const pct = ((ev.clientX - rect.left) / rect.width) * 100;
  sideSplitPct.value = Math.min(42, Math.max(16, pct));
}
function onHSplitPointerUp() {
  if (!hSplitDragging) return;
  hSplitDragging = false;
  splitRowEl = null;
  window.removeEventListener("pointermove", onHSplitPointerMove);
  window.removeEventListener("pointerup", onHSplitPointerUp);
  window.removeEventListener("pointercancel", onHSplitPointerUp);
  localStorage.setItem(SIDE_SPLIT_KEY, String(Math.round(sideSplitPct.value)));
}

/** workspace = daily home; settings = secondary; git = repo pane */
const view = ref<"workspace" | "settings" | "git">("workspace");
const settingsSection = ref<"project" | "toolchain" | "appearance">("project");
const moreOpen = ref(false);

const SIDEBAR_KEY = "pl.sidebarCollapsed";
const MODULES_KEY = "pl.modulesCollapsed";
const STARRED_FILES_KEY = "pl.starredFilesOpen";
const sidebarCollapsed = ref(localStorage.getItem(SIDEBAR_KEY) === "1");
const modulesCollapsed = ref(localStorage.getItem(MODULES_KEY) === "1");
/** Starred-files block collapsed by default to keep the tree taller */
const starredFilesOpen = ref(localStorage.getItem(STARRED_FILES_KEY) === "1");

watch(sidebarCollapsed, (v) => localStorage.setItem(SIDEBAR_KEY, v ? "1" : "0"));
watch(modulesCollapsed, (v) => localStorage.setItem(MODULES_KEY, v ? "1" : "0"));
watch(starredFilesOpen, (v) => localStorage.setItem(STARRED_FILES_KEY, v ? "1" : "0"));

function toggleSidebar() {
  sidebarCollapsed.value = !sidebarCollapsed.value;
}
function toggleModules() {
  modulesCollapsed.value = !modulesCollapsed.value;
}
function toggleStarredFiles() {
  starredFilesOpen.value = !starredFilesOpen.value;
}

const global = reactive<GlobalSettings>({
  javaHome: "",
  mavenHome: "",
  nodeHome: "",
  mavenRepoUrl: "",
  mavenLocalRepo: "",
  activeProjectId: null,
  ui: { ...DEFAULT_UI },
});
const ui = reactive<UiPreferences>({ ...DEFAULT_UI });

const naiveTheme = computed(() =>
  resolvedThemeMode(ui) === "dark" ? darkTheme : null,
);
const naiveOverrides = computed<GlobalThemeOverrides>(() => ({
  common: naivePrimaryCommon(ui),
}));
/** Naive control size from density preference */
const controlSize = computed(() => (ui.density === "compact" ? "small" : "medium"));
/** Refs for Naive v-model (avoids template `v-model:value` + reactive nesting quirks). */
const uiTheme = toRef(ui, "theme");
const uiFontSize = toRef(ui, "fontSize");
const uiDensity = toRef(ui, "density");
const primaryHex = computed(() => resolvePrimaryColor(ui));
const globalMavenRepoUrl = toRef(global, "mavenRepoUrl");
const globalMavenLocalRepo = toRef(global, "mavenLocalRepo");

const themeSelectOptions = [
  { label: "跟随系统", value: "system" },
  { label: "浅色", value: "light" },
  { label: "深色", value: "dark" },
];
const densitySelectOptions = [
  { label: "舒适", value: "comfortable" },
  { label: "紧凑", value: "compact" },
];
const buildBeforeRunOptions = [
  { label: "智能 — 仅编译有变更（推荐）", value: "smart" },
  { label: "总是 — 每次强制 compile", value: "always" },
  { label: "从不 — 直接用 target/classes", value: "never" },
];

function pickAccentPreset(key: UiAccent) {
  ui.accent = key;
  ui.primaryColor = ACCENT_COLORS[key];
}

function pickAccentPresetKey(key: string) {
  if (key in ACCENT_COLORS) pickAccentPreset(key as UiAccent);
}

function onPrimaryColorUpdate(v: string) {
  if (!v) return;
  const hex = v.startsWith("#") ? v : `#${v}`;
  ui.primaryColor = hex.slice(0, 7);
  // Clear accent match unless it equals a preset
  const match = (Object.keys(ACCENT_COLORS) as UiAccent[]).find(
    (k) => ACCENT_COLORS[k].toLowerCase() === ui.primaryColor!.toLowerCase(),
  );
  if (match) ui.accent = match;
}
const projects = ref<Project[]>([]);
const activeProject = computed(
  () => projects.value.find((p) => p.id === global.activeProjectId) ?? null,
);

const draft = reactive<Project>({
  id: "",
  name: "",
  rootPath: "",
  javaHome: "",
  mavenHome: "",
  nodeHome: "",
  mavenRepoUrl: "",
  mavenLocalRepo: "",
  scanDepth: 0,
  enableSpringBoot: true,
  enableVue: true,
  buildBeforeRun: "smart",
  hotReload: false,
  hotReloadRestart: false,
});

/** After save+compile, wait for DevTools "Restarted application" once. */
const pendingHotReloadToast = ref(false);
const hotReloadBusy = ref(false);

const units = ref<Launchable[]>([]);
const unitsByProject = reactive<Record<string, Launchable[]>>({});
const activeUnitIdByProject = reactive<Record<string, string>>({});
const kindFilter = ref<"all" | "springBoot" | "vue">("all");
const unitSearch = ref("");
const forceBuild = ref(false);
const profileByUnit = reactive<Record<string, string>>({});
/** Per-unit NODE_HOME (Vue). Empty = inherit project then global. */
const nodeHomeByUnit = reactive<Record<string, string>>({});
/** Starred unit ids for current project (also mirrored in scan cache). */
const starredUnitIds = ref<string[]>([]);
const starredByProject = reactive<Record<string, string[]>>({});
/** unitId → absolute file paths starred for quick edit */
const starredFilesByUnit = reactive<Record<string, string[]>>({});
const starredFilesByProject = reactive<Record<string, Record<string, string[]>>>({});

function isStarred(id: string): boolean {
  return starredUnitIds.value.includes(id);
}

function toggleStar(unit: Launchable, ev?: Event) {
  ev?.stopPropagation();
  const id = unit.id;
  const set = new Set(starredUnitIds.value);
  if (set.has(id)) set.delete(id);
  else set.add(id);
  starredUnitIds.value = [...set];
  const pid = global.activeProjectId;
  if (pid) starredByProject[pid] = [...starredUnitIds.value];
  persistCurrentScan();
}

function activeUnitStarredFiles(): string[] {
  const id = activeUnitId.value;
  if (!id) return [];
  return starredFilesByUnit[id] || [];
}

const activeStarredFiles = computed(() => {
  const paths = activeUnitStarredFiles();
  return paths.map((path) => ({
    path,
    name: fileBaseName(path),
  }));
});

function isFileStarred(path: string, unitId?: string): boolean {
  const id = unitId || activeUnitId.value;
  if (!id || !path) return false;
  return (starredFilesByUnit[id] || []).includes(path);
}

function toggleFileStar(path: string, unitId?: string, ev?: Event) {
  ev?.stopPropagation();
  const id = unitId || activeUnitId.value;
  if (!id || !path) return;
  const list = [...(starredFilesByUnit[id] || [])];
  const i = list.indexOf(path);
  if (i >= 0) list.splice(i, 1);
  else list.push(path);
  if (list.length) starredFilesByUnit[id] = list;
  else delete starredFilesByUnit[id];
  const pid = global.activeProjectId;
  if (pid) {
    starredFilesByProject[pid] = { ...starredFilesByUnit };
  }
  persistCurrentScan();
}

function snapshotStarredFiles(): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [k, v] of Object.entries(starredFilesByUnit)) {
    if (v?.length) out[k] = [...v];
  }
  return out;
}

function applyStarredFiles(map: Record<string, string[]> | undefined) {
  for (const k of Object.keys(starredFilesByUnit)) delete starredFilesByUnit[k];
  if (!map) return;
  for (const [k, v] of Object.entries(map)) {
    if (v?.length) starredFilesByUnit[k] = [...v];
  }
}

function kindRank(kind: string): number {
  if (kind === "springBoot") return 0;
  if (kind === "vue") return 1;
  return 2;
}

const filteredUnits = computed(() => {
  const q = unitSearch.value.trim().toLowerCase();
  let list =
    kindFilter.value === "all"
      ? [...units.value]
      : units.value.filter((u) => u.kind === kindFilter.value);
  if (q) {
    list = list.filter(
      (u) =>
        u.name.toLowerCase().includes(q) ||
        u.relativePath.toLowerCase().includes(q),
    );
  }
  const starred = new Set(starredUnitIds.value);
  list.sort((a, b) => {
    const sa = starred.has(a.id) ? 0 : 1;
    const sb = starred.has(b.id) ? 0 : 1;
    if (sa !== sb) return sa - sb;
    const ka = kindRank(a.kind);
    const kb = kindRank(b.kind);
    if (ka !== kb) return ka - kb;
    return a.name.localeCompare(b.name, "zh-CN");
  });
  return list;
});

/** Launch order for running tabs. */
const startedAt = reactive<Record<string, number>>({});
const healthByUnit = reactive<Record<string, RunHealth>>({});

const activeUnit = computed(
  () => units.value.find((u) => u.id === activeUnitId.value) ?? null,
);

const projectOverrideOpen = computed(
  () =>
    !!(
      draft.javaHome ||
      draft.mavenHome ||
      draft.nodeHome ||
      draft.mavenRepoUrl ||
      draft.mavenLocalRepo
    ),
);

/** Disk scan cache metadata (scannedAt) per project */
const scanMetaByProject = reactive<Record<string, { scannedAt: number }>>({});
const statusTask = ref("");
const statusBusy = ref(false);
const statusIndeterminate = ref(false);
let statusIdleTimer: ReturnType<typeof setTimeout> | undefined;

const cacheLabel = computed(() => {
  const pid = global.activeProjectId;
  if (!pid) return "未选择项目";
  const meta = scanMetaByProject[pid];
  if (!meta?.scannedAt) return "未扫描";
  const d = new Date(meta.scannedAt);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `缓存于 ${hh}:${mm}`;
});

function setStatusTask(message: string, busy = true, indeterminate = true) {
  if (statusIdleTimer) {
    clearTimeout(statusIdleTimer);
    statusIdleTimer = undefined;
  }
  statusTask.value = message;
  statusBusy.value = busy;
  statusIndeterminate.value = indeterminate && busy;
}

function clearStatusSoon(message: string, ms = 2800) {
  statusTask.value = message;
  statusBusy.value = false;
  statusIndeterminate.value = false;
  if (statusIdleTimer) clearTimeout(statusIdleTimer);
  statusIdleTimer = setTimeout(() => {
    statusTask.value = "";
  }, ms);
}

function persistCurrentScan() {
  const pid = global.activeProjectId;
  if (!pid) return;
  unitsByProject[pid] = [...units.value];
  activeUnitIdByProject[pid] = activeUnitId.value;
  void saveScanCacheToDisk(pid);
}

async function saveScanCacheToDisk(projectId: string) {
  const p = projects.value.find((x) => x.id === projectId);
  if (!p) return;
  const profiles: Record<string, string> = {};
  for (const [k, v] of Object.entries(profileByUnit)) {
    if (v) profiles[k] = v;
  }
  const nodeHomes: Record<string, string> = {};
  for (const [k, v] of Object.entries(nodeHomeByUnit)) {
    if (v?.trim()) nodeHomes[k] = v.trim();
  }
  const stars =
    projectId === global.activeProjectId
      ? [...starredUnitIds.value]
      : [...(starredByProject[projectId] || [])];
  const fileStars =
    projectId === global.activeProjectId
      ? snapshotStarredFiles()
      : { ...(starredFilesByProject[projectId] || {}) };
  if (projectId === global.activeProjectId) {
    starredFilesByProject[projectId] = fileStars;
  }
  const scannedAt = scanMetaByProject[projectId]?.scannedAt || Date.now();
  const cache: ScanCache = {
    projectId,
    rootPath: p.rootPath,
    scannedAt,
    units: unitsByProject[projectId] || units.value,
    activeUnitId: activeUnitIdByProject[projectId] || activeUnitId.value || null,
    profileByUnit: profiles,
    nodeHomeByUnit: nodeHomes,
    starredUnitIds: stars,
    starredFilesByUnit: fileStars,
  };
  try {
    await invoke("save_scan_cache_cmd", { cache });
  } catch {
    /* ignore persist errors */
  }
}

async function loadScanCache(projectId: string) {
  try {
    const cache = await invoke<ScanCache | null>("get_scan_cache", { projectId });
    if (!cache || !cache.units?.length) {
      if (!unitsByProject[projectId]) {
        units.value = [];
        activeUnitId.value = "";
      } else {
        restoreScanForProject(projectId);
      }
      return;
    }
    unitsByProject[projectId] = [...cache.units];
    activeUnitIdByProject[projectId] = cache.activeUnitId || cache.units[0]?.id || "";
    scanMetaByProject[projectId] = { scannedAt: cache.scannedAt };
    for (const [id, profile] of Object.entries(cache.profileByUnit || {})) {
      profileByUnit[id] = profile;
    }
    for (const [id, home] of Object.entries(cache.nodeHomeByUnit || {})) {
      if (home?.trim()) nodeHomeByUnit[id] = home.trim();
    }
    const stars = (cache.starredUnitIds || []).filter((id) =>
      cache.units.some((u) => u.id === id),
    );
    starredByProject[projectId] = stars;
    const unitIdSet = new Set(cache.units.map((u) => u.id));
    const fileStars: Record<string, string[]> = {};
    for (const [uid, paths] of Object.entries(cache.starredFilesByUnit || {})) {
      if (!unitIdSet.has(uid) || !paths?.length) continue;
      fileStars[uid] = [...paths];
    }
    starredFilesByProject[projectId] = fileStars;
    for (const u of cache.units) {
      if (u.kind === "springBoot" && !profileByUnit[u.id]) {
        profileByUnit[u.id] = u.selectedSpringProfile || "application";
      }
    }
    restoreScanForProject(projectId);
  } catch {
    restoreScanForProject(projectId);
  }
}

function restoreScanForProject(projectId: string) {
  units.value = unitsByProject[projectId] ? [...unitsByProject[projectId]] : [];
  activeUnitId.value = activeUnitIdByProject[projectId] || "";
  starredUnitIds.value = [...(starredByProject[projectId] || [])];
  applyStarredFiles(starredFilesByProject[projectId]);
}

function profileOf(unit: Launchable): string {
  return profileByUnit[unit.id] || unit.selectedSpringProfile || "application";
}

function setProfile(unit: Launchable, value: string) {
  profileByUnit[unit.id] = value;
  persistCurrentScan();
}

function nodeHomeOf(unit: Launchable): string {
  return nodeHomeByUnit[unit.id] || "";
}

function setNodeHome(unit: Launchable, value: string) {
  const v = value.trim();
  if (v) nodeHomeByUnit[unit.id] = v;
  else delete nodeHomeByUnit[unit.id];
  persistCurrentScan();
}

function onUnitNodeHomeBlur(e: Event) {
  if (!activeUnit.value) return;
  const el = e.target as HTMLInputElement | null;
  if (!el || typeof el.value !== "string") return;
  setNodeHome(activeUnit.value, el.value);
}

function onHotReloadRestartUpdate(v: boolean) {
  draft.hotReloadRestart = v;
  if (v) draft.hotReload = true;
}

/** Inherited NODE_HOME when unit has no override (for placeholder). */
function inheritedNodeHome(): string {
  const projectHome = (draft.nodeHome || activeProject.value?.nodeHome || "").trim();
  if (projectHome) return projectHome;
  return (global.nodeHome || "").trim();
}

const toolchain = ref<ToolchainInfo | null>(null);
const msg = ref("");
const scanError = ref("");
const scanning = ref(false);
const actionError = ref("");
const statusMap = reactive<Record<string, ModuleStatus>>({});

/** Colored console line (IDEA-like). */
interface LogLine {
  text: string;
  stream: string;
  level: ClassifiedLevel;
}

/** Keep more in memory than we paint — avoids DOM thrash / UI lag. */
const LOG_BUFFER_MAX = 20_000;

const logs = reactive<Record<string, LogLine[]>>({});
const activeUnitId = ref("");
const creating = ref(false);
const newName = ref("");
const newRoot = ref("");
const logFollow = ref(true);
const logLevelFilter = ref<LogLevelFilter>("all");
const logFullOpen = ref(false);

let unlistenLog: UnlistenFn | undefined;
let unlistenStatus: UnlistenFn | undefined;
let unlistenScanProgress: UnlistenFn | undefined;
let unlistenHotReloadFs: UnlistenFn | undefined;

const activeLogBuffer = computed(() => logs[activeUnitId.value] ?? []);
const logBufferTotal = computed(() => activeLogBuffer.value.length);
const fullLogPlainText = computed(() =>
  activeLogBuffer.value.map((l) => stripAnsi(l.text)).join("\n"),
);

/** Vue uses embedded xterm + PTY (IDEA-like nested terminal). */
const isTerminalConsole = computed(() => activeUnit.value?.kind === "vue");
const termPaneRef = ref<{ clear: () => void; focus: () => void } | null>(null);
const logXtermRef = ref<{
  write: (chunk: string) => void;
  clear: () => void;
  replay: (chunks: string[]) => void;
  fit: () => void;
} | null>(null);

/** rAF-batched writes for the active Boot log xterm. */
let pendingXtermChunks: string[] = [];
let xtermRaf = 0;

function flushXtermWrites() {
  xtermRaf = 0;
  const chunks = pendingXtermChunks;
  pendingXtermChunks = [];
  if (!chunks.length || !logXtermRef.value) return;
  logXtermRef.value.write(chunks.join(""));
}

function queueXtermWrite(chunk: string) {
  pendingXtermChunks.push(chunk);
  if (!xtermRaf) {
    xtermRaf = requestAnimationFrame(flushXtermWrites);
  }
}

function filteredChunksForBuffer(buf: LogLine[]): string[] {
  return buf
    .filter((l) => linePassesFilter(l.level, logLevelFilter.value))
    .map((l) => formatLogLineForXterm(l.text, l.level));
}

function replayActiveBootLog() {
  if (isTerminalConsole.value) return;
  const chunks = filteredChunksForBuffer(activeLogBuffer.value);
  const tryReplay = (attempt: number) => {
    const pane = logXtermRef.value;
    if (!pane) {
      if (attempt < 8) requestAnimationFrame(() => tryReplay(attempt + 1));
      return;
    }
    pane.replay(chunks);
    pane.fit();
  };
  nextTick(() => tryReplay(0));
}

function applyHealthSignal(
  unitId: string,
  signal: ReturnType<typeof detectHealthSignal>,
  line?: string,
) {
  if (!signal) return;
  const cur = healthByUnit[unitId] || "none";
  // port / failed stick until restart; ready can upgrade from none
  if (signal === "portInUse") {
    const first = cur !== "portInUse";
    healthByUnit[unitId] = "portInUse";
    if (first) {
      showErrorWithTip(line || "Address already in use / 端口占用", {
        health: "portInUse",
        unitId,
      });
    }
    return;
  }
  if (signal === "failed") {
    if (cur !== "portInUse") {
      const first = cur !== "failed";
      healthByUnit[unitId] = "failed";
      if (first) {
        showErrorWithTip(line || "运行失败", { health: "failed", unitId });
      }
    }
    return;
  }
  if (signal === "ready" && cur !== "portInUse" && cur !== "failed") {
    healthByUnit[unitId] = "ready";
  }
}

function resetUnitHealth(unitId: string) {
  healthByUnit[unitId] = "none";
}

/** Attribute for status-dot / badge coloring (only health signals). */
function healthAttr(id: string): RunHealth | undefined {
  const h = healthByUnit[id];
  if (h === "portInUse" || h === "failed" || h === "ready") return h;
  if (statusOf(id) === "failed") return "failed";
  return undefined;
}

function displayStatusLabel(id: string): string {
  const h = healthByUnit[id];
  if (h === "portInUse") return "端口占用";
  if (h === "failed") return "失败";
  if (h === "ready") return "就绪";
  const s = statusOf(id);
  if (s === "failed") return "失败";
  return statusLabel(s);
}

function appendLog(unitId: string, stream: string, line: string) {
  ensureLog(unitId);
  const text = line.startsWith("[err] ") ? line.slice(6) : line;
  const level = classifyLogLevel(text);
  logs[unitId].push({ text, stream, level });
  if (logs[unitId].length > LOG_BUFFER_MAX) {
    logs[unitId] = logs[unitId].slice(-Math.floor(LOG_BUFFER_MAX * 0.8));
  }
  applyHealthSignal(unitId, detectHealthSignal(text), text);
  if (
    unitId === activeUnitId.value &&
    !isTerminalConsole.value &&
    linePassesFilter(level, logLevelFilter.value)
  ) {
    queueXtermWrite(formatLogLineForXterm(text, level));
  }
}

async function copyFullLog() {
  const text = fullLogPlainText.value;
  try {
    await navigator.clipboard.writeText(text);
    flash(`已复制 ${activeLogBuffer.value.length} 行日志`);
  } catch {
    flash("复制失败");
  }
}

function openFullLog() {
  logFullOpen.value = true;
}

watch(activeUnitId, () => {
  logFollow.value = true;
  pendingXtermChunks = [];
  if (xtermRaf) {
    cancelAnimationFrame(xtermRaf);
    xtermRaf = 0;
  }
  replayActiveBootLog();
});

watch(logLevelFilter, () => {
  replayActiveBootLog();
});

watch(isTerminalConsole, (v) => {
  if (!v) replayActiveBootLog();
});

let flashTimer: ReturnType<typeof setTimeout> | undefined;
let errorTimer: ReturnType<typeof setTimeout> | undefined;

const tipDedupe = new TipDedupe();
const errorTip = ref<LaunchTip | null>(null);

function flash(text: string) {
  actionError.value = "";
  errorTip.value = null;
  msg.value = text;
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => {
    if (msg.value === text) msg.value = "";
  }, 2400);
}

function clearErrorToast() {
  actionError.value = "";
  errorTip.value = null;
  if (errorTimer) clearTimeout(errorTimer);
}

function scheduleErrorDismiss(ms: number) {
  if (errorTimer) clearTimeout(errorTimer);
  const text = actionError.value;
  const tipId = errorTip.value?.id;
  errorTimer = setTimeout(() => {
    if (actionError.value === text && errorTip.value?.id === tipId) {
      clearErrorToast();
    }
  }, ms);
}

function showError(text: string) {
  showErrorWithTip(text);
}

/** Classify raw error / log and show toast + optional guidance. */
function showErrorWithTip(
  raw: unknown,
  opts?: { health?: RunHealth | null; unitId?: string | null; force?: boolean },
) {
  msg.value = "";
  const summary = summarizeError(raw);
  const tip = classifyLaunchTip(summary, opts?.health);
  const unitKey = opts?.unitId || "_global";
  if (!opts?.force) {
    if (!tipDedupe.tryShow(unitKey, tip.id)) return;
  } else {
    tipDedupe.tryShow(unitKey, tip.id);
  }
  actionError.value = summary;
  errorTip.value = tip;
  scheduleErrorDismiss(tip.action !== "none" ? 8000 : 6000);
}

function dismissToast() {
  msg.value = "";
  clearErrorToast();
  if (flashTimer) clearTimeout(flashTimer);
}

function runTipAction(action: TipAction) {
  if (action === "toolchain") openSettings("toolchain");
  else if (action === "project") openSettings("project");
  clearErrorToast();
}

function openSettings(section: "project" | "toolchain" | "appearance" = "project") {
  settingsSection.value = section;
  view.value = "settings";
  moreOpen.value = false;
}

function openWorkspace() {
  view.value = "workspace";
  moreOpen.value = false;
}

function openGit() {
  if (!activeProject.value) return;
  view.value = "git";
  moreOpen.value = false;
}

/** Open a path from Git pane (absolute or project-relative). */
async function openFileFromGit(path: string) {
  await loadFileIntoEditor(path);
  // Keep Git view; user can switch to workspace to edit if needed
}

function startCreateFromGuide() {
  creating.value = true;
  openWorkspace();
}

function syncUiFromGlobal(g: GlobalSettings) {
  const src = g.ui ?? DEFAULT_UI;
  const accent = (src.accent ?? "teal") as UiAccent;
  Object.assign(ui, {
    theme: (src.theme ?? "system") as UiTheme,
    fontSize: src.fontSize ?? 14,
    accent,
    primaryColor: src.primaryColor?.trim() || ACCENT_COLORS[accent] || DEFAULT_UI.primaryColor,
    density: (src.density ?? "comfortable") as UiDensity,
  });
  applyUiPreferences(ui);
}

async function refreshProjects() {
  projects.value = await invoke<Project[]>("list_projects");
  const g = await invoke<GlobalSettings>("get_global_settings");
  Object.assign(global, {
    javaHome: g.javaHome ?? "",
    mavenHome: g.mavenHome ?? "",
    nodeHome: g.nodeHome ?? "",
    mavenRepoUrl: g.mavenRepoUrl ?? "",
    mavenLocalRepo: g.mavenLocalRepo ?? "",
    activeProjectId: g.activeProjectId ?? null,
    ui: g.ui ?? { ...DEFAULT_UI },
  });
  syncUiFromGlobal(g);
  syncDraft();
}

function syncDraft() {
  const p = activeProject.value;
  if (!p) return;
  Object.assign(draft, {
    ...p,
    mavenRepoUrl: p.mavenRepoUrl ?? "",
    mavenLocalRepo: p.mavenLocalRepo ?? "",
    buildBeforeRun: (p.buildBeforeRun ?? "smart") as BuildBeforeRun,
    hotReload: !!p.hotReload,
    hotReloadRestart: !!p.hotReloadRestart,
  });
}

watch(activeProject, syncDraft);
watch(activeUnitId, () => {
  persistCurrentScan();
  moreOpen.value = false;
});

type PathPickTarget =
  | "globalJava"
  | "globalMaven"
  | "globalNode"
  | "globalMavenLocal"
  | "draftJava"
  | "draftMaven"
  | "draftNode"
  | "draftMavenLocal"
  | "draftRoot"
  | "newRoot"
  | "unitNode";

type DiscoverKind = "java" | "maven" | "node";

interface ToolchainHomeHit {
  path: string;
  label: string;
  source: string;
}

interface DiscoverToolchainHomes {
  javaHomes: ToolchainHomeHit[];
  mavenHomes: ToolchainHomeHit[];
  nodeHomes: ToolchainHomeHit[];
}

const discoverBusy = ref(false);
const discoverMenu = ref<PathPickTarget | null>(null);
const discoverHits = ref<ToolchainHomeHit[]>([]);
let discoverCache: DiscoverToolchainHomes | null = null;
let discoverCacheAt = 0;

function applyPathTarget(target: PathPickTarget, selected: string) {
  switch (target) {
    case "globalJava":
      global.javaHome = selected;
      break;
    case "globalMaven":
      global.mavenHome = selected;
      break;
    case "globalNode":
      global.nodeHome = selected;
      break;
    case "globalMavenLocal":
      global.mavenLocalRepo = selected;
      break;
    case "draftJava":
      draft.javaHome = selected;
      break;
    case "draftMaven":
      draft.mavenHome = selected;
      break;
    case "draftNode":
      draft.nodeHome = selected;
      break;
    case "draftMavenLocal":
      draft.mavenLocalRepo = selected;
      break;
    case "draftRoot":
      draft.rootPath = selected;
      break;
    case "newRoot":
      newRoot.value = selected;
      break;
    case "unitNode":
      if (activeUnit.value?.kind === "vue") {
        setNodeHome(activeUnit.value, selected);
      }
      break;
  }
}

async function pickPath(target: PathPickTarget) {
  discoverMenu.value = null;
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected !== "string") return;
  applyPathTarget(target, selected);
}

async function loadDiscoverHomes(force = false): Promise<DiscoverToolchainHomes> {
  if (!force && discoverCache && Date.now() - discoverCacheAt < 60_000) {
    return discoverCache;
  }
  discoverBusy.value = true;
  try {
    const res = await invoke<DiscoverToolchainHomes>("discover_toolchain_homes_cmd", {
      force: !!force,
    });
    discoverCache = res;
    discoverCacheAt = Date.now();
    return res;
  } finally {
    discoverBusy.value = false;
  }
}

async function openDiscoverMenu(kind: DiscoverKind, target: PathPickTarget) {
  try {
    const res = await loadDiscoverHomes();
    const hits =
      kind === "java" ? res.javaHomes : kind === "maven" ? res.mavenHomes : res.nodeHomes;
    if (!hits.length) {
      const label = kind === "java" ? "JDK" : kind === "maven" ? "Maven" : "Node";
      showErrorWithTip(
        kind === "java"
          ? `JAVA_HOME / JDK 未找到：系统中未检测到可用的 ${label}`
          : kind === "maven"
            ? `MAVEN_HOME / Maven 未找到：系统中未检测到可用的 ${label}`
            : `NODE_HOME / Node 未找到：系统中未检测到可用的 ${label}`,
        { force: true },
      );
      discoverMenu.value = null;
      discoverHits.value = [];
      return;
    }
    if (hits.length === 1) {
      applyPathTarget(target, hits[0].path);
      discoverMenu.value = null;
      flash(`已填入：${hits[0].label}`);
      return;
    }
    discoverHits.value = hits;
    discoverMenu.value = target;
  } catch (e) {
    showError(String(e));
  }
}

function pickDiscoverHit(path: string | null) {
  if (!path || !discoverMenu.value) return;
  applyPathTarget(discoverMenu.value, path);
  discoverMenu.value = null;
  discoverHits.value = [];
}

function discoverSourceLabel(source: string): string {
  if (source === "env") return "环境变量";
  if (source === "path") return "PATH";
  return "常见目录";
}

const discoverSelectOptions = computed(() =>
  discoverHits.value.map((h) => ({
    label: `[${discoverSourceLabel(h.source)}] ${h.label} — ${h.path}`,
    value: h.path,
  })),
);

async function saveGlobal() {
  await invoke("save_global_settings", {
    settings: { ...global, ui: { ...ui } },
  });
  flash("工具链已保存");
}

async function saveAppearance() {
  ui.fontSize = Math.min(20, Math.max(12, Number(ui.fontSize) || 14));
  applyUiPreferences(ui);
  await invoke("save_global_settings", {
    settings: {
      javaHome: global.javaHome,
      mavenHome: global.mavenHome,
      nodeHome: global.nodeHome,
      mavenRepoUrl: global.mavenRepoUrl,
      mavenLocalRepo: global.mavenLocalRepo,
      activeProjectId: global.activeProjectId,
      ui: { ...ui },
    },
  });
  flash("外观已保存");
}

function previewAppearance() {
  applyUiPreferences(ui);
}

function resetAppearance() {
  Object.assign(ui, { ...DEFAULT_UI });
  applyUiPreferences(ui);
}

watch(
  () => [ui.theme, ui.fontSize, ui.accent, ui.primaryColor, ui.density],
  () => previewAppearance(),
);

async function detectToolchain(nodeHomeOverride: string | null = null) {
  toolchain.value = await invoke<ToolchainInfo>("detect_toolchain_cmd", {
    projectId: global.activeProjectId,
    nodeHome: nodeHomeOverride?.trim() || null,
  });
}

async function detectUnitNode() {
  if (!activeUnit.value || activeUnit.value.kind !== "vue") return;
  await detectToolchain(nodeHomeOf(activeUnit.value) || null);
  if (toolchain.value?.nodeOk) {
    flash(`Node: ${toolchain.value.nodeVersion}`);
  } else {
    showErrorWithTip(
      `Node 不可用: ${toolchain.value?.nodeVersion || "未知"}`,
      { unitId: activeUnit.value.id, force: true },
    );
  }
}

async function selectProject(id: string) {
  const prev = global.activeProjectId;
  if (prev && prev !== id) {
    persistCurrentScan();
    openFiles.value = [];
    activeFilePath.value = "";
  }
  global.activeProjectId = id;
  await invoke("set_active_project", { projectId: id });
  if (prev !== id) {
    if (unitsByProject[id]?.length) {
      restoreScanForProject(id);
    } else {
      await loadScanCache(id);
    }
  }
  syncDraft();
  if (view.value === "settings" && settingsSection.value === "project") {
    /* stay */
  } else if (view.value !== "settings") {
    openWorkspace();
  }
}

async function createProject() {
  if (!newName.value.trim() || !newRoot.value.trim()) {
    showErrorWithTip("请填写项目名称与根目录", { force: true });
    return;
  }
  const p = await invoke<Project>("create_project", {
    name: newName.value.trim(),
    rootPath: newRoot.value.trim(),
  });
  creating.value = false;
  newName.value = "";
  newRoot.value = "";
  await refreshProjects();
  await selectProject(p.id);
  openWorkspace();
  flash("项目已创建");
}

async function saveProjectConfig() {
  if (!draft.id) return;
  if (!draft.hotReload) draft.hotReloadRestart = false;
  await invoke("update_project", { project: { ...draft } });
  await refreshProjects();
  flash("项目配置已保存");
}

async function removeProject(id: string) {
  await invoke("delete_project", { projectId: id });
  delete unitsByProject[id];
  delete activeUnitIdByProject[id];
  delete scanMetaByProject[id];
  delete starredByProject[id];
  delete starredFilesByProject[id];
  await refreshProjects();
  if (global.activeProjectId) {
    await loadScanCache(global.activeProjectId);
  } else {
    units.value = [];
    activeUnitId.value = "";
    starredUnitIds.value = [];
    applyStarredFiles({});
  }
  flash("项目已删除");
}

async function scan() {
  scanError.value = "";
  if (!activeProject.value) {
    scanError.value = "请先选择或创建项目";
    return;
  }
  const pid = activeProject.value.id;
  scanning.value = true;
  setStatusTask("正在扫描…", true, true);
  try {
    units.value = await invoke<Launchable[]>("scan_project", {
      projectId: pid,
    });
    for (const u of units.value) {
      if (u.kind === "springBoot" && !profileByUnit[u.id]) {
        profileByUnit[u.id] = u.selectedSpringProfile || "application";
      }
    }
    if (units.value.length && !units.value.some((u) => u.id === activeUnitId.value)) {
      activeUnitId.value = units.value[0].id;
    } else if (units.value.length && !activeUnitId.value) {
      activeUnitId.value = units.value[0].id;
    }
    scanMetaByProject[pid] = { scannedAt: Date.now() };
    const idSet = new Set(units.value.map((u) => u.id));
    starredUnitIds.value = starredUnitIds.value.filter((id) => idSet.has(id));
    starredByProject[pid] = [...starredUnitIds.value];
    for (const uid of Object.keys(starredFilesByUnit)) {
      if (!idSet.has(uid)) delete starredFilesByUnit[uid];
    }
    starredFilesByProject[pid] = snapshotStarredFiles();
    persistCurrentScan();
    clearStatusSoon(`扫描完成 · ${units.value.length} 个单元`);
  } catch (e) {
    scanError.value = String(e);
    units.value = [];
    persistCurrentScan();
    clearStatusSoon(`扫描失败`);
  } finally {
    scanning.value = false;
  }
}

function ensureLog(id: string) {
  if (!logs[id]) logs[id] = [];
}

async function startUnit(unit: Launchable) {
  if (!activeProject.value) return;
  clearErrorToast();
  tipDedupe.resetUnit(unit.id);
  ensureLog(unit.id);
  activeUnitId.value = unit.id;
  moreOpen.value = false;
  setStatusTask(`启动 ${unit.name}…`, true, true);
  try {
    await invoke("stop_unit", { unitId: unit.id });
  } catch {
    /* not running */
  }
  resetUnitHealth(unit.id);
  startedAt[unit.id] = Date.now();
  statusMap[unit.id] = "starting";
  const springProfile = profileOf(unit);
  const nodeHome = unit.kind === "vue" ? nodeHomeOf(unit) || null : null;
  try {
    await invoke("start_unit", {
      projectId: activeProject.value.id,
      unit: { ...unit, selectedSpringProfile: springProfile },
      springProfile,
      nodeHome,
    });
    clearStatusSoon(`已启动 ${unit.name}`);
  } catch (e) {
    statusMap[unit.id] = "failed";
    healthByUnit[unit.id] = "failed";
    showErrorWithTip(e, { health: "failed", unitId: unit.id, force: true });
    clearStatusSoon(`启动失败: ${unit.name}`);
  }
}

async function buildUnit(unit: Launchable) {
  if (!activeProject.value) return;
  clearErrorToast();
  tipDedupe.resetUnit(unit.id);
  ensureLog(unit.id);
  activeUnitId.value = unit.id;
  moreOpen.value = false;
  setStatusTask(`构建 ${unit.name}…`, true, true);
  statusMap[unit.id] = "starting";
  try {
    await invoke("build_unit", {
      projectId: activeProject.value.id,
      unit,
      force: forceBuild.value,
    });
    clearStatusSoon(`构建完成 ${unit.name}`);
  } catch (e) {
    statusMap[unit.id] = "failed";
    healthByUnit[unit.id] = "failed";
    showErrorWithTip(e, { unitId: unit.id, force: true });
    clearStatusSoon(`构建失败: ${unit.name}`);
  }
}

async function installDeps(unit: Launchable) {
  if (!activeProject.value) return;
  clearErrorToast();
  ensureLog(unit.id);
  activeUnitId.value = unit.id;
  moreOpen.value = false;
  setStatusTask(`安装依赖 ${unit.name}…`, true, true);
  try {
    await invoke("install_local_deps", {
      projectId: activeProject.value.id,
      unit,
    });
    clearStatusSoon(`依赖安装完成 ${unit.name}`);
  } catch (e) {
    showErrorWithTip(e, { unitId: unit.id, force: true });
    clearStatusSoon(`安装依赖失败`);
  }
}

async function stopUnit(unit: Launchable) {
  clearErrorToast();
  moreOpen.value = false;
  try {
    await invoke("stop_unit", { unitId: unit.id });
  } catch (e) {
    showErrorWithTip(e, { unitId: unit.id, force: true });
  }
}

function clearLog() {
  if (activeUnitId.value) logs[activeUnitId.value] = [];
  pendingXtermChunks = [];
  if (xtermRaf) {
    cancelAnimationFrame(xtermRaf);
    xtermRaf = 0;
  }
  if (isTerminalConsole.value) termPaneRef.value?.clear();
  else logXtermRef.value?.clear();
}

function statusOf(id: string): ModuleStatus {
  return statusMap[id] ?? "idle";
}

function isRunning(id: string) {
  const s = statusOf(id);
  return s === "running" || s === "starting";
}

const runningTabs = computed(() =>
  units.value
    .filter((u) => isRunning(u.id))
    .sort((a, b) => (startedAt[a.id] || 0) - (startedAt[b.id] || 0)),
);

function statusLabel(s: ModuleStatus): string {
  switch (s) {
    case "idle":
      return "空闲";
    case "starting":
      return "启动中";
    case "running":
      return "运行中";
    case "stopped":
      return "已停止";
    case "failed":
      return "失败";
    default:
      return s;
  }
}

function selectRunningTab(id: string) {
  activeUnitId.value = id;
}

let schemeListener: ((e: MediaQueryListEvent) => void) | undefined;
let schemeMq: MediaQueryList | undefined;

/** Custom titlebar (decorations: false) — window chrome */
const appWindow = getCurrentWindow();
const winMaximized = ref(false);
let unlistenWinResize: UnlistenFn | undefined;

async function syncWinMaximized() {
  try {
    winMaximized.value = await appWindow.isMaximized();
  } catch {
    /* ignore in browser preview */
  }
}

async function winMinimize() {
  try {
    await appWindow.minimize();
  } catch {
    /* ignore */
  }
}

async function winToggleMaximize() {
  try {
    await appWindow.toggleMaximize();
    await syncWinMaximized();
  } catch {
    /* ignore */
  }
}

async function winClose() {
  try {
    await appWindow.close();
  } catch {
    /* ignore */
  }
}

const searchActions = computed<SearchAction[]>(() => {
  const u = activeUnit.value;
  const actions: SearchAction[] = [
    { id: "scan", label: "扫描项目单元", hint: "重新发现 Boot / Vue 模块" },
    { id: "search", label: "搜索所有地方", hint: "Ctrl+Shift+F" },
    { id: "settings-project", label: "打开项目设置", hint: "编译 / 热更新" },
    { id: "settings-toolchain", label: "打开工具链设置", hint: "JAVA / Maven / Node" },
    { id: "save-file", label: "保存当前文件", hint: "Ctrl+S" },
  ];
  if (u) {
    actions.push(
      {
        id: "start-unit",
        label: `启动单元：${u.name}`,
        hint: u.kind === "springBoot" ? "Spring Boot" : "Vue",
      },
      { id: "stop-unit", label: `停止单元：${u.name}` },
      { id: "build-unit", label: `构建单元：${u.name}`, hint: "仅 Boot" },
    );
  }
  return actions;
});

function onSearchRunAction(id: string) {
  switch (id) {
    case "scan":
      void scan();
      break;
    case "search":
      openSearchEverywhere();
      break;
    case "settings-project":
      openSettings("project");
      break;
    case "settings-toolchain":
      openSettings("toolchain");
      break;
    case "save-file":
      void saveActiveFile();
      break;
    case "start-unit":
      if (activeUnit.value) void startUnit(activeUnit.value);
      break;
    case "stop-unit":
      if (activeUnit.value) void stopUnit(activeUnit.value);
      break;
    case "build-unit":
      if (activeUnit.value) void buildUnit(activeUnit.value);
      break;
    default:
      break;
  }
}

let lastShiftAt = 0;
function onGlobalKeydown(ev: KeyboardEvent) {
  if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "s") {
    if (view.value !== "workspace" || !activeOpenFile.value) return;
    ev.preventDefault();
    void saveActiveFile();
    return;
  }
  if ((ev.ctrlKey || ev.metaKey) && ev.shiftKey && ev.key.toLowerCase() === "f") {
    if (view.value !== "workspace") return;
    ev.preventDefault();
    openSearchEverywhere();
    return;
  }
  // IDEA-style Double Shift (ignore when typing in inputs unless already open)
  if (ev.key === "Shift" && !ev.ctrlKey && !ev.metaKey && !ev.altKey) {
    const now = Date.now();
    if (now - lastShiftAt < 400) {
      const t = ev.target as HTMLElement | null;
      const tag = t?.tagName?.toLowerCase();
      const typing =
        tag === "input" ||
        tag === "textarea" ||
        tag === "select" ||
        t?.isContentEditable;
      if (!typing || searchEverywhereOpen.value) {
        if (view.value === "workspace") {
          ev.preventDefault();
          openSearchEverywhere();
        }
      }
      lastShiftAt = 0;
    } else {
      lastShiftAt = now;
    }
  }
}

onMounted(async () => {
  applyUiPreferences(ui);
  window.addEventListener("keydown", onGlobalKeydown);
  schemeMq = window.matchMedia("(prefers-color-scheme: dark)");
  schemeListener = () => {
    if (ui.theme === "system") applyUiPreferences(ui);
  };
  schemeMq.addEventListener("change", schemeListener);

  try {
    setBoot("读取配置…", 18);
    // Projects first (sets activeProjectId); then cache / window / running in parallel.
    setBoot("加载项目…", 36);
    await refreshProjects();

    const empty = !projects.value.length;
    if (empty) {
      setBoot("就绪", 100);
      await syncWinMaximized();
    } else {
      setBoot("恢复扫描缓存…", 58);
      await Promise.all([
        global.activeProjectId
          ? loadScanCache(global.activeProjectId)
          : Promise.resolve(),
        syncWinMaximized(),
        (async () => {
          try {
            const running = await invoke<{ unitId: string; status: string }[]>(
              "list_running",
            );
            const now = Date.now();
            for (const r of running) {
              statusMap[r.unitId] = "running";
              if (!startedAt[r.unitId]) startedAt[r.unitId] = now;
            }
          } catch {
            /* ignore */
          }
        })(),
      ]);
      setBoot("同步窗口状态…", 88);
    }

    try {
      unlistenWinResize = await appWindow.onResized(() => {
        void syncWinMaximized();
      });
    } catch {
      /* ignore */
    }

    unlistenLog = await listen<LogEvent>("module-log", (event) => {
      const { unitId, stream, line } = event.payload;
      appendLog(unitId, stream, line);
      if (
        pendingHotReloadToast.value &&
        /Restarted application|restartedApplication/i.test(line)
      ) {
        pendingHotReloadToast.value = false;
        flash("热更新完成");
        clearStatusSoon("热更新完成");
      }
    });
    unlistenStatus = await listen<StatusEvent>("module-status", (event) => {
      const { unitId, status, message } = event.payload;
      statusMap[unitId] = status;
      if (status === "failed" && healthByUnit[unitId] !== "portInUse") {
        const first = healthByUnit[unitId] !== "failed";
        healthByUnit[unitId] = "failed";
        if (first) {
          showErrorWithTip(message || "单元运行失败", {
            health: "failed",
            unitId,
          });
        }
      }
      if (status === "starting") {
        if (!startedAt[unitId]) startedAt[unitId] = Date.now();
      }
      if (
        (status === "stopped" || status === "failed" || status === "idle") &&
        activeUnitId.value === unitId &&
        runningTabs.value.length
      ) {
        /* stay on unit; tabs list updates via computed */
      }
    });
    unlistenScanProgress = await listen<ScanProgress>("scan-progress", (event) => {
      const p = event.payload;
      if (p.projectId !== global.activeProjectId) return;
      if (p.phase === "done") {
        clearStatusSoon(p.message || `扫描完成 · ${p.found} 个`);
      } else if (p.phase === "error") {
        clearStatusSoon(p.message || "扫描失败");
      } else {
        setStatusTask(p.message || "扫描中…", true, true);
      }
    });
    unlistenHotReloadFs = await listen<{ projectId: string; path: string }>(
      "hot-reload-fs-change",
      (event) => {
        const { projectId, path } = event.payload;
        if (projectId !== global.activeProjectId) return;
        void triggerHotReloadCompile(path);
      },
    );
    void syncHotReloadFsWatch();
    prefetchHeavyChunks();
    await finishBoot(empty);
  } catch (e) {
    showErrorWithTip(e, { force: true });
    bootVisible.value = false;
  }
});

watch(
  [
    () => activeProject.value?.id,
    () => activeProject.value?.hotReload,
    () => activeProject.value?.rootPath,
    () =>
      units.value
        .filter((u) => u.kind === "springBoot" && isRunning(u.id))
        .map((u) => u.id)
        .join(","),
  ],
  () => {
    void syncHotReloadFsWatch();
  },
);

onUnmounted(() => {
  window.removeEventListener("keydown", onGlobalKeydown);
  if (schemeMq && schemeListener) {
    schemeMq.removeEventListener("change", schemeListener);
  }
  if (statusIdleTimer) clearTimeout(statusIdleTimer);
  if (flashTimer) clearTimeout(flashTimer);
  if (errorTimer) clearTimeout(errorTimer);
  unlistenLog?.();
  unlistenStatus?.();
  unlistenScanProgress?.();
  unlistenHotReloadFs?.();
  unlistenWinResize?.();
  void invoke("stop_hot_reload_watch", { projectId: null }).catch(() => {});
});
</script>

<template>
  <NConfigProvider
    :theme="naiveTheme"
    :theme-overrides="naiveOverrides"
    :locale="zhCN"
    :date-locale="dateZhCN"
  >
  <div class="app" :data-control-size="controlSize">
    <Transition name="boot-fade">
      <div
        v-if="bootVisible"
        class="boot-overlay"
        role="status"
        aria-live="polite"
        aria-busy="true"
      >
        <div class="boot-card">
          <div class="boot-brand">
            <span class="brand-mark" aria-hidden="true" />
            <strong>SparkRun</strong>
          </div>
          <p class="boot-step">{{ bootStep }}</p>
          <div class="boot-bar" aria-hidden="true">
            <div class="boot-bar-fill" :style="{ width: `${bootProgress}%` }" />
          </div>
        </div>
      </div>
    </Transition>
    <header class="header titlebar">
      <div class="brand-block">
        <button type="button" class="brand" @click="openWorkspace">
          <span class="brand-mark" aria-hidden="true" />
          SparkRun
        </button>
        <span v-if="activeProject && view === 'workspace'" class="header-ctx muted">
          {{ activeProject.name }}
        </span>
      </div>
      <div
        class="titlebar-drag"
        data-tauri-drag-region
        title="拖动窗口 · 双击最大化"
        @dblclick="winToggleMaximize"
      />
      <div class="header-actions">
        <button
          type="button"
          class="nav-btn"
          :class="{ active: view === 'workspace' }"
          @click="openWorkspace"
        >
          工作区
        </button>
        <button
          type="button"
          class="nav-btn"
          :class="{ active: view === 'git' }"
          :disabled="!activeProject"
          title="Git（分支 / 提交 / 合并）"
          @click="openGit"
        >
          Git
        </button>
        <button
          type="button"
          class="nav-btn"
          :class="{ active: view === 'settings' }"
          @click="openSettings(activeProject ? 'project' : 'toolchain')"
        >
          设置
        </button>
        <button
          type="button"
          class="nav-btn icon-btn"
          title="搜索 (Ctrl+Shift+F)"
          :disabled="!activeProject"
          @click="openSearchEverywhere"
        >
          ⌕
        </button>
        <div class="win-controls" aria-label="窗口控制">
          <button type="button" class="win-btn" title="最小化" @click="winMinimize">
            <span aria-hidden="true">─</span>
          </button>
          <button
            type="button"
            class="win-btn"
            :title="winMaximized ? '还原' : '最大化'"
            @click="winToggleMaximize"
          >
            <span aria-hidden="true">{{ winMaximized ? "❐" : "□" }}</span>
          </button>
          <button type="button" class="win-btn win-close" title="关闭" @click="winClose">
            <span aria-hidden="true">×</span>
          </button>
        </div>
      </div>
    </header>

    <div class="body" :class="{ 'sidebar-collapsed': sidebarCollapsed }">
      <aside class="sidebar" :class="{ collapsed: sidebarCollapsed }">
        <div class="side-head">
          <button
            type="button"
            class="collapse-btn"
            :title="sidebarCollapsed ? '展开项目' : '收起项目'"
            @click="toggleSidebar"
          >
            {{ sidebarCollapsed ? "›" : "‹" }}
          </button>
          <template v-if="!sidebarCollapsed">
            <span class="side-label">项目</span>
            <button type="button" class="small primary" @click="creating = !creating">
              {{ creating ? "取消" : "新建" }}
            </button>
          </template>
        </div>
        <template v-if="!sidebarCollapsed">
          <div v-if="creating" class="create-box">
            <input v-model="newName" placeholder="项目名称" />
            <div class="row">
              <input v-model="newRoot" placeholder="根目录" />
              <button type="button" class="small" @click="pickPath('newRoot')">…</button>
            </div>
            <button type="button" class="primary small" @click="createProject">创建</button>
          </div>
          <button
            v-for="(p, i) in projects"
            :key="p.id"
            type="button"
            class="project-item"
            :class="{ active: p.id === global.activeProjectId }"
            :style="{ animationDelay: `${i * 40}ms` }"
            @click="selectProject(p.id)"
          >
            <div class="pname">{{ p.name }}</div>
            <div class="proot">{{ p.rootPath }}</div>
          </button>
          <div v-if="!projects.length && !creating" class="side-empty">暂无项目</div>
        </template>
        <div v-else class="side-rail" :title="activeProject?.name || '展开项目栏'">
          <button type="button" class="rail-expand" @click="toggleSidebar">项目</button>
          <span v-if="activeProject" class="rail-name">{{ activeProject.name }}</span>
        </div>
      </aside>

      <main class="main">
        <!-- ===== WORKSPACE ===== -->
        <section v-show="view === 'workspace'" class="panel workspace fade-in">
          <template v-if="!projects.length">
            <div class="guide">
              <h2>开始使用</h2>
              <p class="hint">按步骤配置一次，之后每天只需扫描与启停。</p>
              <ol class="guide-steps">
                <li>
                  <div class="step-num">1</div>
                  <div class="step-body">
                    <strong>配置工具链</strong>
                    <p class="muted">设置 JAVA_HOME / MAVEN_HOME，可选 Maven 仓库</p>
                    <button type="button" class="small" @click="openSettings('toolchain')">
                      打开工具链
                    </button>
                  </div>
                </li>
                <li>
                  <div class="step-num">2</div>
                  <div class="step-body">
                    <strong>新建项目</strong>
                    <p class="muted">选择仓库根目录（如 new-ypyh）</p>
                    <button type="button" class="small primary" @click="startCreateFromGuide">
                      新建项目
                    </button>
                  </div>
                </li>
                <li>
                  <div class="step-num">3</div>
                  <div class="step-body">
                    <strong>扫描并启动</strong>
                    <p class="muted">发现 Spring Boot / Vue 单元后一键启动</p>
                  </div>
                </li>
              </ol>
            </div>
          </template>

          <template v-else-if="activeProject">
            <div class="workspace-bar">
              <div class="ws-meta">
                <strong class="ws-name" :title="activeProject.rootPath">{{
                  activeProject.name
                }}</strong>
                <span class="ws-path muted" :title="activeProject.rootPath">{{
                  activeProject.rootPath
                }}</span>
                <button
                  type="button"
                  class="chip chip-quiet"
                  :title="'点击修改启动前编译策略'"
                  @click="openSettings('project')"
                >
                  {{ buildStrategyLabel(activeProject.buildBeforeRun) }}
                </button>
                <button
                  type="button"
                  class="chip chip-quiet"
                  :class="{ 'chip-on': activeProject.hotReload }"
                  :title="
                    activeProject.hotReloadRestart
                      ? '增量编译 + DevTools 上下文重启（慢）'
                      : activeProject.hotReload
                        ? '保存后仅增量编译，不重启进程'
                        : '点击开启：保存后增量编译'
                  "
                  @click="openSettings('project')"
                >
                  {{
                    activeProject.hotReloadRestart
                      ? "编译+重启 · 开"
                      : activeProject.hotReload
                        ? "仅编译 · 开"
                        : "热编译 · 关"
                  }}
                </button>
              </div>
              <div class="ws-tools">
                <select v-model="kindFilter" class="filter-select">
                  <option value="all">全部类型</option>
                  <option value="springBoot">Spring Boot</option>
                  <option value="vue">Vue</option>
                </select>
                <label class="force-label">
                  <input v-model="forceBuild" type="checkbox" />
                  强制构建
                </label>
                <button
                  type="button"
                  class="small"
                  :disabled="
                    !activeOpenFile?.dirty ||
                    activeOpenFile?.kind === 'image' ||
                    editorBusy
                  "
                  title="保存 (Ctrl+S)"
                  @click="saveActiveFile"
                >
                  保存
                </button>
                <button
                  type="button"
                  class="small"
                  :disabled="scanning"
                  title="重新扫描可启动单元"
                  @click="scan"
                >
                  {{ scanning ? "扫描中…" : "扫描" }}
                </button>
              </div>
            </div>
            <p v-if="scanError" class="bad">{{ scanError }}</p>

            <div class="split" :class="{ 'modules-collapsed': modulesCollapsed }">
              <div
                v-if="!modulesCollapsed"
                class="module-list side-panel"
                :style="{ flex: `0 0 ${sideSplitPct}%` }"
              >
                <div class="list-head">
                  <button
                    type="button"
                    class="collapse-btn"
                    title="收起侧栏"
                    @click="toggleModules"
                  >
                    ‹
                  </button>
                  <span class="list-head-label">文件</span>
                </div>
                <div class="unit-strip">
                  <span
                    class="status-dot"
                    :data-status="activeUnit ? statusOf(activeUnit.id) : 'idle'"
                    :data-health="activeUnit ? healthAttr(activeUnit.id) : undefined"
                  />
                  <select
                    v-model="activeUnitId"
                    class="unit-select"
                    :disabled="!filteredUnits.length"
                    aria-label="当前单元"
                  >
                    <option value="" disabled>选择运行单元…</option>
                    <option v-for="u in filteredUnits" :key="u.id" :value="u.id">
                      {{ u.name }} · {{ u.kind === "springBoot" ? "Boot" : "Vue" }}
                    </option>
                  </select>
                  <button
                    type="button"
                    class="star-btn"
                    :class="{ on: activeUnit && isStarred(activeUnit.id) }"
                    :disabled="!activeUnit"
                    :title="activeUnit && isStarred(activeUnit.id) ? '取消标星' : '标星单元'"
                    @click="activeUnit && toggleStar(activeUnit, $event)"
                  >
                    {{ activeUnit && isStarred(activeUnit.id) ? "★" : "☆" }}
                  </button>
                </div>
                <div class="starred-files compact">
                  <button
                    type="button"
                    class="starred-files-toggle"
                    :aria-expanded="starredFilesOpen"
                    @click="toggleStarredFiles"
                  >
                    <span class="starred-twist" aria-hidden="true">{{
                      starredFilesOpen ? "▾" : "▸"
                    }}</span>
                    <span>收藏</span>
                    <span class="muted starred-count">{{
                      activeStarredFiles.length
                        ? `${activeStarredFiles.length}`
                        : "0"
                    }}</span>
                  </button>
                  <template v-if="starredFilesOpen">
                    <div v-if="!activeUnit" class="starred-empty muted">
                      点选文件树自动匹配单元
                    </div>
                    <div v-else-if="!activeStarredFiles.length" class="starred-empty muted">
                      Tab 上点 ★ 收藏
                    </div>
                    <button
                      v-for="f in activeStarredFiles"
                      :key="f.path"
                      type="button"
                      class="starred-file-item"
                      :class="{ active: activeFilePath === f.path }"
                      :title="f.path"
                      @click="loadFileIntoEditor(f.path)"
                    >
                      <span class="starred-file-name">{{ f.name }}</span>
                      <span
                        class="star-btn on"
                        title="取消标星"
                        @click="toggleFileStar(f.path, activeUnitId, $event)"
                        >★</span
                      >
                    </button>
                  </template>
                </div>
                <ProjectFileTree
                  v-if="activeProject"
                  ref="fileTreeRef"
                  :project-id="activeProject.id"
                  :root-path="activeProject.rootPath"
                  @open="onTreeOpenFile"
                  @path-focus="onTreePathFocus"
                />
                <div v-else class="module-empty muted">请先选择项目</div>
              </div>
              <button
                v-else
                type="button"
                class="modules-rail"
                title="展开文件树"
                @click="toggleModules"
              >
                <span class="rail-chevron">›</span>
                <span class="rail-label">文件</span>
                <span class="rail-count">{{ filteredUnits.length }}</span>
              </button>

              <div
                v-if="!modulesCollapsed"
                class="h-split"
                title="拖动调整左右宽度"
                @pointerdown="onHSplitPointerDown"
              />

              <div class="main-stack">
                <div
                  class="editor-panel"
                  :style="{ flex: `0 0 ${editorSplitPct}%` }"
                >
                  <div class="editor-tabs" role="tablist">
                    <button
                      v-for="f in openFiles"
                      :key="f.path"
                      type="button"
                      role="tab"
                      class="editor-tab"
                      :class="{ active: activeFilePath === f.path }"
                      :title="f.path"
                      @click="selectEditorTab(f.path)"
                      @auxclick.middle.prevent="closeEditorTab(f.path)"
                    >
                      <span
                        class="star-btn tab-star"
                        :class="{ on: isFileStarred(f.path) }"
                        :title="isFileStarred(f.path) ? '取消文件标星' : '标星到左侧快捷区'"
                        @click="toggleFileStar(f.path, activeUnitId, $event)"
                        >{{ isFileStarred(f.path) ? "★" : "☆" }}</span
                      >
                      <span class="editor-tab-name">{{ f.name }}{{ f.dirty ? " *" : "" }}</span>
                      <span
                        class="editor-tab-close"
                        title="关闭"
                        @click="closeEditorTab(f.path, $event)"
                        >×</span
                      >
                    </button>
                    <button
                      type="button"
                      class="editor-tab editor-tab-add"
                      title="搜索所有地方 (Ctrl+Shift+F)"
                      @click="openSearchEverywhere"
                    >
                      +
                    </button>
                  </div>
                  <div class="editor-body">
                    <ImagePreview
                      v-if="activeOpenFile?.kind === 'image' && activeOpenFile.previewUrl"
                      :key="activeOpenFile.path"
                      :path="activeOpenFile.path"
                      :name="activeOpenFile.name"
                      :src="activeOpenFile.previewUrl"
                    />
                    <CodeEditor
                      v-else-if="activeOpenFile"
                      :key="activeOpenFile.path"
                      :path="activeOpenFile.path"
                      :content="activeOpenFile.content"
                      :dark="isDarkUi"
                      :active="view === 'workspace'"
                      :reveal-line="editorRevealLine"
                      @change="onEditorChange"
                      @save="saveActiveFile"
                    />
                    <div v-else class="editor-empty">
                      <p class="editor-empty-title">打开文件，或选择单元后启动</p>
                      <p class="muted editor-empty-hint">
                        左侧文件树单击打开 · 日常操作为下方「启动 / 停止」
                      </p>
                      <div class="editor-empty-actions">
                        <button
                          v-if="activeUnit && !isRunning(activeUnit.id)"
                          type="button"
                          class="primary"
                          @click="startUnit(activeUnit)"
                        >
                          启动 {{ activeUnit.name }}
                        </button>
                        <button
                          type="button"
                          class="small"
                          :disabled="!activeProject"
                          @click="openSearchEverywhere"
                        >
                          搜索文件
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
                <div
                  class="v-split"
                  title="拖动调整编辑器 / 控制台高度"
                  @pointerdown="onSplitPointerDown"
                />
                <div class="log-panel" :style="{ flex: `1 1 ${100 - editorSplitPct}%` }">
                <div v-if="runningTabs.length" class="run-tabs" role="tablist">
                  <button
                    v-for="t in runningTabs"
                    :key="t.id"
                    type="button"
                    role="tab"
                    class="run-tab"
                    :class="{ active: activeUnitId === t.id }"
                    :aria-selected="activeUnitId === t.id"
                    @click="selectRunningTab(t.id)"
                  >
                    <span
                      class="status-dot"
                      :data-status="statusOf(t.id)"
                      :data-health="healthAttr(t.id)"
                    />
                    <span class="run-tab-name">{{ t.name }}</span>
                    <span class="run-tab-hint muted">{{ displayStatusLabel(t.id) }}</span>
                  </button>
                </div>
                <div v-if="activeUnit" class="action-bar">
                  <div class="action-meta">
                    <strong>{{ activeUnit.name }}</strong>
                    <span class="kind">{{ kindLabel(activeUnit.kind) }}</span>
                    <span
                      class="badge"
                      :data-status="statusOf(activeUnit.id)"
                      :data-health="healthAttr(activeUnit.id)"
                      >{{ displayStatusLabel(activeUnit.id) }}</span
                    >
                    <span
                      v-if="
                        activeProject?.hotReload &&
                        activeUnit.kind === 'springBoot' &&
                        isRunning(activeUnit.id)
                      "
                      class="badge hot-reload-badge"
                      :title="
                        activeProject.hotReloadRestart
                          ? 'DevTools 上下文重启已启用'
                          : '仅增量编译，改代码后需手动 Restart 才生效'
                      "
                      >{{ activeProject.hotReloadRestart ? "编译+重启" : "仅编译" }}</span
                    >
                    <select
                      v-if="
                        activeUnit.kind === 'springBoot' &&
                        (activeUnit.springProfiles?.length || 0) > 0
                      "
                      class="profile-select"
                      :value="profileOf(activeUnit)"
                      :disabled="isRunning(activeUnit.id)"
                      title="Spring Profile"
                      @change="
                        setProfile(activeUnit, ($event.target as HTMLSelectElement).value)
                      "
                    >
                      <option
                        v-for="p in activeUnit.springProfiles"
                        :key="p"
                        :value="p"
                      >
                        {{ p === "application" ? "application（默认）" : "application-" + p }}
                      </option>
                    </select>
                  </div>
                  <div class="action-btns">
                    <button
                      type="button"
                      class="primary"
                      :disabled="isRunning(activeUnit.id)"
                      @click="startUnit(activeUnit)"
                    >
                      启动
                    </button>
                    <button
                      type="button"
                      class="danger"
                      :disabled="!isRunning(activeUnit.id)"
                      @click="stopUnit(activeUnit)"
                    >
                      停止
                    </button>
                    <button
                      v-if="activeUnit.kind === 'springBoot'"
                      type="button"
                      class="small"
                      :disabled="isRunning(activeUnit.id)"
                      @click="buildUnit(activeUnit)"
                    >
                      构建
                    </button>
                    <div v-if="activeUnit.kind === 'springBoot'" class="more-wrap">
                      <button
                        type="button"
                        class="small"
                        :disabled="isRunning(activeUnit.id)"
                        @click="moreOpen = !moreOpen"
                      >
                        更多 ▾
                      </button>
                      <div v-if="moreOpen" class="more-menu">
                        <button
                          type="button"
                          class="menu-item"
                          @click="installDeps(activeUnit)"
                        >
                          安装依赖
                          <span class="muted">mvn install 本地模块</span>
                        </button>
                      </div>
                    </div>
                  </div>
                  <div v-if="activeUnit.kind === 'vue'" class="unit-node-row">
                    <span class="unit-node-label" title="本模块 Node 安装目录">NODE_HOME</span>
                    <NInput
                      class="unit-node-input"
                      size="small"
                      :value="nodeHomeOf(activeUnit)"
                      :placeholder="
                        inheritedNodeHome()
                          ? `继承：${inheritedNodeHome()}`
                          : '空=项目/全局（可按模块指定版本）'
                      "
                      :disabled="isRunning(activeUnit.id)"
                      @blur="onUnitNodeHomeBlur"
                      @keydown.enter="onUnitNodeHomeBlur"
                    />
                    <NButton
                      size="small"
                      :disabled="isRunning(activeUnit.id)"
                      @click="pickPath('unitNode')"
                    >
                      浏览
                    </NButton>
                    <NButton
                      size="small"
                      :disabled="isRunning(activeUnit.id)"
                      :loading="discoverBusy"
                      title="从环境变量 / PATH / 常见目录快速查找"
                      @click="openDiscoverMenu('node', 'unitNode')"
                    >
                      查找
                    </NButton>
                    <NButton
                      size="small"
                      :disabled="isRunning(activeUnit.id)"
                      title="用本模块路径检测 Node"
                      @click="detectUnitNode"
                    >
                      检测
                    </NButton>
                    <NButton
                      v-if="nodeHomeOf(activeUnit)"
                      size="small"
                      :disabled="isRunning(activeUnit.id)"
                      title="清除模块覆盖，改用项目/全局"
                      @click="setNodeHome(activeUnit, '')"
                    >
                      清除
                    </NButton>
                    <NSelect
                      v-if="discoverMenu === 'unitNode'"
                      class="discover-select unit-discover"
                      size="small"
                      :options="discoverSelectOptions"
                      :placeholder="`选择检测到的 Node（${discoverHits.length}）…`"
                      filterable
                      @update:value="pickDiscoverHit"
                    />
                  </div>
                </div>
                <div class="log-header">
                  <div class="log-header-left">
                    <span>
                      {{ isTerminalConsole ? "终端" : "日志"
                      }}{{ activeUnit ? ` — ${activeUnit.name}` : "" }}
                    </span>
                    <span
                      v-if="!isTerminalConsole && logBufferTotal"
                      class="log-meta muted"
                    >
                      缓冲 {{ logBufferTotal }} 行
                    </span>
                    <span v-else-if="isTerminalConsole" class="log-meta muted">
                      内嵌 PTY · 可输入
                    </span>
                  </div>
                  <div class="log-header-actions">
                    <template v-if="!isTerminalConsole">
                      <label class="log-follow" title="写入时自动滚到底部">
                        <input v-model="logFollow" type="checkbox" />
                        跟随
                      </label>
                      <select
                        v-model="logLevelFilter"
                        class="log-filter-select"
                        title="按级别过滤展示"
                      >
                        <option value="all">全部</option>
                        <option value="warn">WARN+</option>
                        <option value="error">ERROR</option>
                      </select>
                      <button
                        type="button"
                        class="small ghost"
                        :disabled="!logBufferTotal"
                        @click="openFullLog"
                      >
                        完整日志
                      </button>
                    </template>
                    <button type="button" class="small ghost" @click="clearLog">清空</button>
                  </div>
                </div>
                <div
                  v-if="isTerminalConsole && activeUnit"
                  class="term-embed"
                >
                  <TerminalPane
                    :key="activeUnit.id"
                    ref="termPaneRef"
                    :unit-id="activeUnit.id"
                    :active="view === 'workspace'"
                  />
                </div>
                <div v-else class="term-embed log-xterm-wrap">
                  <LogXterm
                    v-if="activeUnit"
                    ref="logXtermRef"
                    :active="view === 'workspace' && !isTerminalConsole"
                    :follow="logFollow"
                    @follow-change="logFollow = $event"
                  />
                  <div v-else class="log-placeholder muted">选择左侧单元查看输出</div>
                </div>
              </div>
              </div>
            </div>
          </template>

          <div v-else class="scan-empty">
            <p>请在左侧选择一个项目</p>
          </div>
        </section>

        <!-- ===== GIT ===== -->
        <section v-show="view === 'git'" class="panel git-view fade-in">
          <GitPane
            v-if="activeProject"
            :root-path="activeProject.rootPath"
            :project-id="activeProject.id"
            :dark="resolvedThemeMode(ui) === 'dark'"
            @open-file="openFileFromGit"
          />
          <div v-else class="scan-empty">
            <p>请先选择项目</p>
          </div>
        </section>

        <!-- ===== SETTINGS ===== -->
        <section v-show="view === 'settings'" class="panel settings fade-in">
          <div class="settings-layout">
            <nav class="settings-nav">
              <NButton
                :type="settingsSection === 'project' ? 'primary' : 'default'"
                :secondary="settingsSection !== 'project'"
                :size="controlSize"
                block
                @click="settingsSection = 'project'"
              >
                项目
              </NButton>
              <NButton
                :type="settingsSection === 'toolchain' ? 'primary' : 'default'"
                :secondary="settingsSection !== 'toolchain'"
                :size="controlSize"
                block
                @click="settingsSection = 'toolchain'"
              >
                工具链
              </NButton>
              <NButton
                :type="settingsSection === 'appearance' ? 'primary' : 'default'"
                :secondary="settingsSection !== 'appearance'"
                :size="controlSize"
                block
                @click="settingsSection = 'appearance'"
              >
                外观
              </NButton>
              <NButton quaternary :size="controlSize" class="back-ws" @click="openWorkspace">
                ← 返回工作区
              </NButton>
            </nav>

            <div class="settings-body">
              <!-- Project -->
              <div v-show="settingsSection === 'project'" class="settings-pane">
                <template v-if="activeProject">
                  <h2>项目配置</h2>
                  <p class="hint">当前：{{ draft.name }}</p>

                  <label class="field-grid">
                    <span>名称</span>
                    <NInput v-model:value="draft.name" :size="controlSize" />
                  </label>
                  <label class="field-grid">
                    <span>根目录</span>
                    <div class="row path-row">
                      <NInput v-model:value="draft.rootPath" :size="controlSize" />
                      <NButton :size="controlSize" @click="pickPath('draftRoot')">浏览</NButton>
                    </div>
                  </label>
                  <label class="field-grid">
                    <span>扫描深度</span>
                    <NInputNumber
                      v-model:value="draft.scanDepth"
                      :min="0"
                      :max="8"
                      :size="controlSize"
                      title="0 = 默认 3"
                    />
                  </label>
                  <div class="field-grid">
                    <span>识别类型</span>
                    <div class="checks">
                      <NCheckbox v-model:checked="draft.enableSpringBoot" :size="controlSize">
                        Spring Boot
                      </NCheckbox>
                      <NCheckbox v-model:checked="draft.enableVue" :size="controlSize">
                        Vue
                      </NCheckbox>
                    </div>
                  </div>
                  <label class="field-grid">
                    <span>启动前编译</span>
                    <NSelect
                      v-model:value="draft.buildBeforeRun"
                      :options="buildBeforeRunOptions"
                      :size="controlSize"
                    />
                  </label>
                  <div class="field-grid">
                    <span>热编译</span>
                    <div class="checks switch-col">
                      <NSwitch
                        v-model:value="draft.hotReload"
                        :size="controlSize === 'small' ? 'small' : 'medium'"
                      >
                        <template #checked>保存后增量编译</template>
                        <template #unchecked>保存后增量编译</template>
                      </NSwitch>
                      <NSwitch
                        v-model:value="draft.hotReloadRestart"
                        :disabled="!draft.hotReload"
                        :size="controlSize === 'small' ? 'small' : 'medium'"
                        @update:value="onHotReloadRestartUpdate"
                      >
                        <template #checked>DevTools 上下文重启</template>
                        <template #unchecked>DevTools 上下文重启</template>
                      </NSwitch>
                    </div>
                  </div>
                  <p class="muted hint-block">
                    <strong>仅编译</strong>：磁盘变更会编进
                    <code>target/classes</code>，但已运行的 JVM
                    <em>不会</em>自动加载新字节码，改完逻辑请点
                    <strong>Restart</strong>。<strong>上下文重启</strong>才接近热部署，但对
                    growup100-api 这类应用几乎等于整站重开，一般不如手动 Restart
                    清晰。日志
                    <code>compile only, no context restart</code> 表示未触发 DevTools。
                  </p>

                  <details class="fold" :open="projectOverrideOpen">
                    <summary>覆盖工具链与 Maven 仓库（留空继承全局）</summary>
                    <label class="field-grid">
                      <span>JAVA_HOME</span>
                      <div class="path-stack">
                        <div class="row path-row">
                          <NInput
                            v-model:value="draft.javaHome"
                            placeholder="继承全局"
                            :size="controlSize"
                          />
                          <NButton :size="controlSize" @click="pickPath('draftJava')">浏览</NButton>
                          <NButton
                            :size="controlSize"
                            :loading="discoverBusy"
                            title="从环境变量 / PATH / 常见目录快速查找"
                            @click="openDiscoverMenu('java', 'draftJava')"
                          >
                            查找
                          </NButton>
                        </div>
                        <NSelect
                          v-if="discoverMenu === 'draftJava'"
                          class="discover-select"
                          :options="discoverSelectOptions"
                          :placeholder="`选择检测到的 JDK（${discoverHits.length}）…`"
                          :size="controlSize"
                          filterable
                          @update:value="pickDiscoverHit"
                        />
                      </div>
                    </label>
                    <label class="field-grid">
                      <span>MAVEN_HOME</span>
                      <div class="path-stack">
                        <div class="row path-row">
                          <NInput
                            v-model:value="draft.mavenHome"
                            placeholder="继承全局"
                            :size="controlSize"
                          />
                          <NButton :size="controlSize" @click="pickPath('draftMaven')">浏览</NButton>
                          <NButton
                            :size="controlSize"
                            :loading="discoverBusy"
                            title="从环境变量 / PATH / 常见目录快速查找"
                            @click="openDiscoverMenu('maven', 'draftMaven')"
                          >
                            查找
                          </NButton>
                        </div>
                        <NSelect
                          v-if="discoverMenu === 'draftMaven'"
                          class="discover-select"
                          :options="discoverSelectOptions"
                          :placeholder="`选择检测到的 Maven（${discoverHits.length}）…`"
                          :size="controlSize"
                          filterable
                          @update:value="pickDiscoverHit"
                        />
                      </div>
                    </label>
                    <label class="field-grid">
                      <span>NODE_HOME（默认）</span>
                      <div class="path-stack">
                        <div class="row path-row">
                          <NInput
                            v-model:value="draft.nodeHome"
                            placeholder="继承全局；Vue 模块可单独覆盖"
                            :size="controlSize"
                          />
                          <NButton :size="controlSize" @click="pickPath('draftNode')">浏览</NButton>
                          <NButton
                            :size="controlSize"
                            :loading="discoverBusy"
                            title="从环境变量 / PATH / 常见目录快速查找"
                            @click="openDiscoverMenu('node', 'draftNode')"
                          >
                            查找
                          </NButton>
                        </div>
                        <NSelect
                          v-if="discoverMenu === 'draftNode'"
                          class="discover-select"
                          :options="discoverSelectOptions"
                          :placeholder="`选择检测到的 Node（${discoverHits.length}）…`"
                          :size="controlSize"
                          filterable
                          @update:value="pickDiscoverHit"
                        />
                      </div>
                    </label>
                    <label class="field-grid">
                      <span>远程仓库 URL</span>
                      <NInput
                        v-model:value="draft.mavenRepoUrl"
                        placeholder="继承全局"
                        :size="controlSize"
                      />
                    </label>
                    <label class="field-grid">
                      <span>本地仓库</span>
                      <div class="row path-row">
                        <NInput
                          v-model:value="draft.mavenLocalRepo"
                          placeholder="继承全局"
                          :size="controlSize"
                        />
                        <NButton :size="controlSize" @click="pickPath('draftMavenLocal')">
                          浏览
                        </NButton>
                      </div>
                    </label>
                  </details>

                  <div class="actions sticky-actions">
                    <NButton type="primary" :size="controlSize" @click="saveProjectConfig">
                      保存
                    </NButton>
                    <NButton :size="controlSize" @click="detectToolchain()">检测工具链</NButton>
                    <NButton type="error" :size="controlSize" @click="removeProject(draft.id)">
                      删除项目
                    </NButton>
                  </div>
                  <div v-if="toolchain" class="detect">
                    <div :class="toolchain.javaOk ? 'ok' : 'bad'">
                      Java: {{ toolchain.javaVersion }}
                    </div>
                    <div :class="toolchain.mavenOk ? 'ok' : 'bad'">
                      Maven: {{ toolchain.mavenVersion }}
                    </div>
                    <div :class="toolchain.nodeOk ? 'ok' : 'bad'">
                      Node: {{ toolchain.nodeVersion }}
                    </div>
                  </div>
                </template>
                <div v-else class="scan-empty">
                  <p>请先在左侧选择或新建项目</p>
                  <NButton size="small" @click="settingsSection = 'toolchain'">
                    先去配置工具链
                  </NButton>
                </div>
              </div>

              <!-- Toolchain -->
              <div v-show="settingsSection === 'toolchain'" class="settings-pane">
                <h2>全局工具链</h2>
                <p class="hint">项目未单独覆盖时使用；路径可留空以使用系统 PATH。</p>

                <label class="field-grid">
                  <span>JAVA_HOME</span>
                  <div class="path-stack">
                    <div class="row path-row">
                      <NInput v-model:value="global.javaHome" :size="controlSize" />
                      <NButton :size="controlSize" @click="pickPath('globalJava')">浏览</NButton>
                      <NButton
                        :size="controlSize"
                        :loading="discoverBusy"
                        title="从环境变量 / PATH / 常见目录快速查找（不全盘扫描）"
                        @click="openDiscoverMenu('java', 'globalJava')"
                      >
                        查找
                      </NButton>
                    </div>
                    <NSelect
                      v-if="discoverMenu === 'globalJava'"
                      class="discover-select"
                      :options="discoverSelectOptions"
                      :placeholder="`选择检测到的 JDK（${discoverHits.length}）…`"
                      :size="controlSize"
                      filterable
                      @update:value="pickDiscoverHit"
                    />
                  </div>
                </label>
                <label class="field-grid">
                  <span>MAVEN_HOME</span>
                  <div class="path-stack">
                    <div class="row path-row">
                      <NInput v-model:value="global.mavenHome" :size="controlSize" />
                      <NButton :size="controlSize" @click="pickPath('globalMaven')">浏览</NButton>
                      <NButton
                        :size="controlSize"
                        :loading="discoverBusy"
                        title="从环境变量 / PATH / 常见目录快速查找（不全盘扫描）"
                        @click="openDiscoverMenu('maven', 'globalMaven')"
                      >
                        查找
                      </NButton>
                    </div>
                    <NSelect
                      v-if="discoverMenu === 'globalMaven'"
                      class="discover-select"
                      :options="discoverSelectOptions"
                      :placeholder="`选择检测到的 Maven（${discoverHits.length}）…`"
                      :size="controlSize"
                      filterable
                      @update:value="pickDiscoverHit"
                    />
                  </div>
                </label>
                <label class="field-grid">
                  <span>NODE_HOME（默认）</span>
                  <div class="path-stack">
                    <div class="row path-row">
                      <NInput
                        v-model:value="global.nodeHome"
                        placeholder="全局默认；各 Vue 模块可单独指定版本"
                        :size="controlSize"
                      />
                      <NButton :size="controlSize" @click="pickPath('globalNode')">浏览</NButton>
                      <NButton
                        :size="controlSize"
                        :loading="discoverBusy"
                        title="从环境变量 / PATH / 常见目录快速查找（不全盘扫描）"
                        @click="openDiscoverMenu('node', 'globalNode')"
                      >
                        查找
                      </NButton>
                    </div>
                    <NSelect
                      v-if="discoverMenu === 'globalNode'"
                      class="discover-select"
                      :options="discoverSelectOptions"
                      :placeholder="`选择检测到的 Node（${discoverHits.length}）…`"
                      :size="controlSize"
                      filterable
                      @update:value="pickDiscoverHit"
                    />
                  </div>
                </label>
                <p class="hint">
                  「查找」只检查环境变量、PATH（where）和常见安装目录，不会全盘搜索。优先级：<strong>模块</strong> → 项目 → 全局。
                </p>

                <h3 class="subhead">Maven 仓库</h3>
                <p class="hint">
                  填写后生成临时 settings.xml，所有 <code>mvn</code> 带 <code>-s</code>；留空用本机默认。
                </p>
                <label class="field-grid">
                  <span>远程 / Mirror URL</span>
                    <NInput
                      v-model:value="globalMavenRepoUrl"
                      placeholder="https://nexus.example.com/repository/maven-public/"
                      :size="controlSize"
                    />
                  </label>
                  <label class="field-grid">
                    <span>本地仓库</span>
                    <div class="row path-row">
                      <NInput
                        v-model:value="globalMavenLocalRepo"
                        placeholder="~/.m2/repository"
                        :size="controlSize"
                      />
                    <NButton :size="controlSize" @click="pickPath('globalMavenLocal')">浏览</NButton>
                  </div>
                </label>

                <div class="actions sticky-actions">
                  <NButton type="primary" :size="controlSize" @click="saveGlobal">保存</NButton>
                  <NButton :size="controlSize" @click="detectToolchain()">检测</NButton>
                </div>
                <div v-if="toolchain" class="detect">
                  <div :class="toolchain.javaOk ? 'ok' : 'bad'">
                    Java: {{ toolchain.javaVersion }}
                  </div>
                  <div :class="toolchain.mavenOk ? 'ok' : 'bad'">
                    Maven: {{ toolchain.mavenVersion }}
                  </div>
                  <div :class="toolchain.nodeOk ? 'ok' : 'bad'">
                    Node: {{ toolchain.nodeVersion }}
                  </div>
                </div>
              </div>

              <!-- Appearance -->
              <div v-show="settingsSection === 'appearance'" class="settings-pane">
                <h2>外观</h2>
                <p class="hint">保存在本机。主色可任意选取，同步到界面与 Naive UI 组件。</p>

                <label class="field-grid">
                  <span>主题</span>
                  <NSelect
                    v-model:value="uiTheme"
                    :options="themeSelectOptions"
                    :size="controlSize"
                  />
                </label>
                <label class="field-grid">
                  <span>字号（{{ uiFontSize }}px）</span>
                  <NSlider v-model:value="uiFontSize" :min="12" :max="20" :step="1" />
                </label>
                <div class="field-grid">
                  <span>主色</span>
                  <div class="accent-stack">
                    <div class="accent-row">
                      <button
                        v-for="(color, key) in ACCENT_COLORS"
                        :key="key"
                        type="button"
                        class="accent-swatch"
                        :class="{ active: primaryHex === color }"
                        :style="{ background: color }"
                        :title="key"
                        @click="pickAccentPresetKey(key)"
                      />
                    </div>
                    <div class="color-picker-row">
                      <NColorPicker
                        :value="primaryHex"
                        :modes="['hex']"
                        :show-alpha="false"
                        :size="controlSize"
                        style="width: 140px"
                        @update:value="onPrimaryColorUpdate"
                      />
                      <NInput
                        :value="primaryHex"
                        :size="controlSize"
                        placeholder="#14b8a6"
                        style="max-width: 140px"
                        @update:value="onPrimaryColorUpdate"
                      />
                    </div>
                  </div>
                </div>
                <label class="field-grid">
                  <span>密度</span>
                  <NSelect
                    v-model:value="uiDensity"
                    :options="densitySelectOptions"
                    :size="controlSize"
                  />
                </label>

                <div class="preview-card">
                  <strong>预览</strong>
                  <p class="muted">Naive 按钮与自定义主色即时联动。</p>
                  <div class="actions">
                    <NButton type="primary" :size="controlSize">主要</NButton>
                    <NButton :size="controlSize">次要</NButton>
                    <NButton type="error" :size="controlSize">危险</NButton>
                  </div>
                </div>

                <div class="actions sticky-actions">
                  <NButton type="primary" :size="controlSize" @click="saveAppearance">保存</NButton>
                  <NButton :size="controlSize" @click="resetAppearance">恢复默认</NButton>
                </div>
              </div>
            </div>
          </div>
        </section>
      </main>
    </div>

    <div class="toast-host" aria-live="polite">
      <Transition name="toast-pop">
        <div v-if="msg" key="ok" class="toast ok-toast" role="status">
          <span class="toast-text">{{ msg }}</span>
          <button type="button" class="toast-close" title="关闭" @click="dismissToast">×</button>
        </div>
      </Transition>
      <Transition name="toast-pop">
        <div v-if="actionError" key="err" class="toast bad-toast tip-toast" role="alert">
          <div class="toast-body">
            <div class="toast-head">
              <strong v-if="errorTip" class="toast-title">{{ errorTip.title }}</strong>
              <span class="toast-text">{{ actionError }}</span>
            </div>
            <p v-if="errorTip" class="toast-advice">{{ errorTip.advice }}</p>
            <div v-if="errorTip && errorTip.action !== 'none'" class="toast-actions">
              <NButton
                size="tiny"
                type="primary"
                @click="runTipAction(errorTip.action)"
              >
                {{ errorTip.actionLabel || "去设置" }}
              </NButton>
            </div>
          </div>
          <button type="button" class="toast-close" title="关闭" @click="dismissToast">×</button>
        </div>
      </Transition>
    </div>

    <footer class="status-bar">
      <div class="status-left">
        <span class="status-proj" :title="activeProject?.rootPath || ''">
          {{ activeProject?.name || "无项目" }}
        </span>
        <span class="status-sep">·</span>
        <span>{{ units.length }} 单元</span>
        <span class="status-sep">·</span>
        <span>{{ cacheLabel }}</span>
        <template v-if="statusFileLabel">
          <span class="status-sep">·</span>
          <span class="status-file" :title="activeOpenFile?.path || ''">{{
            statusFileLabel
          }}</span>
        </template>
      </div>
      <div class="status-mid" :title="statusTask">
        {{ statusTask || "就绪" }}
      </div>
      <div class="status-right">
        <div
          v-if="statusBusy || scanning"
          class="status-progress"
          :class="{ indeterminate: statusIndeterminate || scanning }"
        >
          <div class="status-progress-bar" />
        </div>
        <span v-else class="status-ready">Ready</span>
      </div>
    </footer>

    <SearchEverywhere
      v-if="activeProject"
      :open="searchEverywhereOpen"
      :project-id="activeProject.id"
      :unit-path="activeUnit?.path"
      :actions="searchActions"
      @close="searchEverywhereOpen = false"
      @open-file="onSearchOpenFile"
      @run-action="onSearchRunAction"
    />

    <!-- Full log dialog -->
    <div
      v-if="logFullOpen"
      class="log-modal-backdrop"
      @click.self="logFullOpen = false"
    >
      <div
        class="log-modal"
        role="dialog"
        :aria-label="isTerminalConsole ? '完整输出' : '完整日志'"
      >
        <div class="log-modal-head">
          <div>
            <strong>{{ isTerminalConsole ? "完整输出" : "完整日志" }}</strong>
            <span class="muted">
              {{ activeUnit?.name || "" }} · 缓冲内 {{ logBufferTotal }} 行（上限
              {{ LOG_BUFFER_MAX }}）
            </span>
          </div>
          <div class="log-header-actions">
            <button type="button" class="small" @click="copyFullLog">复制全部</button>
            <button type="button" class="small ghost" @click="logFullOpen = false">关闭</button>
          </div>
        </div>
        <div class="log-modal-body">
          <pre v-if="logBufferTotal" class="log-full-pre">{{ fullLogPlainText }}</pre>
          <div v-else class="log-placeholder muted">
            {{ isTerminalConsole ? "暂无输出" : "暂无日志" }}
          </div>
        </div>
      </div>
    </div>
  </div>
  </NConfigProvider>
</template>

<style>
:root {
  color-scheme: light dark;
  --bg: #e8edf2;
  --bg-grid: rgba(15, 23, 42, 0.04);
  --panel: #f7f9fc;
  --panel-elevated: #ffffff;
  --border: #c5ced9;
  --text: #0f172a;
  --muted: #64748b;
  --primary: #14b8a6;
  --primary-fg: #042f2e;
  --danger: #e11d48;
  --ok: #059669;
  --bad: #dc2626;
  --pad: 0.45rem 0.75rem;
  --pad-sm: 0.22rem 0.5rem;
  --font-size: 14px;
  --radius: 7px;
  --shadow: 0 1px 0 rgba(15, 23, 42, 0.04);
  font-family: "Segoe UI", "PingFang SC", "Microsoft YaHei", "Noto Sans SC", sans-serif;
  font-size: var(--font-size);
  color: var(--text);
  background: var(--bg);
}
:root[data-theme="light"] {
  color-scheme: light;
  --bg: #e8edf2;
  --bg-grid: rgba(15, 23, 42, 0.045);
  --panel: #f7f9fc;
  --panel-elevated: #ffffff;
  --border: #c5ced9;
  --text: #0f172a;
  --muted: #64748b;
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --bg: #0c1017;
  --bg-grid: rgba(148, 163, 184, 0.06);
  --panel: #141a22;
  --panel-elevated: #1a222d;
  --border: #2a3544;
  --text: #e8eef5;
  --muted: #8b9aab;
  --primary-fg: #042f2e;
  --shadow: 0 1px 0 rgba(0, 0, 0, 0.25);
}
:root[data-density="compact"] {
  --pad: 0.28rem 0.5rem;
  --pad-sm: 0.12rem 0.38rem;
}
* {
  box-sizing: border-box;
}
body {
  margin: 0;
  font-size: inherit;
  background: var(--bg);
  color: var(--text);
}
button,
input,
select {
  font: inherit;
}
button {
  cursor: pointer;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
  border-radius: var(--radius);
  padding: var(--pad);
  transition: border-color 0.15s, background 0.15s, opacity 0.15s;
}
button:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--primary) 55%, var(--border));
}
button.primary {
  background: var(--primary);
  border-color: var(--primary);
  color: #fff;
  font-weight: 560;
}
button.primary:hover:not(:disabled) {
  filter: brightness(1.06);
}
button.danger {
  background: var(--danger);
  border-color: var(--danger);
  color: #fff;
}
button.small {
  padding: var(--pad-sm);
  font-size: 0.85em;
}
button.ghost {
  background: transparent;
}
button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
input,
select {
  flex: 1;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  color: var(--text);
  border-radius: var(--radius);
  padding: var(--pad);
  min-width: 0;
}
input[type="range"] {
  padding: 0;
}
input:focus,
select:focus,
button:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--primary) 45%, transparent);
  outline-offset: 1px;
}
code {
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 0.9em;
  padding: 0.05em 0.3em;
  border-radius: 4px;
  background: color-mix(in srgb, var(--primary) 12%, transparent);
}
</style>

<style scoped>
.app {
  height: 100vh;
  display: flex;
  flex-direction: column;
  background-color: var(--bg);
  background-image:
    linear-gradient(var(--bg-grid) 1px, transparent 1px),
    linear-gradient(90deg, var(--bg-grid) 1px, transparent 1px);
  background-size: 24px 24px;
}
.status-bar {
  flex-shrink: 0;
  height: 28px;
  display: grid;
  grid-template-columns: minmax(0, 1.2fr) minmax(0, 1.4fr) 120px;
  align-items: center;
  gap: 0.65rem;
  padding: 0 0.75rem;
  border-top: 1px solid var(--border);
  background: color-mix(in srgb, var(--panel) 94%, #000);
  font-size: 11px;
  color: var(--muted);
  z-index: 25;
}
.status-left,
.status-mid,
.status-right {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status-left {
  display: flex;
  align-items: center;
  gap: 0.35rem;
}
.status-proj {
  color: var(--text);
  font-weight: 560;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
}
.status-sep {
  opacity: 0.55;
}
.status-mid {
  text-align: center;
  color: var(--text);
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 10.5px;
}
.status-right {
  display: flex;
  justify-content: flex-end;
  align-items: center;
}
.status-ready {
  letter-spacing: 0.06em;
  text-transform: uppercase;
  font-size: 10px;
}
.status-progress {
  width: 88px;
  height: 4px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--border) 80%, transparent);
  overflow: hidden;
}
.status-progress-bar {
  height: 100%;
  width: 40%;
  background: var(--primary);
  border-radius: 2px;
}
.status-progress.indeterminate .status-progress-bar {
  width: 35%;
  animation: status-indeterminate 1.1s ease-in-out infinite;
}
@keyframes status-indeterminate {
  0% {
    transform: translateX(-120%);
  }
  100% {
    transform: translateX(320%);
  }
}

.header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.55rem 0.9rem;
  border-bottom: 1px solid var(--border);
  background: color-mix(in srgb, var(--panel) 92%, transparent);
  backdrop-filter: blur(8px);
  z-index: 20;
}
.header.titlebar {
  height: 40px;
  padding: 0 0 0 0.65rem;
  user-select: none;
  flex-shrink: 0;
}
.titlebar-drag {
  flex: 1 1 auto;
  align-self: stretch;
  min-width: 48px;
  cursor: default;
  -webkit-app-region: drag;
  app-region: drag;
}
.brand-block {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  min-width: 0;
  flex-shrink: 0;
  -webkit-app-region: no-drag;
  app-region: no-drag;
}
.brand {
  display: inline-flex;
  align-items: center;
  gap: 0.45rem;
  border: none;
  background: transparent;
  padding: 0.2rem 0.35rem;
  font-weight: 650;
  letter-spacing: -0.02em;
  font-size: 1.02em;
}
.brand:hover {
  border-color: transparent;
}
.brand-mark {
  width: 10px;
  height: 10px;
  border-radius: 2px;
  background: var(--primary);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--primary) 28%, transparent);
}
.header-ctx {
  font-size: 0.85em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 280px;
}
.header-actions {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  flex-shrink: 0;
  height: 100%;
  -webkit-app-region: no-drag;
  app-region: no-drag;
  padding-right: 0;
}
.nav-btn.icon-btn {
  min-width: 32px;
  padding-left: 0.45rem;
  padding-right: 0.45rem;
}
.nav-btn.active {
  border-color: var(--primary);
  color: var(--primary);
  background: color-mix(in srgb, var(--primary) 12%, var(--panel-elevated));
}
.win-controls {
  display: flex;
  align-items: stretch;
  height: 100%;
  margin-left: 0.25rem;
  border-left: 1px solid var(--border);
}
.win-btn {
  width: 46px;
  height: 100%;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--text);
  font-size: 13px;
  line-height: 1;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}
.win-btn:hover {
  background: color-mix(in srgb, var(--muted) 18%, transparent);
  border-color: transparent;
}
.win-btn.win-close:hover {
  background: #e81123;
  color: #fff;
}

.body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: 232px 1fr;
  transition: grid-template-columns 0.18s ease;
}
.body.sidebar-collapsed {
  grid-template-columns: 40px 1fr;
}

.sidebar {
  border-right: 1px solid var(--border);
  background: var(--panel);
  overflow: auto;
  padding: 0.65rem;
  min-width: 0;
}
.sidebar.collapsed {
  padding: 0.4rem 0.2rem;
  overflow: hidden;
}
.side-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.35rem;
  margin-bottom: 0.55rem;
}
.sidebar.collapsed .side-head {
  justify-content: center;
  margin-bottom: 0.35rem;
}
.collapse-btn {
  flex-shrink: 0;
  width: 26px;
  height: 26px;
  padding: 0;
  display: grid;
  place-items: center;
  font-size: 14px;
  line-height: 1;
  border-radius: 5px;
  background: transparent;
  color: var(--muted);
}
.collapse-btn:hover {
  color: var(--primary);
  border-color: color-mix(in srgb, var(--primary) 40%, var(--border));
}
.side-rail {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
  padding-top: 0.25rem;
}
.rail-expand {
  writing-mode: vertical-rl;
  text-orientation: mixed;
  letter-spacing: 0.12em;
  font-size: 0.72em;
  font-weight: 650;
  text-transform: uppercase;
  color: var(--muted);
  border: none;
  background: transparent;
  padding: 0.4rem 0.15rem;
}
.rail-expand:hover {
  color: var(--primary);
  border-color: transparent;
}
.rail-name {
  writing-mode: vertical-rl;
  text-orientation: mixed;
  font-size: 0.78em;
  color: var(--primary);
  max-height: 40vh;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 600;
}
.side-label {
  flex: 1;
  font-size: 0.72em;
  font-weight: 650;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--muted);
}
.list-head .collapse-btn {
  width: 22px;
  height: 22px;
  font-size: 13px;
}
.list-head > span {
  flex: 1;
}
.create-box {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  margin-bottom: 0.65rem;
  padding: 0.5rem;
  border: 1px dashed var(--border);
  border-radius: var(--radius);
  background: var(--panel-elevated);
}
.project-item {
  width: 100%;
  text-align: left;
  margin-bottom: 0.35rem;
  padding: 0.55rem 0.6rem 0.55rem 0.7rem;
  position: relative;
  animation: rise 0.35s ease both;
  box-shadow: var(--shadow);
}
.project-item.active {
  border-color: color-mix(in srgb, var(--primary) 55%, var(--border));
  background: color-mix(in srgb, var(--primary) 10%, var(--panel-elevated));
}
.project-item.active::before {
  content: "";
  position: absolute;
  left: 0;
  top: 6px;
  bottom: 6px;
  width: 3px;
  border-radius: 0 2px 2px 0;
  background: var(--primary);
}
.pname {
  font-weight: 600;
}
.proot {
  font-size: 11px;
  color: var(--muted);
  word-break: break-all;
  margin-top: 0.15rem;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
}
.side-empty {
  padding: 1rem 0.5rem;
  color: var(--muted);
  text-align: center;
  font-size: 0.9em;
}

.main {
  min-height: 0;
  overflow: auto;
  position: relative;
}
.panel {
  padding: 0.85rem 1rem;
}
.workspace {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
}
.git-view {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  padding: 0.55rem 0.65rem;
}
.fade-in {
  animation: fade 0.22s ease;
}

.boot-overlay {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: grid;
  place-items: center;
  background: color-mix(in srgb, var(--bg) 92%, transparent);
  backdrop-filter: blur(6px);
  pointer-events: all;
}
.boot-card {
  width: min(320px, calc(100vw - 2.5rem));
  padding: 1.35rem 1.4rem 1.25rem;
  border-radius: 12px;
  border: 1px solid var(--border);
  background: var(--panel-elevated);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.18);
}
.boot-brand {
  display: flex;
  align-items: center;
  gap: 0.55rem;
  font-size: 1.05rem;
  letter-spacing: 0.02em;
}
.boot-step {
  margin: 0.85rem 0 0.65rem;
  color: var(--muted);
  font-size: 0.9em;
}
.boot-bar {
  height: 4px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--border) 80%, transparent);
  overflow: hidden;
}
.boot-bar-fill {
  height: 100%;
  border-radius: inherit;
  background: var(--primary);
  transition: width 0.22s ease;
}
.boot-fade-enter-active,
.boot-fade-leave-active {
  transition: opacity 0.22s ease;
}
.boot-fade-enter-from,
.boot-fade-leave-to {
  opacity: 0;
}

.toast-host {
  position: fixed;
  top: 52px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.4rem;
  width: min(440px, calc(100vw - 2rem));
  pointer-events: none;
}
.toast {
  pointer-events: auto;
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
  width: 100%;
  margin: 0;
  padding: 0.55rem 0.65rem 0.55rem 0.85rem;
  border-radius: 8px;
  border: 1px solid var(--border);
  font-size: 0.9em;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);
  backdrop-filter: blur(8px);
}
.toast-text {
  flex: 1;
  min-width: 0;
  line-height: 1.35;
  word-break: break-word;
}
.toast-close {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.55;
  font-size: 16px;
  line-height: 1;
  display: grid;
  place-items: center;
}
.toast-close:hover {
  opacity: 1;
  border-color: transparent;
}
.ok-toast {
  color: var(--ok);
  background: color-mix(in srgb, var(--panel-elevated) 88%, var(--ok));
  border-color: color-mix(in srgb, var(--ok) 40%, var(--border));
}
.bad-toast {
  color: var(--bad);
  background: color-mix(in srgb, var(--panel-elevated) 88%, var(--bad));
  border-color: color-mix(in srgb, var(--bad) 40%, var(--border));
}
.toast-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
}
.toast-head {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
}
.toast-title {
  font-size: 0.95em;
  color: var(--text);
}
.tip-toast .toast-text {
  color: var(--muted);
  font-size: 0.88em;
}
.toast-advice {
  margin: 0;
  color: var(--text);
  font-size: 0.88em;
  line-height: 1.4;
  opacity: 0.92;
}
.toast-actions {
  display: flex;
  gap: 0.4rem;
  margin-top: 0.15rem;
}
.toast-pop-enter-active,
.toast-pop-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.toast-pop-enter-from,
.toast-pop-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}

.hint,
.muted {
  color: var(--muted);
}
.ok {
  color: var(--ok);
}
.bad {
  color: var(--bad);
}

.guide {
  max-width: 560px;
  margin: 1.5rem auto;
  padding: 1.25rem 1.4rem;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--panel-elevated);
  box-shadow: var(--shadow);
}
.guide h2 {
  margin: 0 0 0.35rem;
  letter-spacing: -0.02em;
}
.guide-steps {
  list-style: none;
  margin: 1.25rem 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.85rem;
}
.guide-steps li {
  display: flex;
  gap: 0.75rem;
  align-items: flex-start;
}
.step-num {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  display: grid;
  place-items: center;
  font-weight: 700;
  font-size: 0.85em;
  background: color-mix(in srgb, var(--primary) 18%, transparent);
  color: var(--primary);
  flex-shrink: 0;
}
.step-body {
  flex: 1;
  min-width: 0;
}
.step-body p {
  margin: 0.2rem 0 0.45rem;
  font-size: 0.9em;
}

.workspace-bar {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 0.75rem;
  flex-wrap: wrap;
  margin-bottom: 0.65rem;
  padding-bottom: 0.65rem;
  border-bottom: 1px solid var(--border);
}
.ws-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.45rem;
  min-width: 0;
  flex: 1;
}
.ws-name {
  font-size: 1.05em;
  letter-spacing: -0.02em;
}
.ws-path {
  font-size: 0.72em;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  opacity: 0.75;
}
.chip {
  font-size: 0.72em;
  padding: 0.15rem 0.5rem;
  border-radius: 999px;
  border: 1px solid color-mix(in srgb, var(--primary) 40%, var(--border));
  background: color-mix(in srgb, var(--primary) 12%, transparent);
  color: var(--primary);
  font-weight: 560;
}
.chip-quiet {
  border-color: var(--border);
  background: transparent;
  color: var(--muted);
  font-weight: 500;
}
.chip-quiet.chip-on {
  border-color: color-mix(in srgb, var(--primary) 35%, var(--border));
  color: color-mix(in srgb, var(--primary) 75%, var(--muted));
  background: color-mix(in srgb, var(--primary) 8%, transparent);
}
.ws-tools {
  display: flex;
  gap: 0.45rem;
  align-items: center;
  flex-wrap: wrap;
}
.filter-select {
  flex: 0 1 auto;
  min-width: 120px;
}
.force-label {
  display: flex;
  align-items: center;
  gap: 0.3rem;
  font-size: 12px;
  color: var(--muted);
  white-space: nowrap;
}

.scan-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.75rem;
  color: var(--muted);
  padding: 2.5rem 1rem;
  border: 1px dashed var(--border);
  border-radius: 10px;
  background: color-mix(in srgb, var(--panel-elevated) 70%, transparent);
  margin-top: 0.5rem;
}
.scan-empty p {
  margin: 0;
}

.split {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: row;
  align-items: stretch;
  gap: 0;
  height: calc(100vh - 176px);
}
.split.modules-collapsed {
  gap: 0.65rem;
}
.h-split {
  flex: 0 0 5px;
  cursor: col-resize;
  background: color-mix(in srgb, var(--border) 80%, transparent);
  position: relative;
  z-index: 2;
  touch-action: none;
  align-self: stretch;
}
.h-split:hover,
.h-split:active {
  background: color-mix(in srgb, var(--primary) 55%, var(--border));
}

.module-list,
.log-panel,
.editor-panel {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel-elevated);
  overflow: hidden;
  display: flex;
  flex-direction: column;
  box-shadow: var(--shadow);
}
.module-list {
  overflow: hidden;
  min-width: 160px;
  max-width: 48%;
}
.module-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
}
.module-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
  padding: 1.25rem 0.75rem;
  font-size: 12px;
}
.unit-strip {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.35rem 0.45rem;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
  background: var(--panel);
}
.unit-select {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  padding: 0.2rem 0.3rem;
}
.side-panel :deep(.pft) {
  flex: 1 1 auto;
  min-height: 120px;
  border-top: 1px solid var(--border);
}
.starred-files {
  flex: 0 0 auto;
  border-top: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  background: color-mix(in srgb, var(--panel) 55%, var(--panel-elevated));
  overflow: hidden;
}
.starred-files.compact {
  min-height: 0;
  max-height: 30%;
  overflow: hidden;
}
.starred-files.compact:has(.starred-file-item),
.starred-files.compact:has(.starred-empty) {
  overflow: auto;
}
.starred-files-toggle {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  width: 100%;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--muted);
  padding: 0.28rem 0.5rem;
  font-size: 0.72em;
  letter-spacing: 0.04em;
  text-align: left;
  cursor: pointer;
  flex-shrink: 0;
}
.starred-files-toggle:hover {
  color: var(--text);
  background: color-mix(in srgb, var(--primary) 6%, transparent);
  border-color: transparent;
}
.starred-twist {
  width: 10px;
  font-size: 10px;
}
.starred-count {
  margin-left: auto;
  font-variant-numeric: tabular-nums;
}
.starred-empty {
  padding: 0.4rem 0.5rem 0.55rem;
  font-size: 11px;
  text-align: center;
}
.starred-file-item {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  width: 100%;
  border: none;
  border-radius: 0;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  background: transparent;
  color: var(--text);
  padding: 0.28rem 0.4rem 0.28rem 0.5rem;
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.starred-file-item:hover {
  background: color-mix(in srgb, var(--primary) 8%, transparent);
  border-color: color-mix(in srgb, var(--border) 55%, transparent);
}
.starred-file-item.active {
  background: color-mix(in srgb, var(--primary) 14%, transparent);
  box-shadow: inset 3px 0 0 var(--primary);
}
.starred-file-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 560;
}
.starred-files .star-btn {
  width: 20px;
  height: 20px;
  font-size: 12px;
}
.editor-tab .tab-star {
  width: 16px;
  height: 16px;
  font-size: 11px;
  padding: 0;
}
.main-stack {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 0;
  height: 100%;
}
.editor-panel {
  min-height: 80px;
  border-radius: 8px 8px 0 0;
}
.log-panel {
  min-height: 100px;
  border-radius: 0 0 8px 8px;
  flex: 1 1 auto;
}
.v-split {
  flex: 0 0 5px;
  cursor: row-resize;
  background: color-mix(in srgb, var(--border) 80%, transparent);
  position: relative;
  z-index: 2;
  touch-action: none;
}
.v-split:hover,
.v-split:active {
  background: color-mix(in srgb, var(--primary) 55%, var(--border));
}
.editor-tabs {
  display: flex;
  flex-wrap: nowrap;
  overflow-x: auto;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  flex-shrink: 0;
  min-height: 30px;
  align-items: stretch;
}
.editor-tab {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
  padding: 0.28rem 0.45rem 0.28rem 0.55rem;
  border: none;
  border-right: 1px solid var(--border);
  border-radius: 0;
  background: transparent;
  color: var(--muted);
  font-size: 12px;
  max-width: 160px;
  white-space: nowrap;
}
.editor-tab:hover {
  color: var(--text);
  background: color-mix(in srgb, var(--primary) 8%, transparent);
  border-color: var(--border);
}
.editor-tab.active {
  background: var(--panel-elevated);
  color: var(--text);
  box-shadow: inset 0 -2px 0 var(--primary);
}
.editor-tab-name {
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 560;
}
.editor-tab-close {
  opacity: 0.55;
  font-size: 14px;
  line-height: 1;
  padding: 0 2px;
}
.editor-tab-close:hover {
  opacity: 1;
  color: var(--bad);
}
.editor-tab-add {
  max-width: none;
  padding: 0.28rem 0.65rem;
  font-weight: 700;
  color: var(--primary);
}
.editor-body {
  flex: 1;
  min-height: 0;
  position: relative;
  background: #1e1e1e;
}
.editor-empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.45rem;
  background: var(--panel);
  padding: 1.5rem;
  text-align: center;
}
.editor-empty-title {
  margin: 0;
  font-size: 0.98em;
  font-weight: 600;
  color: var(--text);
}
.editor-empty-hint {
  margin: 0 0 0.55rem;
  font-size: 0.85em;
  max-width: 28rem;
}
.editor-empty-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  justify-content: center;
}
.list-head-label {
  font-weight: 500;
  opacity: 0.9;
}
.status-file {
  color: var(--text);
  max-width: 28%;
  overflow: hidden;
  text-overflow: ellipsis;
}
.list-head {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.3rem 0.45rem;
  font-size: 0.7em;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--muted);
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  position: sticky;
  top: 0;
  z-index: 1;
}
.list-search {
  padding: 0.35rem 0.45rem;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  position: sticky;
  top: 28px;
  z-index: 1;
}
.list-search input {
  width: 100%;
  padding: 0.22rem 0.45rem;
  font-size: 12px;
}
.star-btn {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--muted);
  font-size: 13px;
  line-height: 1;
  display: grid;
  place-items: center;
}
.star-btn:hover {
  color: #eab308;
  border-color: transparent;
}
.star-btn.on {
  color: #eab308;
}
.modules-rail {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.35rem;
  padding: 0.45rem 0.15rem;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel-elevated);
  color: var(--muted);
  box-shadow: var(--shadow);
  flex: 0 0 36px;
  min-width: 36px;
  width: 36px;
}
.modules-rail:hover {
  color: var(--primary);
  border-color: color-mix(in srgb, var(--primary) 45%, var(--border));
}
.modules-rail .rail-chevron {
  font-size: 14px;
  line-height: 1;
}
.modules-rail .rail-label {
  writing-mode: vertical-rl;
  text-orientation: mixed;
  letter-spacing: 0.12em;
  font-size: 0.72em;
  font-weight: 650;
  text-transform: uppercase;
}
.modules-rail .rail-count {
  font-size: 0.7em;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  color: var(--primary);
}
.module-item {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.28rem 0.5rem;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 70%, transparent);
  cursor: pointer;
  animation: rise 0.25s ease both;
  min-height: 28px;
}
.module-item:hover {
  background: color-mix(in srgb, var(--primary) 6%, transparent);
}
.module-item.selected {
  background: color-mix(in srgb, var(--primary) 12%, transparent);
  box-shadow: inset 3px 0 0 var(--primary);
}
.m-name {
  flex: 1;
  min-width: 0;
  font-size: 0.92em;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--muted);
  flex-shrink: 0;
}
.status-dot[data-status="running"],
.status-dot[data-status="starting"] {
  background: #3b82f6;
  animation: pulse 1.4s ease-in-out infinite;
}
.status-dot[data-status="failed"],
.status-dot[data-health="failed"],
.status-dot[data-health="portInUse"] {
  background: var(--bad);
  animation: none;
}
.status-dot[data-health="ready"] {
  background: var(--ok);
  animation: none;
}
.badge.hot-reload-badge {
  border-color: color-mix(in srgb, #0d9488 45%, var(--border));
  color: #0d9488;
}
.chip-on {
  border-color: color-mix(in srgb, #0d9488 45%, var(--border));
  color: #0d9488;
}
.hint-block {
  margin: 0.25rem 0 0.75rem;
  font-size: 0.85em;
  line-height: 1.45;
}
.hint-block code {
  font-size: 0.92em;
}
.badge[data-health="ready"] {
  border-color: color-mix(in srgb, var(--ok) 50%, var(--border));
  color: var(--ok);
}
.badge[data-health="failed"],
.badge[data-health="portInUse"],
.badge[data-status="failed"] {
  border-color: color-mix(in srgb, var(--bad) 50%, var(--border));
  color: var(--bad);
}
.profile-select {
  flex: 0 1 160px;
  min-width: 100px;
  max-width: 200px;
  padding: 0.15rem 0.35rem;
  font-size: 11px;
}

.kind {
  font-size: 9px;
  padding: 0.05rem 0.28rem;
  border-radius: 3px;
  background: color-mix(in srgb, var(--primary) 14%, transparent);
  color: var(--primary);
  font-weight: 560;
  flex-shrink: 0;
}
.module-item .badge {
  flex-shrink: 0;
  max-width: 5.5em;
  overflow: hidden;
  text-overflow: ellipsis;
}
.run-tabs {
  display: flex;
  flex-wrap: nowrap;
  gap: 0;
  overflow-x: auto;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  flex-shrink: 0;
}
.run-tab {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.35rem 0.65rem;
  border: none;
  border-right: 1px solid var(--border);
  border-radius: 0;
  background: transparent;
  color: var(--muted);
  font-size: 12px;
  white-space: nowrap;
  max-width: 200px;
}
.run-tab:hover {
  background: color-mix(in srgb, var(--primary) 8%, transparent);
  color: var(--text);
  border-color: var(--border);
}
.run-tab.active {
  background: var(--panel-elevated);
  color: var(--text);
  box-shadow: inset 0 -2px 0 var(--primary);
}
.run-tab-name {
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 560;
}
.run-tab-hint {
  font-size: 10px;
}
.badge {
  font-size: 9px;
  padding: 0.05rem 0.32rem;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--muted);
}
.badge[data-status="running"],
.badge[data-status="starting"] {
  color: var(--ok);
  border-color: color-mix(in srgb, var(--ok) 50%, var(--border));
}
.badge[data-status="failed"] {
  color: var(--bad);
  border-color: color-mix(in srgb, var(--bad) 50%, var(--border));
}

.action-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: wrap;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.action-meta {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  flex-wrap: wrap;
  min-width: 0;
}
.action-btns {
  display: flex;
  gap: 0.4rem;
  align-items: center;
  flex-wrap: wrap;
}
.action-btns > .primary,
.action-btns > .danger {
  min-width: 4.25rem;
  font-weight: 600;
  padding: 0.35rem 0.85rem;
}
.unit-node-row {
  flex: 1 1 100%;
  display: flex;
  align-items: center;
  gap: 0.4rem;
  min-width: 0;
  padding-top: 0.15rem;
}
.unit-node-label {
  flex: 0 0 5.5rem;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--muted);
}
.unit-node-input {
  flex: 1 1 auto;
  min-width: 0;
  font-size: 11px;
  padding: 0.2rem 0.4rem;
}
.unit-node-row .n-button {
  flex-shrink: 0;
}
.module-node-tag {
  font-size: 9px;
  color: var(--primary);
  opacity: 0.9;
  flex-shrink: 0;
}
.more-wrap {
  position: relative;
}
.more-menu {
  position: absolute;
  right: 0;
  top: calc(100% + 4px);
  min-width: 200px;
  padding: 0.3rem;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel-elevated);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  z-index: 30;
}
.menu-item {
  width: 100%;
  text-align: left;
  display: flex;
  flex-direction: column;
  gap: 0.1rem;
  border: none;
  background: transparent;
  padding: 0.45rem 0.55rem;
  border-radius: 5px;
}
.menu-item:hover {
  background: color-mix(in srgb, var(--primary) 10%, transparent);
  border-color: transparent;
}
.menu-item .muted {
  font-size: 0.78em;
}

.log-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.5rem;
  padding: 0.4rem 0.75rem;
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  color: var(--muted);
  flex-shrink: 0;
}
.log-header-left {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 0.45rem;
  min-width: 0;
}
.log-header-actions {
  display: flex;
  gap: 0.35rem;
  flex-shrink: 0;
  flex-wrap: wrap;
  justify-content: flex-end;
  align-items: center;
}
.log-follow {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
  font-size: 11px;
  color: var(--muted);
  white-space: nowrap;
  cursor: pointer;
  user-select: none;
}
.log-filter-select {
  flex: 0 0 auto;
  width: auto;
  min-width: 72px;
  padding: 0.12rem 0.3rem;
  font-size: 11px;
}
.log-xterm-wrap {
  flex: 1;
  min-height: 0;
  position: relative;
}
.log-full-pre {
  margin: 0;
  padding: 0.65rem 0.75rem;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 12px;
  line-height: 1.35;
  white-space: pre-wrap;
  word-break: break-word;
  color: #d4d4d4;
  background: #1e1e1e;
  min-height: 100%;
}
.log-meta {
  font-size: 0.92em;
}
.log-view {
  margin: 0;
  flex: 1;
  overflow: auto;
  padding: 0.45rem 0.65rem;
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 12px;
  line-height: 1.4;
  /* IDEA Darcula-like console surface (Spring logs) */
  background: #2b2b2b;
  color: #a9b7c6;
}
.term-embed {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: #1e1e1e;
}
.log-line {
  white-space: pre-wrap;
  word-break: break-word;
  color: #a9b7c6;
}
.log-line[data-level="launcher"] {
  color: #808080;
}
.log-line[data-level="error"] {
  color: #ff6b68;
}
.log-line[data-level="warn"] {
  color: #bbb529;
}
.log-line[data-level="debug"] {
  color: #808080;
}
.log-line .ansi {
  color: inherit;
}
.log-line .ansi-bold {
  font-weight: 700;
}
/* IDEA / Spring Boot ANSI palette on dark console */
.log-line .ansi-fg-black { color: #000000; }
.log-line .ansi-fg-red { color: #ff6b68; }
.log-line .ansi-fg-green { color: #6a8759; }
.log-line .ansi-fg-yellow { color: #bbb529; }
.log-line .ansi-fg-blue { color: #6897bb; }
.log-line .ansi-fg-magenta { color: #c186c0; }
.log-line .ansi-fg-cyan { color: #299999; }
.log-line .ansi-fg-white { color: #a9b7c6; }
.log-line .ansi-fg-bright-black { color: #808080; }
.log-line .ansi-fg-bright-red { color: #ff8785; }
.log-line .ansi-fg-bright-green { color: #a8c023; }
.log-line .ansi-fg-bright-yellow { color: #ffff00; }
.log-line .ansi-fg-bright-blue { color: #6fa3d0; }
.log-line .ansi-fg-bright-magenta { color: #d6a0d6; }
.log-line .ansi-fg-bright-cyan { color: #36d1d1; }
.log-line .ansi-fg-bright-white { color: #ffffff; }
.log-trim-hint,
.log-placeholder {
  color: #808080 !important;
}
.log-trim-hint {
  padding: 0.25rem 0 0.45rem;
  font-size: 11px;
  border-bottom: 1px dashed var(--border);
  margin-bottom: 0.35rem;
}
.log-placeholder {
  padding: 1rem 0;
}

.log-modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: rgba(8, 12, 18, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 1.25rem;
  backdrop-filter: blur(3px);
}
.log-modal {
  width: min(1100px, 100%);
  height: min(820px, 92vh);
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--panel-elevated);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.35);
  overflow: hidden;
}
.log-modal-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.75rem;
  flex-wrap: wrap;
  padding: 0.65rem 0.9rem;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.log-modal-head .muted {
  margin-left: 0.5rem;
  font-size: 0.85em;
}
.log-modal-body {
  flex: 1;
  min-height: 0;
}

/* Settings */
.settings {
  height: 100%;
  padding: 0;
}
.settings-layout {
  display: grid;
  grid-template-columns: 140px 1fr;
  min-height: 100%;
  height: calc(100vh - 52px);
}
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.85rem 0.65rem;
  border-right: 1px solid var(--border);
  background: var(--panel);
}
.settings-nav .n-button {
  justify-content: flex-start;
}
.back-ws {
  margin-top: auto;
}
.accent-stack {
  display: flex;
  flex-direction: column;
  gap: 0.55rem;
}
.color-picker-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: wrap;
}
.switch-col {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 0.55rem;
}
.path-row .n-input {
  flex: 1;
  min-width: 0;
}
.settings-body {
  overflow: auto;
  padding: 1rem 1.25rem 2rem;
}
.settings-pane {
  max-width: 640px;
  animation: fade 0.2s ease;
}
.settings-pane h2 {
  margin: 0 0 0.25rem;
  letter-spacing: -0.02em;
}
.subhead {
  margin: 1.25rem 0 0.35rem;
  font-size: 0.95em;
}

.field-grid {
  display: grid;
  grid-template-columns: 128px 1fr;
  gap: 0.5rem 0.75rem;
  align-items: start;
  margin-bottom: 0.75rem;
}
.field-grid > span {
  font-weight: 560;
  font-size: 0.9em;
  color: var(--muted);
  padding-top: 0.45rem;
}
.path-stack {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  min-width: 0;
  flex: 1;
}
.path-row input {
  flex: 1;
  min-width: 0;
}
.discover-select {
  width: 100%;
  font-size: 12px;
  padding: 0.3rem 0.4rem;
}
.discover-select.unit-discover {
  flex-basis: 100%;
  width: 100%;
  margin-top: 0.25rem;
}
.row,
.actions {
  display: flex;
  gap: 0.45rem;
  align-items: center;
  flex-wrap: wrap;
}
.checks {
  display: flex;
  gap: 1rem;
  flex-wrap: wrap;
}
.fold {
  margin: 0.75rem 0 1rem;
  padding: 0.65rem 0.75rem;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
}
.fold summary {
  cursor: pointer;
  font-weight: 560;
  color: var(--muted);
  margin-bottom: 0.35rem;
}
.fold[open] summary {
  margin-bottom: 0.75rem;
  color: var(--text);
}
.sticky-actions {
  margin-top: 0.5rem;
  padding-top: 0.75rem;
  border-top: 1px solid var(--border);
}
.accent-row {
  display: flex;
  gap: 0.45rem;
  flex-wrap: wrap;
}
.accent-swatch {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  padding: 0;
  border: 2px solid transparent;
  box-shadow: 0 0 0 1px var(--border);
}
.accent-swatch.active {
  border-color: var(--text);
}
.preview-card {
  margin: 1rem 0;
  padding: 0.85rem;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
}
.detect {
  margin-top: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  padding: 0.75rem;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
  font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
  font-size: 0.85em;
}

@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@keyframes fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}
@keyframes pulse {
  0%,
  100% {
    opacity: 1;
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--ok) 45%, transparent);
  }
  50% {
    opacity: 0.7;
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--ok) 0%, transparent);
  }
}

@media (max-width: 900px) {
  .body {
    grid-template-columns: 200px 1fr;
  }
  .body.sidebar-collapsed {
    grid-template-columns: 40px 1fr;
  }
  .split {
    flex-direction: column;
    height: auto;
  }
  .split.modules-collapsed {
    flex-direction: column;
  }
  .h-split {
    display: none;
  }
  .module-list {
    flex: 0 0 auto !important;
    max-width: none;
    width: 100%;
  }
  .modules-rail {
    flex-direction: row;
    writing-mode: horizontal-tb;
    padding: 0.35rem 0.6rem;
  }
  .modules-rail .rail-label {
    writing-mode: horizontal-tb;
    letter-spacing: 0.06em;
  }
  .module-list {
    max-height: 220px;
  }
  .log-panel {
    min-height: 280px;
  }
  .field-grid {
    grid-template-columns: 1fr;
  }
  .settings-layout {
    grid-template-columns: 1fr;
    height: auto;
  }
  .settings-nav {
    flex-direction: row;
    flex-wrap: wrap;
    border-right: none;
    border-bottom: 1px solid var(--border);
  }
  .back-ws {
    margin-top: 0;
    margin-left: auto;
  }
}
</style>
