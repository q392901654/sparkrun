import type { RunHealth } from "./types";

/** Settings section to jump to from a tip CTA. */
export type TipAction = "toolchain" | "project" | "none";

export interface LaunchTip {
  id: string;
  title: string;
  advice: string;
  action: TipAction;
  /** Short label for the CTA button when action !== none */
  actionLabel?: string;
}

type TipRule = {
  id: string;
  title: string;
  advice: string;
  action: TipAction;
  actionLabel?: string;
  /** Match against error / log text (case-insensitive for most). */
  test: (raw: string) => boolean;
};

const RULES: TipRule[] = [
  {
    id: "port-in-use",
    title: "端口被占用",
    advice: "结束占用该端口的进程，或修改 application*.yml 中的 server.port 后重新启动。",
    action: "none",
    test: (raw) =>
      /Address already in use/i.test(raw) ||
      /BindException/i.test(raw) ||
      /端口[^\n]{0,40}占用/.test(raw) ||
      /Web server failed to start\. Port/i.test(raw) ||
      /portInUse/i.test(raw),
  },
  {
    id: "jdk-missing",
    title: "未找到 JDK",
    advice: "打开设置 → 工具链，填写 JAVA_HOME，可用「查找」或「检测」确认 java / javac 可用。",
    action: "toolchain",
    actionLabel: "去配置工具链",
    test: (raw) =>
      /JAVA_HOME/i.test(raw) ||
      /javac failed/i.test(raw) ||
      /['"]?java['"]? (不是内部或外部命令|不是内部命令)/i.test(raw) ||
      /java: command not found/i.test(raw) ||
      /Unable to locate a Java Runtime/i.test(raw) ||
      /no java executable/i.test(raw) ||
      /JDK/i.test(raw) && /not found|找不到|不可用|missing/i.test(raw),
  },
  {
    id: "maven-missing",
    title: "未找到 Maven",
    advice: "打开设置 → 工具链，填写 MAVEN_HOME（或确保 PATH 有 mvn），再点「检测」。",
    action: "toolchain",
    actionLabel: "去配置工具链",
    test: (raw) =>
      /MAVEN_HOME/i.test(raw) ||
      /['"]?mvn['"]? (不是内部或外部命令|不是内部命令)/i.test(raw) ||
      /mvn: command not found/i.test(raw) ||
      /Maven/i.test(raw) && /not found|找不到|不可用|missing/i.test(raw),
  },
  {
    id: "node-missing",
    title: "Node 或依赖问题",
    advice:
      "检查全局 / 项目 / 模块 NODE_HOME；Vue 单元可在工作区点「检测」，缺 node_modules 时会先 install。",
    action: "toolchain",
    actionLabel: "去配置工具链",
    test: (raw) =>
      /NODE_HOME/i.test(raw) ||
      /node_modules/i.test(raw) ||
      /['"]?npm['"]? (不是内部或外部命令|不是内部命令)/i.test(raw) ||
      /['"]?node['"]? (不是内部或外部命令|不是内部命令)/i.test(raw) ||
      /node: command not found/i.test(raw) ||
      /Node/i.test(raw) && /不可用|not found|找不到|missing/i.test(raw),
  },
  {
    id: "cmdline-too-long",
    title: "命令行过长",
    advice:
      "Windows CreateProcess 限制约 8191 字符；启动器会尝试 classpath jar。仍失败请缩短工程路径或查看日志中的 [launcher] 提示。",
    action: "none",
    test: (raw) =>
      /CreateProcess/i.test(raw) ||
      /OS error 206/i.test(raw) ||
      /command line is too long/i.test(raw),
  },
  {
    id: "compile-failed",
    title: "编译失败",
    advice:
      "查看下方日志中的 [launcher] 与 BUILD FAILURE；可尝试「强制构建」，或检查 Maven 远程/本地仓库配置。",
    action: "project",
    actionLabel: "打开项目设置",
    test: (raw) =>
      /BUILD FAILURE/i.test(raw) ||
      /compile FAILED/i.test(raw) ||
      /javac failed/i.test(raw) ||
      /hot-reload mvn compile failed/i.test(raw) ||
      /build failed/i.test(raw),
  },
  {
    id: "app-start-failed",
    title: "应用启动失败",
    advice:
      "多为配置、库表或依赖问题。展开完整日志定位 APPLICATION FAILED TO START / Failed to start bean，复制相关片段排查。",
    action: "none",
    test: (raw) =>
      /APPLICATION FAILED TO START/i.test(raw) ||
      /Failed to start bean/i.test(raw) ||
      /Error creating bean/i.test(raw),
  },
];

const GENERIC_TIP: LaunchTip = {
  id: "generic",
  title: "操作失败",
  advice: "查看下方日志中的 [launcher] 提示与异常堆栈；工具链问题可到设置中检测 JAVA / Maven / Node。",
  action: "none",
};

/**
 * Classify an error string or log snippet into a user-facing tip.
 * Prefer explicit health when provided (e.g. portInUse from log scan).
 */
export function classifyLaunchTip(
  raw: string,
  health?: RunHealth | null,
): LaunchTip {
  if (health === "portInUse") {
    const r = RULES.find((x) => x.id === "port-in-use")!;
    return toTip(r);
  }
  const text = (raw || "").trim();
  if (text) {
    for (const rule of RULES) {
      if (rule.test(text)) return toTip(rule);
    }
  }
  if (health === "failed") {
    return {
      id: "run-failed",
      title: "运行失败",
      advice: "进程已退出或启动未成功。查看日志中的错误栈；端口冲突、工具链与编译问题见常见引导。",
      action: "none",
    };
  }
  return { ...GENERIC_TIP };
}

function toTip(rule: TipRule): LaunchTip {
  return {
    id: rule.id,
    title: rule.title,
    advice: rule.advice,
    action: rule.action,
    actionLabel: rule.actionLabel,
  };
}

/** Truncate long invoke errors for the toast headline. */
export function summarizeError(raw: unknown, max = 180): string {
  let s = String(raw ?? "").trim();
  // Tauri sometimes wraps as "Error: ..."
  s = s.replace(/^Error:\s*/i, "");
  if (s.length <= max) return s || "未知错误";
  return `${s.slice(0, max - 1)}…`;
}

/** Per unit+tipId once-per-start-cycle dedupe. */
export class TipDedupe {
  private seen = new Map<string, Set<string>>();

  /** Call when a unit (re)starts so tips can show again. */
  resetUnit(unitId: string) {
    this.seen.delete(unitId);
  }

  /** Returns true if this tip should be shown (first time in cycle). */
  tryShow(unitId: string | null | undefined, tipId: string): boolean {
    const key = unitId || "_global";
    let set = this.seen.get(key);
    if (!set) {
      set = new Set();
      this.seen.set(key, set);
    }
    if (set.has(tipId)) return false;
    set.add(tipId);
    return true;
  }
}
