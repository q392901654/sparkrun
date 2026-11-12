import * as vscode from "vscode";
import { LogHub } from "./logHub";
import {
  activeFolder,
  pickFolder,
  projectFromSettings,
  runtimeEnvFrom,
} from "./projectModel";
import { resolveSidecarPath, SidecarClient } from "./sidecarClient";
import type {
  Launchable,
  LogEvent,
  ModuleRuntime,
  StatusEvent,
  TermDataEvent,
  ToolchainInfo,
} from "./types";
import { UnitItem, UnitTreeProvider } from "./unitTree";

let client: SidecarClient | undefined;
let logs: LogHub | undefined;
let tree: UnitTreeProvider | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const sidecarPath = resolveSidecarPath(context);
  const dataDir = context.globalStorageUri.fsPath;
  tree = new UnitTreeProvider();
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("sparkrun.units", tree)
  );

  client = new SidecarClient(sidecarPath, dataDir, (event, payload) => {
    handleEvent(event, payload);
  });
  logs = new LogHub(client);
  context.subscriptions.push({ dispose: () => logs?.dispose() });

  try {
    client.start();
  } catch (e) {
    void vscode.window.showErrorMessage(
      `SparkRun: failed to start sidecar (${sidecarPath}): ${e}`
    );
  }

  client.on("restarted", () => {
    void refreshRunning();
    void vscode.window.showWarningMessage(
      "SparkRun sidecar restarted. Running unit handles were lost — stop orphan JVM/node if needed."
    );
  });

  const cfg = () => vscode.workspace.getConfiguration("sparkrun");

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const register = (id: string, fn: (...args: any[]) => any) => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };

  register("sparkrun.scan", async () => {
    const folder = activeFolder(context) ?? (await pickFolder(context));
    if (!folder || !client || !tree) {
      return;
    }
    const project = projectFromSettings(folder, cfg());
    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `SparkRun: scanning ${folder.name}…`,
        cancellable: false,
      },
      async () => {
        try {
          const units = await client!.request<Launchable[]>("scan", { project });
          tree!.setUnits(units || []);
          void vscode.window.showInformationMessage(
            `SparkRun: found ${(units || []).length} unit(s)`
          );
        } catch (e) {
          void vscode.window.showErrorMessage(`SparkRun scan failed: ${e}`);
        }
      }
    );
  });

  register("sparkrun.start", async (item?: UnitItem) => {
    const unit = await resolveUnit(item);
    if (!unit || !client || !tree) {
      return;
    }
    const folder = activeFolder(context);
    if (!folder) {
      return;
    }
    const project = projectFromSettings(folder, cfg());
    const env = runtimeEnvFrom(project);
    const hotReload = !!(project.hotReload && project.hotReloadRestart);
    try {
      if (unit.kind === "vue") {
        logs?.ensurePty(unit.id, unit.name);
      } else {
        logs?.showLog(unit.id, unit.name);
      }
      tree.setStatus(unit.id, "starting");
      await client.request("start", {
        env,
        unit,
        buildBeforeRun: project.buildBeforeRun,
        springProfile: unit.selectedSpringProfile || null,
        hotReload,
      });
    } catch (e) {
      tree.setStatus(unit.id, "failed");
      void vscode.window.showErrorMessage(`SparkRun start failed: ${e}`);
    }
  });

  register("sparkrun.stop", async (item?: UnitItem) => {
    const unit = await resolveUnit(item);
    if (!unit || !client) {
      return;
    }
    try {
      await client.request("stop", { unitId: unit.id });
      tree?.setStatus(unit.id, "stopped");
    } catch (e) {
      void vscode.window.showErrorMessage(`SparkRun stop failed: ${e}`);
    }
  });

  register("sparkrun.refreshRunning", async () => {
    await refreshRunning();
  });

  register("sparkrun.openLog", async (item?: UnitItem) => {
    const unit = await resolveUnit(item);
    if (!unit || !logs) {
      return;
    }
    if (unit.kind === "vue") {
      logs.ensurePty(unit.id, unit.name);
    } else {
      logs.showLog(unit.id, unit.name);
    }
  });

  register("sparkrun.detectToolchain", async () => {
    const folder = activeFolder(context);
    if (!folder || !client) {
      return;
    }
    const project = projectFromSettings(folder, cfg());
    const env = runtimeEnvFrom(project);
    try {
      const info = await client.request<ToolchainInfo>("detectToolchain", { env });
      void vscode.window.showInformationMessage(
        `SparkRun toolchain — Java: ${info.javaOk ? info.javaVersion : "NO"} · Maven: ${
          info.mavenOk ? info.mavenVersion : "NO"
        } · Node: ${info.nodeOk ? info.nodeVersion : "NO"}`
      );
    } catch (e) {
      void vscode.window.showErrorMessage(`SparkRun detect failed: ${e}`);
    }
  });

  register("sparkrun.build", async (item?: UnitItem) => {
    const unit = await resolveUnit(item);
    if (!unit || unit.kind !== "springBoot" || !client) {
      return;
    }
    const folder = activeFolder(context);
    if (!folder) {
      return;
    }
    const project = projectFromSettings(folder, cfg());
    logs?.showLog(unit.id, unit.name);
    try {
      await client.request("build", {
        env: runtimeEnvFrom(project),
        unit,
        force: false,
      });
      void vscode.window.showInformationMessage(`SparkRun: build finished · ${unit.name}`);
    } catch (e) {
      void vscode.window.showErrorMessage(`SparkRun build failed: ${e}`);
    }
  });

  register("sparkrun.installLocalDeps", async (item?: UnitItem) => {
    const unit = await resolveUnit(item);
    if (!unit || unit.kind !== "springBoot" || !client) {
      return;
    }
    const folder = activeFolder(context);
    if (!folder) {
      return;
    }
    const project = projectFromSettings(folder, cfg());
    logs?.showLog(unit.id, unit.name);
    try {
      await client.request("installLocalDeps", {
        env: runtimeEnvFrom(project),
        unit,
      });
      void vscode.window.showInformationMessage(`SparkRun: install deps finished · ${unit.name}`);
    } catch (e) {
      void vscode.window.showErrorMessage(`SparkRun install failed: ${e}`);
    }
  });

  register("sparkrun.selectWorkspaceFolder", async () => {
    await pickFolder(context);
    void vscode.commands.executeCommand("sparkrun.scan");
  });

  // Hot reload on save (M4)
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument(async (doc) => {
      if (!cfg().get<boolean>("hotReload")) {
        return;
      }
      const folder = activeFolder(context);
      if (!folder || !client || !tree) {
        return;
      }
      const project = projectFromSettings(folder, cfg());
      if (!project.hotReload) {
        return;
      }
      const file = doc.uri.fsPath;
      const unit = findRunningBootUnitForFile(file);
      if (!unit) {
        return;
      }
      try {
        await client.request("hotReloadCompile", {
          env: runtimeEnvFrom(project),
          unit,
          projectRoot: project.rootPath,
          changedPath: file,
          restart: !!project.hotReloadRestart,
        });
      } catch (e) {
        void vscode.window.setStatusBarMessage(`SparkRun hot-reload failed: ${e}`, 5000);
      }
    })
  );

  // Auto-scan first folder on activate
  if (vscode.workspace.workspaceFolders?.length) {
    void vscode.commands.executeCommand("sparkrun.scan");
  }
}

export async function deactivate(): Promise<void> {
  const stop = vscode.workspace
    .getConfiguration("sparkrun")
    .get<boolean>("stopUnitsOnDeactivate");
  if (client && stop !== false) {
    try {
      const running = await client.request<ModuleRuntime[]>("listRunning", {});
      for (const r of running || []) {
        try {
          await client.request("stop", { unitId: r.unitId });
        } catch {
          /* ignore */
        }
      }
    } catch {
      /* ignore */
    }
  }
  await client?.shutdown();
  client = undefined;
}

function handleEvent(event: string, payload: unknown): void {
  if (event === "log") {
    const ev = payload as LogEvent;
    const unit = tree?.findUnit(ev.unitId);
    logs?.appendLog(ev.unitId, unit?.name || ev.unitId, ev.stream, ev.line);
    return;
  }
  if (event === "status") {
    const ev = payload as StatusEvent;
    tree?.setStatus(ev.unitId, ev.status);
    return;
  }
  if (event === "term") {
    logs?.onTermData(payload as TermDataEvent);
    return;
  }
  if (event === "scanProgress") {
    // optional status bar — keep quiet for now
    return;
  }
  if (event === "hotReloadFsChange") {
    // external IDE changes — extension save handler covers VS Code edits
    return;
  }
}

async function refreshRunning(): Promise<void> {
  if (!client || !tree) {
    return;
  }
  try {
    const running = await client.request<ModuleRuntime[]>("listRunning", {});
    const ids = new Set((running || []).map((r) => r.unitId));
    for (const u of tree.getUnits()) {
      const hit = (running || []).find((r) => r.unitId === u.id);
      tree.setStatus(u.id, hit?.status || (ids.has(u.id) ? "running" : "idle"));
    }
  } catch (e) {
    void vscode.window.showErrorMessage(`SparkRun refresh failed: ${e}`);
  }
}

async function resolveUnit(item?: UnitItem): Promise<Launchable | undefined> {
  if (item?.unit) {
    return item.unit;
  }
  const units = tree?.getUnits() || [];
  if (!units.length) {
    void vscode.window.showWarningMessage("SparkRun: scan the workspace first");
    return undefined;
  }
  const picked = await vscode.window.showQuickPick(
    units.map((u) => ({
      label: u.name,
      description: `${u.kind} · ${u.relativePath}`,
      unit: u,
    })),
    { placeHolder: "Select unit" }
  );
  return picked?.unit;
}

function findRunningBootUnitForFile(file: string): Launchable | undefined {
  if (!tree) {
    return undefined;
  }
  const norm = file.replace(/\\/g, "/").toLowerCase();
  const candidates = tree
    .getUnits()
    .filter((u) => u.kind === "springBoot")
    .filter((u) => tree!.getStatus(u.id) === "running" || tree!.getStatus(u.id) === "starting")
    .filter((u) => {
      const root = u.path.replace(/\\/g, "/").toLowerCase();
      return norm === root || norm.startsWith(root + "/");
    })
    .sort((a, b) => b.path.length - a.path.length);
  return candidates[0];
}
