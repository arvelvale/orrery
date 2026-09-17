# AGENTS.md — Orrery

## 项目是什么

个人 Agent 驾驶舱（Tauri 2）：跨 harness 会话中枢 + 本地模型代理。先自用，后开源。

## 当前状态

- 阶段 0B：`npm run dev` 桌面窗口已跑通（2026-09-14），Claude Code 会话真扫 + 磁盘占用统计
- 前端：`ui/`（`index.html` + `styles.css` + `app.js`，浏览器 mock / Tauri invoke 双模式）
- 真实扫描：Claude Code、Kimi Code、DSH（DeepSeek）、Codex、OpenCode 五家；另有通用适配器 `custom.rs`，读 `~/.orrery/harnesses.json` 登记的 OpenCode 系工具（只读、不支持删除与恢复）。**不要把未公开工具的目录名/表结构写进仓库**，那属于用户本地配置
- 模型代理：`src-tauri/src/proxy/` 已能真实转发（OpenAI / Anthropic 两种 wire、SSE 透传、应用内启停），仅对假上游验证过，未与真实供应商联调
- 解析索引：`~/.orrery/index.json` 落盘，冷启动 188 个会话 5.5s → 0.12s
- 在终端恢复：`src-tauri/src/resume.rs`，五家各自的恢复命令见该文件头部表格；Windows 上找可执行文件必须 `.exe`/`.cmd` 优先于无扩展名（npm 的 bash shim 会假装启动成功）
- 发版：打 `v*` tag → GitHub Actions 三平台出包（Windows msi/nsis、macOS universal dmg、Linux deb/rpm/AppImage）→ 进**草稿** release，人工确认再公开。改版本号要同时改 `package.json` / `tauri.conf.json` / `Cargo.toml` + `Cargo.lock`，再打 tag
- macOS / Linux 的包只有 CI 验证过（clippy + 测试），没有人在真机上跑过应用，README 里如实标注了这点
- token 口径：主 agent + 子 agent，按 API 调用去重求和（CC 按 message.id 保留最后一行；Kimi 每条 usage.record 即一次调用；DSH 只读 v3 日志；Codex 以 token_usage_record 为准、之前时段累加去重后的 token_count.last，输入要减缓存命中）。每接一个新 harness 都要用独立脚本逐会话对账后再宣布完成。字段对照写在 `src-tauri/src/adapters/mod.rs` 的 `TokenUsage` 注释里，改口径先改那张表

## 约束

1. UI 对齐 `DESIGN.md`；状态灯语义固定（绿 run / 蓝 idle / 琥珀 warn / 灰 off）。
2. 浏览器预览可 mock；Tauri 路径必须走 `invoke`，失败要有可见提示。
3. 密钥只走环境变量：配置里只存变量名，转发瞬间才 `std::env::var` 读，不落盘、不进日志、不回传界面。代理只听回环地址（非回环的 `listen` 拒绝启动）；调用方带来的鉴权头一律丢弃并按目标供应商重建；选供应商按模型前缀严格匹配，匹配不到报错，不许加兜底。
4. 提交信息用中文。仓库：https://github.com/arvelvale/orrery（公开，MIT）；README 英文为默认，改 README 时三语（README.md / README.zh-CN.md / README.ja.md）同步。
5. `docs/` 是设计文档正本（作者本机的笔记库以目录链接挂载到这里，路径不入库）。
6. README 截图用 mock 数据（无头 Edge 截 `npm run preview?lang=<locale>` 页面，三语各一套 `screenshot-*.{en,zh-CN,ja}.png`），不要用真实会话截图——会暴露会话标题与项目路径。
7. 界面文案一律走 `ui/i18n.js` 的 `t()`，新增键三种语言同时写（控制台 `[i18n] missing keys` 会报缺失）；Rust 后端只返回原始数据（时间戳、空标题），不产出任何自然语言。
8. `ui/app.js` 的 mock 会话必须是虚构项目（acme-web、weather-cli…），不得出现真实项目名或本机路径。
9. **加平台专有调用前先想清楚另外两个平台**：`Command::new("tasklist")` 这类在别的平台上只是执行失败返回空值，编译和测试都不报错，靠它做判断的逻辑会静默失效（删除保护就踩过）。三平台分支写全，或者显式降级并说明。
10. **改 `SessionSummary` / `Rollout` 的字段就要 bump `adapters/index.rs` 的 `INDEX_VERSION`**：索引按这个版本号判断能不能用，忘了 bump 会读出半截旧数据。索引只是缓存，坏了直接整份丢弃重建，任何情况下都不允许它让会话列表出错。
11. **删除功能是破坏性操作**：改 `cleanup.rs` 前先读模块顶部的安全约束；验证只在沙盒做（调试构建设 `ORRERY_HOME` 指向沙盒主目录），不对真实会话调用 `delete_sessions`；前后对真实目录做快照比对。

## 常用命令

```powershell
# 浏览器预览
npm run preview   # http://127.0.0.1:1420；app.js 是 ES module，直接双击 index.html（file://）不会运行

# 桌面（需 Rust + npm i）
cd orrery
npm install
npm run dev
```

## 文档入口

- 定位：`docs/00-项目定位.md`
- 架构：`docs/01-架构与技术路线.md`
- 路线：`docs/02-MVP路线图.md`

## 实测手段

- 桌面窗口自动化：启动前设 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223 --remote-allow-origins=http://127.0.0.1:9223`，再用 CDP（`/json/list` → WebSocket）读 DOM、截图。
- `tauri dev` 无 devUrl 时由内置服务器在 `127.0.0.1:1430` 提供 `ui/`，浏览器打开同地址即 mock 模式。
- `app.js` 是 ES module，`state` 等不在 window 上，自动化时走 DOM 或 `window.__TAURI__.core.invoke`。
