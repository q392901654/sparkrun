export type ProjectKind = "springBoot" | "vue";

export type UiTheme = "system" | "light" | "dark";
export type UiAccent = "blue" | "teal" | "green" | "orange" | "purple" | "rose";
export type UiDensity = "comfortable" | "compact";

export interface UiPreferences {
  theme: UiTheme;
  fontSize: number;
  /** Preset swatch id (shortcut); `primaryColor` wins when set to a valid hex. */
  accent: UiAccent;
  /** Arbitrary primary color, e.g. `#14b8a6`. */
  primaryColor?: string;
  density: UiDensity;
}

export interface GlobalSettings {
  javaHome: string;
  mavenHome: string;
  nodeHome: string;
  /** Remote Maven repository / mirror URL (e.g. Nexus). Empty = Maven default */
  mavenRepoUrl: string;
  /** Custom local repository path. Empty = ~/.m2/repository */
  mavenLocalRepo: string;
  activeProjectId: string | null;
  workspacePath?: string;
  ui?: UiPreferences;
}

export type BuildBeforeRun = "smart" | "always" | "never";

export interface Project {
  id: string;
  name: string;
  rootPath: string;
  javaHome: string;
  mavenHome: string;
  nodeHome: string;
  mavenRepoUrl: string;
  mavenLocalRepo: string;
  scanDepth: number;
  enableSpringBoot: boolean;
  enableVue: boolean;
  buildBeforeRun: BuildBeforeRun;
  /** Watch + incremental compile on save (does not restart JVM by itself) */
  hotReload?: boolean;
  /** After compile, DevTools ApplicationContext restart (slow; near full reboot) */
  hotReloadRestart?: boolean;
}

export interface Launchable {
  id: string;
  kind: ProjectKind;
  name: string;
  path: string;
  relativePath: string;
  detail: string | null;
  packageManager: string | null;
  startScript: string | null;
  /** Detected Spring profiles; "application" = base config */
  springProfiles: string[];
  selectedSpringProfile: string;
}

export interface ScanCache {
  projectId: string;
  rootPath: string;
  scannedAt: number;
  units: Launchable[];
  activeUnitId: string | null;
  profileByUnit: Record<string, string>;
  /** Per-unit NODE_HOME; empty/missing inherits project then global */
  nodeHomeByUnit?: Record<string, string>;
  /** Starred unit ids (pin-to-top); kept across rescan when unit still exists */
  starredUnitIds?: string[];
  /** Per-unit starred file paths (quick open configs); unitId → absolute paths */
  starredFilesByUnit?: Record<string, string[]>;
}

/** Frontend run health (derived from logs + ModuleStatus). */
export type RunHealth = "none" | "ready" | "failed" | "portInUse";

/** Boot log display filter. */
export type LogLevelFilter = "all" | "warn" | "error";

export interface ScanProgress {
  projectId: string;
  phase: string;
  current: number;
  total: number | null;
  found: number;
  message: string;
}

export interface ToolchainInfo {
  javaOk: boolean;
  javaVersion: string;
  mavenOk: boolean;
  mavenVersion: string;
  nodeOk: boolean;
  nodeVersion: string;
}

export type ModuleStatus = "idle" | "starting" | "running" | "stopped" | "failed";

export interface LogEvent {
  unitId: string;
  stream: string;
  line: string;
}

export interface StatusEvent {
  unitId: string;
  status: ModuleStatus;
  pid: number | null;
  message: string | null;
}

export const DEFAULT_UI: UiPreferences = {
  theme: "system",
  fontSize: 14,
  accent: "teal",
  primaryColor: "#14b8a6",
  density: "comfortable",
};

export const ACCENT_COLORS: Record<UiAccent, string> = {
  blue: "#3b82f6",
  teal: "#14b8a6",
  green: "#22c55e",
  orange: "#f59e0b",
  purple: "#a78bfa",
  rose: "#fb7185",
};

/** Normalize / resolve the effective primary hex for CSS + Naive themeOverrides. */
export function resolvePrimaryColor(ui: UiPreferences): string {
  const raw = (ui.primaryColor || "").trim();
  if (/^#([0-9a-fA-F]{6})$/.test(raw)) return raw.toLowerCase();
  if (/^#([0-9a-fA-F]{3})$/.test(raw)) {
    const h = raw.toLowerCase();
    return `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`;
  }
  return ACCENT_COLORS[ui.accent] || ACCENT_COLORS.teal;
}

function clampByte(n: number): number {
  return Math.min(255, Math.max(0, Math.round(n)));
}

/** Lighten (positive) / darken (negative) a #rrggbb color for hover/pressed tokens. */
export function adjustHex(hex: string, delta: number): string {
  let h = hex.trim().toLowerCase();
  if (/^#([0-9a-f]{3})$/.test(h)) {
    h = `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`;
  }
  if (!/^#([0-9a-f]{6})$/.test(h)) return hex;
  const n = parseInt(h.slice(1), 16);
  const r = clampByte((n >> 16) + delta);
  const g = clampByte(((n >> 8) & 0xff) + delta);
  const b = clampByte((n & 0xff) + delta);
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

export function resolvedThemeMode(ui: UiPreferences): "light" | "dark" {
  if (ui.theme === "light") return "light";
  if (ui.theme === "dark") return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Naive UI common primary tokens derived from current primaryColor. */
export function naivePrimaryCommon(ui: UiPreferences): Record<string, string> {
  const primary = resolvePrimaryColor(ui);
  return {
    primaryColor: primary,
    primaryColorHover: adjustHex(primary, 22),
    primaryColorPressed: adjustHex(primary, -28),
    primaryColorSuppl: primary,
  };
}

export function buildStrategyLabel(mode: BuildBeforeRun | string | undefined): string {
  switch (mode) {
    case "always":
      return "总是编译";
    case "never":
      return "从不编译";
    case "smart":
    default:
      return "智能编译";
  }
}

export function kindLabel(kind: ProjectKind): string {
  switch (kind) {
    case "springBoot":
      return "Spring Boot";
    case "vue":
      return "Vue";
    default:
      return kind;
  }
}

/** —— SparkRun Git pane DTOs (mirror Rust git.rs) —— */

export interface GitRepoSummary {
  isRepo: boolean;
  toplevel: string | null;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  dirty: boolean;
  mergeInProgress: boolean;
  rebaseInProgress: boolean;
  conflictCount: number;
  mergingInto: string | null;
  error: string | null;
}

export interface GitStatusEntry {
  path: string;
  origPath: string | null;
  status: string;
  staged: boolean;
  unstaged: boolean;
  untracked: boolean;
  unmerged: boolean;
}

export interface GitBranchInfo {
  name: string;
  isCurrent: boolean;
  isRemote: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
}

export interface GitCommitInfo {
  hash: string;
  shortHash: string;
  subject: string;
  author: string;
  relativeTime: string;
}

export interface GitStashEntry {
  index: number;
  message: string;
}

export interface GitConflictSides {
  path: string;
  base: string | null;
  ours: string | null;
  theirs: string | null;
  worktree: string | null;
  hasStage1: boolean;
  hasStage2: boolean;
  hasStage3: boolean;
}

export interface GitOpResult {
  ok: boolean;
  stdout: string;
  stderr: string;
  conflicts: boolean;
  conflictPaths: string[];
}

/** Apply UI prefs to documentElement CSS variables (custom chrome / Monaco shell). */
export function applyUiPreferences(ui: UiPreferences) {
  const root = document.documentElement;
  const fontSize = Math.min(20, Math.max(12, ui.fontSize || 14));
  root.style.setProperty("--font-size", `${fontSize}px`);
  root.style.fontSize = `${fontSize}px`;

  root.style.setProperty("--primary", resolvePrimaryColor(ui));
  root.setAttribute("data-theme", resolvedThemeMode(ui));
  root.setAttribute("data-density", ui.density === "compact" ? "compact" : "comfortable");
}
