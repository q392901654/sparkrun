export interface TreeNode {
  name: string;
  path: string;
  isDir: boolean;
  expanded?: boolean;
  loading?: boolean;
  loaded?: boolean;
  children?: TreeNode[];
}

export type FileIconKind =
  | "dir"
  | "java"
  | "js"
  | "ts"
  | "vue"
  | "wpy"
  | "json"
  | "xml"
  | "yaml"
  | "md"
  | "html"
  | "css"
  | "img"
  | "cfg"
  | "sql"
  | "sh"
  | "lock"
  | "txt"
  | "file";

export function fileIconMeta(
  name: string,
  isDir: boolean,
): { kind: FileIconKind; label: string } {
  // Folders: no text badge (twist + muted glyph in the tree UI)
  if (isDir) return { kind: "dir", label: "" };
  const lower = name.toLowerCase();
  const dot = lower.lastIndexOf(".");
  const ext = dot >= 0 ? lower.slice(dot + 1) : "";

  if (lower === "pom.xml" || lower === "build.gradle" || lower === "build.gradle.kts") {
    return { kind: "cfg", label: "BUILD" };
  }
  if (
    lower === "package.json" ||
    lower === "package-lock.json" ||
    lower === "pnpm-lock.yaml" ||
    lower === "yarn.lock"
  ) {
    return { kind: "cfg", label: "PKG" };
  }
  if (
    lower === "application.yml" ||
    lower === "application.yaml" ||
    lower.startsWith("application-") ||
    lower === ".env" ||
    lower.startsWith(".env.")
  ) {
    return { kind: "cfg", label: "CFG" };
  }

  switch (ext) {
    case "java":
      return { kind: "java", label: "JAVA" };
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
      return { kind: "js", label: "JS" };
    case "ts":
    case "tsx":
      return { kind: "ts", label: "TS" };
    case "vue":
      return { kind: "vue", label: "VUE" };
    case "wpy":
      return { kind: "wpy", label: "WPY" };
    case "json":
      return { kind: "json", label: "JSON" };
    case "xml":
    case "xsd":
    case "svg":
      return { kind: "xml", label: ext === "svg" ? "SVG" : "XML" };
    case "yml":
    case "yaml":
      return { kind: "yaml", label: "YML" };
    case "md":
    case "markdown":
      return { kind: "md", label: "MD" };
    case "html":
    case "htm":
      return { kind: "html", label: "HTML" };
    case "css":
    case "scss":
    case "less":
    case "sass":
      return { kind: "css", label: "CSS" };
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "ico":
      return { kind: "img", label: "IMG" };
    case "properties":
    case "ini":
    case "conf":
    case "config":
    case "toml":
      return { kind: "cfg", label: "CFG" };
    case "sql":
      return { kind: "sql", label: "SQL" };
    case "sh":
    case "bash":
    case "ps1":
    case "bat":
    case "cmd":
      return { kind: "sh", label: "SH" };
    case "lock":
      return { kind: "lock", label: "LOCK" };
    case "txt":
    case "log":
      return { kind: "txt", label: "TXT" };
    default:
      return { kind: "file", label: ext ? ext.slice(0, 4).toUpperCase() : "FILE" };
  }
}
