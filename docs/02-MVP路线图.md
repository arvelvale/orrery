# 02 · MVP 路线图

## 阶段 0A · UI 原型（已完成）

- [x] 视觉与信息架构：DESIGN.md
- [x] 浅色桌面三栏：会话 / 模型 / 状态
- [x] 浏览器预览交互（`npm run preview`；2026-09-14 实测 file:// 直接打开时 ES module 被拦截，不可用）

## 阶段 0B · Tauri 2 骨架（已完成，2026-09-14 桌面窗口实测）

- [x] `src-tauri/` 工程：Cargo.toml、tauri.conf.json、capabilities、图标
- [x] 前端拆分 `ui/index.html` + `styles.css` + `app.js`（WebView / 浏览器共用）
- [x] `withGlobalTauri` + invoke 桥：`list_sessions` / `get_proxy_status` / `ping_proxy` / `save_route` / `open_path`
- [x] Claude Code adapter：扫 `~/.claude/projects/<project>/*.jsonl`，解析标题/模型/用量
- [x] 路由配置落盘 `~/.openplane/proxy.json`
- [x] **本机 Rust 1.95.0 可用**（`%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin`）
- [x] `cargo check` 已解析并锁定 485 个依赖，开始编译
- [x] VS Build Tools 已装，`cargo build` 链接出 `target/debug/openplane.exe`（2026-09-13）
- [x] 前端移入 `ui/`，`frontendDist` 改为 `../ui`（原 `../` 会把 `target/` 当前端资源嵌入，因 `.cargo-lock` 被锁报 os error 33）
- [x] `npm i` + `npm run dev` 打开桌面窗口，会话来源「本机扫描」（2026-09-14）
- [x] 会话磁盘占用：单条 = 主 jsonl + 同名附属目录（subagents / tool-results）；`storage_stats` 按 harness 汇总 + 总计（状态页卡片）
- [x] 子 agent jsonl 并入父会话不再单列（47 → 25 条）；项目路径改读 jsonl 的 `cwd`
- [x] `list_sessions` / `storage_stats` 标 `#[tauri::command(async)]`，扫盘不占主线程
- 验收：桌面窗口打开；侧栏角标显示「Tauri 桌面」；会话来源为「本机扫描」（有数据时）

### Windows 构建环境注意（2026-09 实测）

| 项 | 说明 |
|---|---|
| Rust | stable（MSVC 工具链），实测 1.95.0 |
| VS Build Tools 2022 | 只需 MSVC x64 + Windows 11 SDK 两个组件，实测 MSVC 14.44 |
| Windows SDK 位置 | 可能不在 C 盘默认路径，以注册表 `KitsRoot10` 为准，排查时别只看 C 盘 |
| 同名陷阱 | Git 自带的 `Git\usr\bin\link.exe` 不是链接器；Rust 构建用 PowerShell 跑 |
| Node | 18+（实测 v22），`npm i` 安装 tauri-cli 2.11 |

日常：`npm run dev`（桌面）/ `npm run preview`（浏览器 mock）。

## 阶段 1 · 会话中枢（真实数据打磨）

目标：稳定扫真实目录，补 MiMoCode，索引与搜索。

| 任务 | 验收 |
|---|---|
| 打磨 Claude Code adapter（标题/状态/token 更准） | 与 `claude --resume` 列表可对上；token 已修（2026-09-14，见下方决策日志） |
| MiMoCode adapter | 列出 mimocode sessions |
| ~~Kimi adapter~~ ✅ 2026-09-14 | 91 个会话全部列出，token 与独立脚本逐项一致 |
| DSH adapter（调研格式后） | 至少元数据 |
| 本地索引 + 搜索 | 标题/路径/模型可检索，增量 < 1s |
| 「在终端恢复」 | 调起对应 CLI（目前仅打开目录） |

## 阶段 2 · 模型代理

| 任务 | 验收 |
|---|---|
| 本地 HTTP 服务 | `curl /health` 通 |
| OpenAI-compatible 入口 | harness 可指向 8787 |
| 路由表读写生效 | UI 改模型 → 下一请求走新路由 |
| 密钥读环境变量 | 仓库无明文 key |

## 阶段 3 · 壳与体验

| 任务 | 验收 |
|---|---|
| 托盘 + 全局快捷键 | 可呼出 |
| 系统通知 | 错误/代理离线可开关 |
| 安装包打磨 | MSI/NSIS 可装 |

## 阶段 4 · 多窗格终端（可选，P1）

- PTY + xterm.js；**阶段 1–2 稳定后再做**

## 明确延期

- 插件系统、云同步、主题商店、遥测面板

## 决策日志

| 日期 | 决策 | 理由 |
|---|---|---|
| 2026-09 | 先自用驾驶舱，窄楔子=会话+代理 | 全能壳易变成兼容性维护地狱 |
| 2026-09 | 桌面壳锁定 Tauri 2 | 日常在电脑上用；包体/资源优先 |
| 2026-09 | UI 改为浅色桌面三栏 | 手机形态不匹配 agent 使用场景；样式已确认 |
| 2026-09 | 不在单文件 HTML 上继续堆，先落 Tauri 工程 | 阶段 0=骨架；前端三文件由 WebView 共用 |
| 2026-09 | Claude 会话先做真扫，其余 harness 仍 mock | 尽快验证 Rust↔UI 数据链路 |
| 2026-09 | 界面三语（en / zh-CN / ja），默认跟随系统语言 | 仓库公开且 README 英文优先；文案量小（约 90 条），原生字典即可，不引 i18n 库 |
| 2026-09 | 后端不产出自然语言，相对时间/空标题兜底交给前端 | 否则切语言时后端字符串无法跟随 |
| 2026-09 | mock 数据改为虚构项目，三语各一份标题 | 截图公开，避免暴露真实项目；三语 README 各自截对应语言界面 |
