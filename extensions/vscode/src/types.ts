export type ProjectKind = "springBoot" | "vue";
export type BuildBeforeRun = "smart" | "always" | "never";
export type ModuleStatus = "idle" | "starting" | "running" | "stopped" | "failed";

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
  hotReload?: boolean;
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
  springProfiles: string[];
  selectedSpringProfile: string;
}

export interface RuntimeEnv {
  javaHome: string;
  mavenHome: string;
  nodeHome: string;
  mavenRepoUrl: string;
  mavenLocalRepo: string;
  workspacePath: string;
}

export interface ModuleRuntime {
  unitId: string;
  status: ModuleStatus;
  pid?: number | null;
}

export interface LogEvent {
  unitId: string;
  stream: string;
  line: string;
}

export interface StatusEvent {
  unitId: string;
  status: ModuleStatus;
  pid?: number | null;
  message?: string | null;
}

export interface TermDataEvent {
  unitId: string;
  data: string;
}

export interface ToolchainInfo {
  javaOk: boolean;
  javaVersion: string;
  mavenOk: boolean;
  mavenVersion: string;
  nodeOk: boolean;
  nodeVersion: string;
}
