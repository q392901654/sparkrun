import * as vscode from "vscode";
import type { Launchable, ModuleStatus } from "./types";

export class UnitItem extends vscode.TreeItem {
  constructor(
    public readonly unit: Launchable,
    public status: ModuleStatus
  ) {
    super(unit.name, vscode.TreeItemCollapsibleState.None);
    this.id = unit.id;
    this.description = `${unit.kind} · ${unit.relativePath}`;
    this.tooltip = unit.detail || unit.path;
    this.contextValue = `${unit.kind}.${status}`;
    this.iconPath = new vscode.ThemeIcon(iconFor(unit.kind, status));
    this.command = {
      command: "sparkrun.openLog",
      title: "Open Log",
      arguments: [this],
    };
  }
}

function iconFor(kind: string, status: ModuleStatus): string {
  if (status === "running" || status === "starting") {
    return "debug-start";
  }
  if (status === "failed") {
    return "error";
  }
  return kind === "vue" ? "symbol-misc" : "server-process";
}

export class UnitTreeProvider implements vscode.TreeDataProvider<UnitItem> {
  private _onDidChange = new vscode.EventEmitter<UnitItem | undefined | null | void>();
  readonly onDidChangeTreeData = this._onDidChange.event;

  private units: Launchable[] = [];
  private status = new Map<string, ModuleStatus>();

  refresh(): void {
    this._onDidChange.fire();
  }

  setUnits(units: Launchable[]): void {
    this.units = units;
    this.refresh();
  }

  getUnits(): Launchable[] {
    return this.units;
  }

  findUnit(id: string): Launchable | undefined {
    return this.units.find((u) => u.id === id);
  }

  setStatus(unitId: string, status: ModuleStatus): void {
    this.status.set(unitId, status);
    this.refresh();
  }

  getStatus(unitId: string): ModuleStatus {
    return this.status.get(unitId) || "idle";
  }

  getTreeItem(element: UnitItem): vscode.TreeItem {
    return element;
  }

  getChildren(): UnitItem[] {
    return this.units.map((u) => new UnitItem(u, this.getStatus(u.id)));
  }
}
