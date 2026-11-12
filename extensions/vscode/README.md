# SparkRun for VS Code / Cursor

在编辑器里扫描并启停多模块 **Spring Boot** / **Vue** 单元。启动引擎复用桌面版同一套 Rust 核心，通过本地 **sidecar**（NDJSON stdio）通信。

## 安装

1. 构建 sidecar（仓库根目录）：

```bash
cargo build -p sparkrun-sidecar --release
```

2. 拷贝二进制到扩展：

```bash
# Windows PowerShell（仓库根）
./scripts/copy-sidecar.ps1
```

3. 打包并安装 VSIX：

```bash
cd extensions/vscode
npm install
npm run compile
npx vsce package --no-dependencies
# 在 VS Code / Cursor: Extensions → … → Install from VSIX
```

开发期可不打包：在 `settings.json` 设置

```json
"sparkrun.sidecarPath": "D:/IdeaProjects/project-launcher/target/debug/sparkrun-sidecar.exe"
```

然后按 F5 启动 Extension Development Host，或直接把本目录当扩展安装。

## 使用

1. 打开含多模块工程的 workspace（一期默认用第一个 folder；可用命令 **SparkRun: Select Workspace Folder** 切换）。
2. 活动栏打开 **SparkRun** → 点扫描，或命令 **SparkRun: Scan Workspace**。
3. 在单元上启动 / 停止；Boot 日志在 Output `SparkRun: <name>`，Vue 走内嵌 Pseudoterminal。

## 设置（`sparkrun.*`）

| 键 | 说明 |
|----|------|
| `sidecarPath` | sidecar 路径；空则用 `bin/` 或仓库 `target/debug` |
| `javaHome` / `mavenHome` / `nodeHome` | 工具链覆盖 |
| `mavenRepoUrl` / `mavenLocalRepo` | Maven 仓库 |
| `buildBeforeRun` | `smart` / `always` / `never` |
| `enableSpringBoot` / `enableVue` / `scanDepth` | 扫描范围 |
| `hotReload` / `hotReloadRestart` | 保存时增量编译 / DevTools 触发 |
| `stopUnitsOnDeactivate` | 扩展停用时停止托管进程（默认 true） |

## 与桌面版差异

- **不共享** 桌面版 `app_data` 配置；工具链与开关用 VS Code settings。
- 不做内嵌 Monaco / 文件树 / 全局搜索（用编辑器自身能力）。
- Sidecar 崩溃后会自动重启，但已托管进程句柄会丢失；可能需要手动结束孤儿 JVM / node。

## 协议（开发）

sidecar：`sparkrun-sidecar --data-dir <path>`，stdin/stdout 每行一条 JSON。详见仓库计划文档 / `crates/sparkrun-sidecar`。
