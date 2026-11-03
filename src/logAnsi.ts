/** Minimal ANSI SGR parser — Spring log coloring + Vue/npm terminal mode. */

export interface AnsiSeg {
  text: string;
  /** CSS class suffix: red | green | yellow | blue | magenta | cyan | white | bright-* | default */
  fg?: string;
  bold?: boolean;
}

export interface AnsiState {
  fg?: string;
  bold: boolean;
}

const ESC = /\u001b\[([0-9;]*)m/g;

const FG: Record<number, string> = {
  30: "black",
  31: "red",
  32: "green",
  33: "yellow",
  34: "blue",
  35: "magenta",
  36: "cyan",
  37: "white",
  39: "default",
  90: "bright-black",
  91: "bright-red",
  92: "bright-green",
  93: "bright-yellow",
  94: "bright-blue",
  95: "bright-magenta",
  96: "bright-cyan",
  97: "bright-white",
};

export function stripAnsi(s: string): string {
  return s.replace(/\u001b\[[0-9;]*m/g, "");
}

export function hasAnsi(s: string): boolean {
  return s.includes("\u001b[");
}

export function defaultAnsiState(): AnsiState {
  return { bold: false };
}

function applySgr(state: AnsiState, codes: number[]): AnsiState {
  let { fg, bold } = state;
  for (const c of codes) {
    if (c === 0) {
      fg = undefined;
      bold = false;
    } else if (c === 1) {
      bold = true;
    } else if (c === 22) {
      bold = false;
    } else if (FG[c]) {
      fg = FG[c] === "default" ? undefined : FG[c];
    }
    // ignore 38/48 256/truecolor for now — fall through as no-op pieces
  }
  return { fg, bold };
}

/** Parse one line; optionally continue SGR state across lines (terminal mode). */
export function parseAnsi(
  line: string,
  initial: AnsiState = defaultAnsiState(),
): { segs: AnsiSeg[]; state: AnsiState } {
  if (!hasAnsi(line)) {
    return {
      segs: line ? [{ text: line, fg: initial.fg, bold: initial.bold || undefined }] : [],
      state: initial,
    };
  }
  const segs: AnsiSeg[] = [];
  let state = { ...initial };
  let last = 0;
  ESC.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = ESC.exec(line)) !== null) {
    if (m.index > last) {
      segs.push({
        text: line.slice(last, m.index),
        fg: state.fg,
        bold: state.bold || undefined,
      });
    }
    const codes = m[1] === "" ? [0] : m[1].split(";").map((n) => Number(n) || 0);
    state = applySgr(state, codes);
    last = m.index + m[0].length;
  }
  if (last < line.length) {
    segs.push({
      text: line.slice(last),
      fg: state.fg,
      bold: state.bold || undefined,
    });
  }
  return { segs: segs.filter((s) => s.text.length > 0), state };
}

/**
 * When Spring ANSI is off, mimic IDEA console: gray time / colored LEVEL / muted logger / body.
 * Pattern approx: `yyyy-MM-dd HH:mm:ss.SSS  LEVEL pid --- [thread] logger : msg`
 */
export function parseSpringPlain(line: string): AnsiSeg[] | null {
  const m = line.match(
    /^(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}[.,]\d{3})\s+(\S+)\s+(\S+)\s+(---)\s+(\[[^\]]*\])\s+(\S+)\s*:\s?(.*)$/,
  );
  if (!m) return null;
  const [, time, level, pid, dash, thread, logger, msg] = m;
  const lv = level.trim().toUpperCase();
  let levelFg = "cyan";
  if (lv === "ERROR" || lv === "FATAL" || lv === "SEVERE") levelFg = "red";
  else if (lv.startsWith("WARN")) levelFg = "yellow";
  else if (lv === "INFO") levelFg = "green";
  else if (lv === "DEBUG" || lv === "TRACE") levelFg = "bright-black";

  return [
    { text: time, fg: "bright-black" },
    { text: "  " },
    { text: level.padStart(5), fg: levelFg, bold: true },
    { text: " " },
    { text: pid, fg: "bright-black" },
    { text: ` ${dash} ` },
    { text: thread, fg: "bright-black" },
    { text: " " },
    { text: logger, fg: "cyan" },
    { text: " : " },
    { text: msg },
  ];
}

/** Spring Boot / Java: log-oriented coloring. */
export function segmentsForLogLine(raw: string, fallbackLevel?: string): AnsiSeg[] {
  if (hasAnsi(raw)) return parseAnsi(raw).segs;
  const plain = stripAnsi(raw);
  const spring = parseSpringPlain(plain);
  if (spring) return spring;
  if (fallbackLevel === "launcher" || plain.includes("[launcher]") || /^\s*>\s+\S/.test(plain)) {
    return [{ text: plain, fg: "bright-black" }];
  }
  if (fallbackLevel === "error") return [{ text: plain, fg: "red" }];
  if (fallbackLevel === "warn") return [{ text: plain, fg: "yellow" }];
  if (fallbackLevel === "debug") return [{ text: plain, fg: "bright-black" }];
  return [{ text: plain }];
}

/**
 * Vue/npm: IDEA Run = terminal pane — only real ANSI, no log-level guessing.
 * Carries SGR across lines so chalk banners stay colored.
 */
export function segmentsForTerminalLines(lines: string[]): AnsiSeg[][] {
  let state = defaultAnsiState();
  return lines.map((raw) => {
    const { segs, state: next } = parseAnsi(raw, state);
    state = next;
    return segs.length ? segs : [{ text: "" }];
  });
}

export type ClassifiedLevel = "error" | "warn" | "info" | "debug" | "launcher" | "plain";

export function classifyLogLevel(line: string): ClassifiedLevel {
  const plain = stripAnsi(line);
  if (/^\s*>\s+\S/.test(plain)) return "launcher";
  if (plain.includes("[launcher]")) return "launcher";
  const m = plain.match(/\b(ERROR|SEVERE|FATAL|WARN(?:ING)?|INFO|DEBUG|TRACE)\b/);
  if (m) {
    const t = m[1].toUpperCase();
    if (t === "ERROR" || t === "SEVERE" || t === "FATAL") return "error";
    if (t.startsWith("WARN")) return "warn";
    if (t === "DEBUG" || t === "TRACE") return "debug";
    if (t === "INFO") return "info";
  }
  return "plain";
}

/** Whether a classified line passes the toolbar filter. */
export function linePassesFilter(
  level: ClassifiedLevel,
  filter: "all" | "warn" | "error",
): boolean {
  if (filter === "all") return true;
  if (filter === "error") return level === "error";
  // warn+: error + warn
  return level === "error" || level === "warn";
}

const FG_ANSI: Record<string, string> = {
  black: "30",
  red: "31",
  green: "32",
  yellow: "33",
  blue: "34",
  magenta: "35",
  cyan: "36",
  white: "37",
  "bright-black": "90",
  "bright-red": "91",
  "bright-green": "92",
  "bright-yellow": "93",
  "bright-blue": "94",
  "bright-magenta": "95",
  "bright-cyan": "96",
  "bright-white": "97",
};

/** Convert colored segments to a single ANSI string for xterm.write. */
export function segsToAnsi(segs: AnsiSeg[]): string {
  let out = "";
  for (const s of segs) {
    const codes: string[] = [];
    if (s.bold) codes.push("1");
    if (s.fg && FG_ANSI[s.fg]) codes.push(FG_ANSI[s.fg]);
    if (codes.length) out += `\u001b[${codes.join(";")}m`;
    out += s.text;
    if (codes.length) out += "\u001b[0m";
  }
  return out;
}

/** Format one log line for xterm (ANSI if present or Spring-colored). */
export function formatLogLineForXterm(raw: string, level?: ClassifiedLevel): string {
  const segs = segmentsForLogLine(raw, level);
  return segsToAnsi(segs) + "\r\n";
}

export type HealthSignal = "ready" | "failed" | "portInUse" | null;

/** Detect Boot/Vite health signals from a single log line. */
export function detectHealthSignal(line: string): HealthSignal {
  const plain = stripAnsi(line);
  if (
    /Address already in use/i.test(plain) ||
    /BindException/i.test(plain) ||
    /端口[^\n]{0,20}占用/.test(plain) ||
    /Web server failed to start\. Port/i.test(plain)
  ) {
    return "portInUse";
  }
  if (
    /BUILD FAILURE/i.test(plain) ||
    /APPLICATION FAILED TO START/i.test(plain) ||
    /Failed to start bean/i.test(plain)
  ) {
    return "failed";
  }
  if (
    /Started \S+ in .+/i.test(plain) ||
    /Tomcat started on port/i.test(plain) ||
    /Netty started on port/i.test(plain)
  ) {
    return "ready";
  }
  // Vite / Vue weak ready
  if (/Local:\s+https?:\/\//i.test(plain) || /ready in \d/i.test(plain)) {
    return "ready";
  }
  return null;
}
