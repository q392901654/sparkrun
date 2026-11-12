import * as crypto from "crypto";
import * as path from "path";
import * as vscode from "vscode";
import type { BuildBeforeRun, Project, RuntimeEnv } from "./types";

/** Stable project id from absolute folder path. */
export function projectIdForRoot(rootPath: string): string {
  const norm = rootPath.replace(/\\/g, "/").toLowerCase();
  const hash = crypto.createHash("sha1").update(norm).digest("hex").slice(0, 12);
  return `proj_${hash}`;
}

export function activeFolder(
  context: vscode.ExtensionContext
): vscode.WorkspaceFolder | undefined {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders?.length) {
    return undefined;
  }
  const saved = context.workspaceState.get<string>("sparkrun.activeFolder");
  if (saved) {
    const hit = folders.find((f) => f.uri.fsPath === saved);
    if (hit) {
      return hit;
    }
  }
  return folders[0];
}

export async function pickFolder(context: vscode.ExtensionContext): Promise<vscode.WorkspaceFolder | undefined> {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders?.length) {
    vscode.window.showWarningMessage("SparkRun: open a workspace folder first");
    return undefined;
  }
  if (folders.length === 1) {
    await context.workspaceState.update("sparkrun.activeFolder", folders[0].uri.fsPath);
    return folders[0];
  }
  const picked = await vscode.window.showQuickPick(
    folders.map((f) => ({ label: f.name, description: f.uri.fsPath, folder: f })),
    { placeHolder: "Select SparkRun workspace folder" }
  );
  if (picked) {
    await context.workspaceState.update("sparkrun.activeFolder", picked.folder.uri.fsPath);
    return picked.folder;
  }
  return activeFolder(context);
}

export function projectFromSettings(
  folder: vscode.WorkspaceFolder,
  cfg: vscode.WorkspaceConfiguration
): Project {
  return {
    id: projectIdForRoot(folder.uri.fsPath),
    name: folder.name,
    rootPath: folder.uri.fsPath,
    javaHome: cfg.get<string>("javaHome") || "",
    mavenHome: cfg.get<string>("mavenHome") || "",
    nodeHome: cfg.get<string>("nodeHome") || "",
    mavenRepoUrl: cfg.get<string>("mavenRepoUrl") || "",
    mavenLocalRepo: cfg.get<string>("mavenLocalRepo") || "",
    scanDepth: cfg.get<number>("scanDepth") ?? 0,
    enableSpringBoot: cfg.get<boolean>("enableSpringBoot") ?? true,
    enableVue: cfg.get<boolean>("enableVue") ?? true,
    buildBeforeRun: (cfg.get<string>("buildBeforeRun") as BuildBeforeRun) || "smart",
    hotReload: cfg.get<boolean>("hotReload") ?? false,
    hotReloadRestart: cfg.get<boolean>("hotReloadRestart") ?? false,
  };
}

export function runtimeEnvFrom(project: Project): RuntimeEnv {
  return {
    javaHome: project.javaHome,
    mavenHome: project.mavenHome,
    nodeHome: project.nodeHome,
    mavenRepoUrl: project.mavenRepoUrl,
    mavenLocalRepo: project.mavenLocalRepo,
    workspacePath: project.rootPath,
  };
}

export function unitMatchesFolder(unitPath: string, folder: vscode.WorkspaceFolder): boolean {
  const root = folder.uri.fsPath.replace(/\\/g, "/").toLowerCase();
  const p = unitPath.replace(/\\/g, "/").toLowerCase();
  return p === root || p.startsWith(root + "/");
}

export function displayName(folder: vscode.WorkspaceFolder): string {
  return `${folder.name} (${path.basename(folder.uri.fsPath)})`;
}
