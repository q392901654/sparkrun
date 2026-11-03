# SparkRun

本地多模块项目启动器（[Tauri 2](https://v2.tauri.app/) + Vue 3），产品名 **SparkRun**。按**项目**管理根目录与工具链，扫描可启动单元，一键启停并在应用内看日志 / 终端。

适合需要同时跑多个 Spring Boot API 与 Vue 前端、又不想为每个模块单独开终端窗口的开发者。

## 目录

- [功能概览](#功能概览)
- [支持的单元类型](#支持的单元类型)
- [环境要求](#环境要求)
- [快速开始](#快速开始)
- [日常使用](#日常使用)
- [Spring Boot 智能编译](#spring-boot-智能编译)
- [Spring Boot 热更新（DevTools）](#spring-boot-热更新devtools)
- [界面说明](#界面说明)
- [外观与主题](#外观与主题)
- [性能与分包](#性能与分包)
- [配置与缓存](#配置与缓存)
- [常见问题](#常见问题)
- [获取帮助](#获取帮助)

## 功能概览

| 能力 | 说明 |
|------|------|
| 多项目管理 | 每个项目独立根目录；可覆盖 JAVA / Maven / Node 与 Maven 仓库 |
| 扫描发现 | 自动识别 Spring Boot / Vue 可启动单元，结果可缓存 |
| 启停与构建 | 启动 / 停止；Boot 可增量编译、强制构建、安装本地依赖 |
| Boot 热更新 | 可选 DevTools：保存 Java/配置 → 增量 compile → 重启 ApplicationContext（**不是** JRebel） |
| IDEA 式布局 | 左项目文件树 · 上 [Monaco Editor](https://microsoft.github.io/monaco-editor/) · 下运行控制台；全局搜索（Ctrl+Shift+F / 连按 Shift） |
| 运行中 Tab | 每个运行中单元一个日志 Tab，点 Tab 切换输出 |
| 健康信号 | 就绪（绿）、失败 / 端口占用（红），少翻日志确认 |
| 日志与终端 | Boot：xterm 流式日志 + 级别过滤；Vue：内嵌 PTY 可输入 |
| Git | 顶栏 **Git**：status / stage / commit / fetch·pull·push、分支检出与合并、stash；冲突时三方合并（Base / Ours / Theirs / Result） |

## 支持的单元类型

| 类型 | 如何识别 | 如何启动 |
|------|----------|----------|
| Spring Boot | `pom.xml` + Boot parent/plugin，或存在 `@SpringBootApplication` | IDEA 式增量 compile → `java -cp`；识别 `application*.yml`，可选 Spring Profile |
| Vue | `package.json` 含 vue / `@vue` / wepy 等，且有 `dev` / `serve` / `start` | 缺 `node_modules` 时先 install，再 `{pm} run {script}` |

## 环境要求

- **必装**：Node.js 18+、Rust（`rustup default stable`）、Windows 上需 [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/)
- **按需**：JDK、Maven、Node（启动对应类型单元时使用）

## 快速开始

```bash
cd project-launcher
npm install
npm run tauri dev
```

首次打开建议按下列顺序配置一次：

1. **设置 → 工具链**：填写 `JAVA_HOME` / `MAVEN_HOME` / 默认 `NODE_HOME`；可选 Maven 远程仓库 URL、本地仓库目录  
2. **左侧 → 新建项目**：名称 + 仓库根目录（例如本仓库根路径）  
3. **工作区 → 扫描** → 选中单元 → **启动**

之后日常路径一般是：**选项目 →（自动加载扫描缓存）→ 启停 → 看日志 / 终端**。模块结构变化时再点「扫描」。

## 日常使用

- **启动 / 停止**：日志上方操作条；双击列表单元也可启动（未运行时）  
- **构建**（Boot）：操作条次要按钮；勾选「强制构建」可跳过智能跳过逻辑  
- **更多 → 安装依赖**：对本地 Maven 模块执行安装（高级，日常通常不需要）  
- **Vue 的 `NODE_HOME`**：可在工作区按模块覆盖（优先于项目 / 全局），便于多 Node 版本并存  
- **编译策略芯片**：工作区工具条上的「智能编译」等标签可点进项目设置  

日志中出现 `[launcher] skip compile (up-to-date)` 表示该模块本次被跳过编译。

## Spring Boot 智能编译

对齐 IDEA，避免大模块（如 `growup100-business`）每次全量 `mvn install`：

1. **smart（默认）**：源码 / `pom` 相对上次编译未变则跳过；过期才 `mvn compile -DskipTests`  
2. **always**：每次强制参与 compile（不跳过模块），仍按**文件**增量  
3. **never**：不编译，直接使用已有 `target/classes`（没有则报错）

**按文件增量（重要）**：Maven 默认 `useIncrementalCompilation=true` 会在「有变更」时**整模块重编**（如 `growup100-business` 七千多个 `.java`）。启动器对 `compile` / `install` 传入 `-Dmaven.compiler.useIncrementalCompilation=false`，只编译比对应 `.class` 更新的源文件。

热更新保存单个 `.java` 时优先 **`javac` 单文件**（classpath 缓存为模块下 `.launcher-compile-cp.txt`）；失败再回退上述增量 `mvn compile`。

限制：改了被很多类引用的 public API 时，依赖类可能不会自动跟着编；若运行异常，用「强制构建」或清 `target/classes` 后再编。改 `pom` 后会刷新 classpath 缓存。

运行 classpath：`当前模块/target/classes` + **本地依赖**的 `target/classes` + 第三方 jar（剔除已用本地 classes 替代的 `.m2` artifact）。

填写全局 / 项目远程仓库后，启动器会写临时 `settings.xml`（mirror `*` + 可选 `localRepository`），所有 `mvn` 调用带 `-s`。留空则使用本机默认 Maven 配置。

## Spring Boot 热更新（DevTools）

**这是 Spring ApplicationContext 快速重启，不是商业 JRebel / 方法级热替换。** 默认关闭，避免误开到不该用的环境。

| 步骤 | 行为 |
|------|------|
| 开启 | **设置 → 项目** 勾选「DevTools 热重启」并保存；工作区芯片显示「热更新 · 开」 |
| 启动 | 启动器若 classpath 上没有 `spring-boot-devtools`，会按当前 Boot 版本从本地 `.m2` 追加 jar（缺失则 `dependency:get`）；并打开 restart / trigger-file |
| 保存 | 编辑器保存 `.java` / `.kt` / `.yml` / `.yaml` / `.properties` 且对应 Boot 单元在运行时：`.java` 优先单文件 `javac`，否则增量 `mvn` / `process-resources`，再 touch `target/classes/.reloadtrigger` |
| 生效 | DevTools 重启上下文（进程一般仍在）；日志出现 `Restarted application` 时提示「热更新完成」 |

边界与限制：

- Vue / WePY 等前端继续用各自 HMR，保存前端文件**不会**触发 Boot 编译  
- 改 `pom`、改启动类签名、部分静态结构变更等仍可能需要**手动停止再启动**  
- 上下文重启比冷启动快，但仍是秒级～十余秒，不是方法体瞬间替换  
- 关闭开关后：保存只写磁盘，不 compile、不触发 DevTools  

## 界面说明

工作区对齐 IDEA 常用分区：**左侧**项目文件树 + 当前运行单元，**上方** Monaco 编辑，**下方**看运行输出。左右宽度、上下高度均可拖动分割条调整（比例会记住）。

### 左侧文件树

- 根目录为当前项目 `rootPath`；懒加载 `list_dir`（跳过 `node_modules` / `target` / `.git` 等）  
- **单击文件**直接在编辑器打开；路径落在某扫描单元下时自动选中该单元（供下方启停）  
- 顶部下拉可切换当前运行单元并标星；标星文件快捷区仍按单元收藏  
- 打开文件后（含全局搜索命中、切换 Tab）会 **reveal**：展开父目录、选中节点并滚入视野  

### Git

- 顶栏 **Git**（需已选项目；仓库根 = 项目 `rootPath`）  
- **Files**：暂存 / 取消暂存 / 丢弃；选中查看 diff；提交（支持 Amend，仅当 HEAD 未推送或可安全改写时）  
- **Branches**：检出、新建、**合并到当前分支**；Fetch / Pull / Push  
- **冲突**：合并或 Pull 产生冲突时进入三方面板（Base / Ours / Theirs / Result）→ 写回并 Stage → **完成合并**；可 Abort  
- **Commits / Stash**：查看 log / show；stash push / apply / drop  
- 依赖本机 PATH 中的 `git`；未安装或不在仓库内会显示空态提示  

### 搜索所有地方

- 快捷键：**Ctrl+Shift+F** 或 **连按两次 Shift**；工具条「搜索」/ 编辑器 Tab `+`  
- Tab：**所有 | 文件 | 文本 | 接口 | 操作**；可选「仅当前单元」  
- **接口**：启发式扫描 Java `@RequestMapping` / `@GetMapping` 等（非完整 PSI）  
- **操作**：扫描、启停当前单元、设置、保存等  
- 与 IDEA 差异：无类/符号索引、无「包括非项目项」  

### 编辑器（Monaco Editor）

- 多标签编辑；脏文件名后带 `*`；**Ctrl/Cmd+S** 或工具条「保存」写回磁盘  
- 搜索/文本命中可跳到行（`revealLine`）  
- 按扩展名语法高亮；内置 minimap、括号着色、查找替换  
- 主题跟随应用浅色 / 深色；单文件软上限约 8MB  
- 说明：内嵌 Monaco **不是**完整 IDE（无项目级 LSP / Vue Volar）；复杂重构仍建议在 Cursor / IDEA 中打开  

### 运行中 Tab

正在 `starting` / `running` 的单元出现在日志区顶部 Tab（不在列表上半区重复占位）。点 Tab 切换对应输出；进程结束后 Tab 消失，仍可从列表查看缓冲日志。

### 日志与终端

| 类型 | 行为 |
|------|------|
| Spring Boot | xterm 流式写入（批处理，减轻卡顿）；工具条：**清空**、**跟随滚底**、级别（全部 / WARN+ / ERROR）；内存约 2 万行；「完整日志」为纯文本并可复制 |
| Vue | 内嵌 PTY（xterm.js）；可输入；「清空」只清视图，不停进程 |

## 外观与主题

设置页表单 / 按钮等使用 [Naive UI](https://www.naiveui.com/)；无边框顶栏、Monaco、xterm 与文件树仍为自定义壳，通过 CSS 变量跟色。

| 项 | 说明 |
|----|------|
| 主题 | `system` / `light` / `dark`；暗色同时启用 Naive `darkTheme` 与 `data-theme` |
| 主色 | `primaryColor`（任意 `#rrggbb`）；预设色块快捷点选；颜色选择器可任意改 |
| 同步 | `applyUiPreferences` 写 `--primary`（自定义区）+ `NConfigProvider` 的 `themeOverrides.common.primaryColor*`（Naive 组件） |
| 密度 | `comfortable` → Naive `medium`；`compact` → `small`，并映射间距 |

外观写入全局 `settings.json` 的 `ui` 字段。

## 性能与分包

原则：**壳轻、能力懒、禁止回灌首屏。**

| 项 | 约定 |
|----|------|
| 首屏 | 顶栏 / 侧栏骨架 / 设置与 Naive 轻控件；`index.html` 有本地 splash，无外网字体 |
| 冷启动层 | Vue 挂载后显示真实步骤（读配置 → 项目 → 扫描缓存）；无项目时极短结束；**不**启动即全仓扫描 |
| 异步组件 | Monaco（`CodeEditor`）、xterm（`LogXterm` / `TerminalPane`）、文件树、全局搜索均 `defineAsyncComponent` |
| 预热 | 壳挂载后 `requestIdleCallback` 预取编辑器 / 日志 / 文件树 chunk，并预拉工具链「查找」缓存，不阻塞首绘 |
| 工具链缓存 | `toolchain-homes-cache.json`（约 24h TTL）；「查找」命中磁盘缓存；`force` 可强制重探 |
| Vite chunk | `monaco` / `xterm` / `naive` / `vendor`（见 `vite.config.ts` `manualChunks`） |
| 日志 / 树 | 日志走 xterm 批写（内存约 2 万行上限）；文件树懒 `list_dir`，勿整仓展开 |
| 新功能 | 独立 `.vue` + 默认 async；勿把重 UI 堆进 `App.vue` |

Agent / 编码硬约束见仓库 [`.cursor/rules/sparkrun-tauri-vue.mdc`](../.cursor/rules/sparkrun-tauri-vue.mdc)。

## 配置与缓存

配置与扫描缓存在本机应用数据目录：

- `settings.json` — 全局工具链与外观  
- `projects.json` — 项目列表  
- `scan-cache/{projectId}.json` — 单元列表、选中单元、Spring Profile、模块 `NODE_HOME`、标星  
- `toolchain-homes-cache.json` — JDK / Maven / Node 探测结果（约 24h，供「查找」）  

再次打开或切换项目会自动加载扫描缓存；修改项目根目录会使旧缓存失效；删除项目时一并清除对应缓存。

底部状态栏：左侧项目与单元数 / 缓存时间；中间为扫描 / 启动等任务文案；右侧进度或 Ready。扫描在后台线程执行，不锁死界面。

**设置 → 项目**：扫描深度、识别类型、启动前编译策略、DevTools 热更新开关，以及可折叠的工具链 / Maven 仓库覆盖。  
**设置 → 外观**：主题、字号、任意主色（含预设）、密度；见上节。

## 常见问题

启动 / 构建 / 工具链失败时，界面会弹出错误摘要 + 简短引导（可一点跳转设置）。与 toast 规则一致的常见情况：

| 现象 | 建议 |
|------|------|
| 端口占用（Address already in use / 状态「端口占用」） | 结束占用进程，或改 `application*.yml` 的 `server.port` 后重启 |
| 未找到 JDK / JAVA_HOME | **设置 → 工具链** 填写并「查找」/「检测」 |
| 未找到 Maven / MAVEN_HOME | 同上配置 Maven；也可检查 PATH |
| Node 不可用 / 缺 node_modules | 配置 NODE_HOME（模块可覆盖）；Vue 启动前会按需 install |
| BUILD FAILURE / 编译失败 | 看日志 `[launcher]`；可勾选强制构建；检查 Maven 仓库 URL |
| APPLICATION FAILED TO START | 配置 / 库表 / 依赖问题居多；用「完整日志」复制堆栈排查 |
| CreateProcess / OS error 206 | Windows 命令行过长；启动器会尝试 classpath jar，仍失败可缩短工程路径 |

同一单元一次启动周期内，同类引导只弹一次，避免日志刷屏。

## 获取帮助

- 本目录即应用源码；行为与编译逻辑以界面与日志中的 `[launcher]` 提示为准  
- 问题排查时请附带：单元名称、编译策略（smart / always / never）、完整日志中相关片段  

欢迎在团队流程中提需求或缺陷。
