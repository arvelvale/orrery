# Openplane — Design Pass

## Subject
Openplane：个人日常用的 **桌面** Agent 驾驶舱。一眼看清：哪些 harness 在跑、会话在哪、模型走哪条路。  
Audience: 多 harness 并行、长期坐在电脑前的独立开发者（首先是作者自己）。  
Job of the UI: 3 秒态势感知 + 一次点击切模型 / 进会话。

## Mode
**Convention mode**（桌面工具/运维台），浅色工作台，航空运行板作为产品身份而非装饰。  
签名元素只准一个：顶部 **Annunciator 灯条**（PROXY / 各 harness 健康灯）。

## Palette（浅色）
| Token | Hex | Role |
|---|---|---|
| `--bg` | `#F4F6F9` | 画布（冷纸白） |
| `--surface` | `#FFFFFF` | 卡片/面板 |
| `--surface-2` | `#EDF1F6` | 次级底（输入框、hover） |
| `--ink` | `#15202B` | 主文字 |
| `--ink-2` | `#5B6B7C` | 次要文字 |
| `--line` | `#D8E0EA` | 发丝线 |
| `--nav` | `#0B6BCB` | 主强调（选中/主按钮/链接） |
| `--run` | `#0F9D6E` | 仅 running / connected |
| `--warn` | `#C47B0A` | 仅 degraded / 注意 |
| `--fault` | `#D64545` | 仅 error |
| `--muted` | `#9AA8B8` | 禁用/弱信息 |

## Typography
- UI：`'Segoe UI', 'PingFang SC', 'Microsoft YaHei', system-ui, sans-serif`  
  15px / 1.45；标题 600，`letter-spacing: -0.02em`
- Data：`'Cascadia Code', 'SF Mono', Consolas, ui-monospace, monospace` 11–13px  
  会话 ID、模型名、时间戳、路径
- 侧栏导航 13px；列表标题 14px 600
- 字体栈随 `<html lang>` 切换：`ja` 用 Yu Gothic UI / Hiragino Sans / Meiryo，`zh-CN` 用 PingFang SC / Microsoft YaHei。中日文共用汉字字形不同，不能混用一套栈

## Language
- 界面三语：English / 简体中文 / 日本語，文案集中在 `ui/i18n.js`
- 默认：`?lang=` → 上次手动选择 → 系统语言 → English
- 切换入口：顶栏右侧 `EN · 中 · 日` 分段按钮，切换即时生效，不刷新页面、不关闭已打开的详情
- **不翻译**：仪表式等宽标签（RUNNING / IDLE / HARNESS / PROXY ROUTES / ONLINE …）、harness 名、模型名、路径，这些是驾驶舱身份
- 相对时间、复数用 `Intl.RelativeTimeFormat` / `Intl.PluralRules`，不手拼
- 用户会话标题保持原文，只有空标题时按界面语言兜底

## Layout（桌面优先）
```
┌──────────────────────────────────────────────────────────┐
│ header  brand · annunciator lamps · proxy status · actions│
├──────────┬───────────────────────────────┬───────────────┤
│ sidebar  │ toolbar: search + harness filter│ detail        │
│ 200px    │ session strips (flight board)  │ 340px         │
│ nav      │                               │ (选中时)       │
└──────────┴───────────────────────────────┴───────────────┘
```
- 最小舒适宽 1100px；内容区可再宽
- 会话条 = 航行情报条：左状态竖条、harness 徽章、标题、模型/路径/时间元数据
- 无底部 tab 栏；主导航在左栏
- 间距 4/8/12/16/24；触控目标 ≥32px（桌面鼠标为主，但别做小点）
- <900px 时侧栏折叠为图标条，详情改底部抽屉（保底，不是主场景）

## Signature
顶部 **Annunciator**：圆形指示灯一排。绿=run，蓝=idle，琥珀=warn，灰=off。  
加载时灯按序自检一次；点灯可跳到对应筛选。

## Risk
不做：深色黑客风、玻璃拟态、emoji、重阴影、大 hero 数字。  
做：白底发丝线、等宽元数据、状态灯、情报条式列表、克制的 ops blue。

## Empty / Error
- 无会话：说明未扫到目录 +「扫描本地路径」按钮（文案写清原型限制）
- 代理离线：灯变琥珀 + 可操作提示（检查端口）

## Tech
- 目标壳：**Tauri 2**（包体小、Rust 适合扫盘与子进程）
- 本文件为 WebView 前身的可运行 HTML 原型；真实网络/扫描不在原型假造
