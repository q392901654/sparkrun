import * as vscode from "vscode";
import type { SidecarClient } from "./sidecarClient";
import type { TermDataEvent } from "./types";

export class LogHub {
  private channels = new Map<string, vscode.OutputChannel>();
  private termWriteEmitters = new Map<string, vscode.EventEmitter<string>>();
  private terminals = new Map<string, vscode.Terminal>();

  constructor(private readonly client: SidecarClient) {}

  appendLog(unitId: string, name: string, stream: string, line: string): void {
    const ch = this.channel(unitId, name);
    const prefix = stream === "stderr" ? "[err] " : "";
    ch.appendLine(`${prefix}${line}`);
  }

  showLog(unitId: string, name: string): void {
    this.channel(unitId, name).show(true);
  }

  /** Vue / PTY: ensure a Pseudoterminal is open and feed base64 chunks. */
  ensurePty(unitId: string, name: string): void {
    if (this.terminals.has(unitId)) {
      this.terminals.get(unitId)!.show(true);
      return;
    }
    const writeEmitter = new vscode.EventEmitter<string>();
    this.termWriteEmitters.set(unitId, writeEmitter);
    const pty: vscode.Pseudoterminal = {
      onDidWrite: writeEmitter.event,
      open: () => {
        writeEmitter.fire(`\x1b[1mSparkRun · ${name}\x1b[0m\r\n`);
      },
      close: () => {
        this.terminals.delete(unitId);
        this.termWriteEmitters.delete(unitId);
      },
      handleInput: (data: string) => {
        void this.client.request("termWrite", { unitId, data });
      },
      setDimensions: (dim) => {
        void this.client.request("termResize", {
          unitId,
          cols: dim.columns,
          rows: dim.rows,
        });
      },
    };
    const term = vscode.window.createTerminal({ name: `SparkRun: ${name}`, pty });
    this.terminals.set(unitId, term);
    term.show(true);
  }

  onTermData(ev: TermDataEvent): void {
    const emitter = this.termWriteEmitters.get(ev.unitId);
    if (!emitter) {
      return;
    }
    try {
      const text = Buffer.from(ev.data, "base64").toString("utf8");
      // Pseudoterminal expects \r\n style; pass through ANSI as-is with \n -> \r\n
      emitter.fire(text.replace(/\n/g, "\r\n"));
    } catch {
      /* ignore */
    }
  }

  dispose(): void {
    for (const ch of this.channels.values()) {
      ch.dispose();
    }
    this.channels.clear();
    for (const t of this.terminals.values()) {
      t.dispose();
    }
    this.terminals.clear();
  }

  private channel(unitId: string, name: string): vscode.OutputChannel {
    let ch = this.channels.get(unitId);
    if (!ch) {
      ch = vscode.window.createOutputChannel(`SparkRun: ${name}`);
      this.channels.set(unitId, ch);
    }
    return ch;
  }
}
