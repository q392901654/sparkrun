import { ChildProcessWithoutNullStreams, spawn } from "child_process";
import { EventEmitter } from "events";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

export type SidecarEventHandler = (event: string, payload: unknown) => void;

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

export class SidecarClient extends EventEmitter {
  private proc: ChildProcessWithoutNullStreams | null = null;
  private buf = "";
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private restarting = false;
  private disposed = false;

  constructor(
    private readonly sidecarPath: string,
    private readonly dataDir: string,
    private readonly onEvent: SidecarEventHandler
  ) {
    super();
  }

  start(): void {
    if (this.proc || this.disposed) {
      return;
    }
    if (!fs.existsSync(this.sidecarPath)) {
      throw new Error(`sidecar not found: ${this.sidecarPath}`);
    }
    fs.mkdirSync(this.dataDir, { recursive: true });
    this.proc = spawn(this.sidecarPath, ["--data-dir", this.dataDir], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.proc.stdout.setEncoding("utf8");
    this.proc.stderr.setEncoding("utf8");
    this.proc.stdout.on("data", (chunk: string) => this.onStdout(chunk));
    this.proc.stderr.on("data", (chunk: string) => {
      console.error(`[sparkrun-sidecar] ${chunk}`);
    });
    this.proc.on("exit", (code, signal) => {
      this.proc = null;
      for (const [, p] of this.pending) {
        p.reject(new Error(`sidecar exited (${code ?? signal})`));
      }
      this.pending.clear();
      this.emit("exit", code, signal);
      if (!this.disposed && !this.restarting) {
        this.restarting = true;
        setTimeout(() => {
          this.restarting = false;
          try {
            this.start();
            this.emit("restarted");
          } catch (e) {
            console.error("sidecar restart failed", e);
          }
        }, 800);
      }
    });
  }

  async request<T = unknown>(method: string, params?: unknown): Promise<T> {
    if (!this.proc?.stdin.writable) {
      this.start();
    }
    const id = this.nextId++;
    const line = JSON.stringify({ id, method, params: params ?? {} }) + "\n";
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (v) => resolve(v as T),
        reject,
      });
      this.proc!.stdin.write(line, (err) => {
        if (err) {
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  async shutdown(): Promise<void> {
    this.disposed = true;
    if (!this.proc) {
      return;
    }
    try {
      await this.request("shutdown", {});
    } catch {
      /* ignore */
    }
    this.proc.kill();
    this.proc = null;
  }

  private onStdout(chunk: string): void {
    this.buf += chunk;
    let idx: number;
    while ((idx = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, idx).trim();
      this.buf = this.buf.slice(idx + 1);
      if (!line) {
        continue;
      }
      let msg: Record<string, unknown>;
      try {
        msg = JSON.parse(line) as Record<string, unknown>;
      } catch {
        continue;
      }
      if (typeof msg.event === "string") {
        this.onEvent(msg.event, msg.payload);
        continue;
      }
      const id = msg.id as number | undefined;
      if (id == null) {
        continue;
      }
      const p = this.pending.get(id);
      if (!p) {
        continue;
      }
      this.pending.delete(id);
      if (msg.ok) {
        p.resolve(msg.result);
      } else {
        p.reject(new Error(String(msg.error ?? "unknown error")));
      }
    }
  }
}

export function resolveSidecarPath(context: vscode.ExtensionContext): string {
  const cfg = vscode.workspace.getConfiguration("sparkrun");
  const configured = (cfg.get<string>("sidecarPath") || "").trim();
  if (configured && fs.existsSync(configured)) {
    return configured;
  }
  const exe = process.platform === "win32" ? "sparkrun-sidecar.exe" : "sparkrun-sidecar";
  const bundled = path.join(context.extensionPath, "bin", exe);
  if (fs.existsSync(bundled)) {
    return bundled;
  }
  // Dev: monorepo target/debug
  const ws = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const candidates = [
    path.join(context.extensionPath, "..", "..", "target", "debug", exe),
    path.join(context.extensionPath, "..", "..", "target", "release", exe),
  ];
  if (ws) {
    candidates.push(path.join(ws, "target", "debug", exe), path.join(ws, "target", "release", exe));
  }
  for (const c of candidates) {
    if (fs.existsSync(c)) {
      return c;
    }
  }
  return bundled;
}
