<div align="center">

<img src=".github/assets/logo.svg" width="112" alt="Openplane logo" />

# Openplane

**AI コーディングエージェントのための、ローカルファーストなコックピット。**

手元の Claude Code・Kimi Code・DSH（DeepSeek）・Codex のセッションをひとつのウィンドウに集約します。トークン、ディスク使用量、プロジェクト、サブエージェントまで一目で把握でき、データは PC の外に出ません。

<p>
  <a href="https://github.com/arvelvale/openplane/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/github/license/arvelvale/openplane?style=flat-square&color=0B6BCB" /></a>
  <a href="https://github.com/arvelvale/openplane/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/arvelvale/openplane?style=flat-square&logo=github&color=15202B" /></a>
  <a href="https://github.com/arvelvale/openplane/commits/main"><img alt="Last commit" src="https://img.shields.io/github/last-commit/arvelvale/openplane?style=flat-square&color=5B6B7C" /></a>
</p>
<p>
  <img alt="Status" src="https://img.shields.io/badge/status-early%20prototype-C47B0A?style=flat-square" />
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows-0B6BCB?style=flat-square" />
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?style=flat-square&logo=tauri&logoColor=white" />
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.77%2B-B7410E?style=flat-square&logo=rust&logoColor=white" />
  <img alt="Frontend" src="https://img.shields.io/badge/frontend-vanilla%20JS-F7DF1E?style=flat-square&logo=javascript&logoColor=black" />
  <img alt="Telemetry" src="https://img.shields.io/badge/telemetry-none-0F9D6E?style=flat-square" />
</p>
<p>
  <img alt="Claude Code" src="https://img.shields.io/badge/Claude%20Code-対応済み-0F9D6E?style=flat-square" />
  <img alt="Kimi Code" src="https://img.shields.io/badge/Kimi%20Code-対応済み-0F9D6E?style=flat-square" />
  <img alt="DSH" src="https://img.shields.io/badge/DSH%20(DeepSeek)-対応済み-0F9D6E?style=flat-square" />
  <img alt="Codex" src="https://img.shields.io/badge/Codex-対応済み-0F9D6E?style=flat-square" />
</p>

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語**

<img src=".github/assets/screenshot-sessions.ja.png" alt="Openplane セッションハブ" width="100%" />

</div>

> [!NOTE]
> UI は English・简体中文・日本語に対応し、システム言語に自動で合わせます。トップバーからいつでも切り替えられます。スクリーンショットは架空のプロジェクトを使ったモックデータです。

## なぜ作ったか

複数のエージェントツールを並行して使っていると、毎日同じ不便にぶつかります。

1. **セッションがバラバラ**：ツールごとに JSONL やセッションフォルダの形式が違い、横断して検索・比較・再開できません。
2. **使用量が見えない**：そのセッションで実際に何トークン使ったのか。何百もの会話ログがディスクをどれだけ占めているのか。
3. **モデル切り替えが手作業**：ツールごとに環境変数と設定ファイルが別々です。

Openplane は、各ツールがすでにディスクへ書き出しているデータを読み取り、ひとつのボードにまとめます。エージェントのラップ、フォーク、再実装は一切しません。

## 現在できること

| 機能 | 状態 | 備考 |
|---|:---:|---|
| セッションハブ：Claude Code | ✅ | `~/.claude/projects`、サブエージェントは親セッションに統合 |
| セッションハブ：Kimi Code | ✅ | `~/.kimi-code/sessions`、新旧両方の `state.json` 形式に対応 |
| セッションハブ：DSH（DeepSeek） | ✅ | `~/.dsh/sessions`、zstd 圧縮のイベントログ、v0 / v3 形式に対応 |
| セッションハブ：Codex | ✅ | `~/.codex/sessions`、同じ id の rollout を統合、guardian サブエージェントは親セッションに統合 |
| 正確なトークン集計 | ✅ | API 呼び出し単位で重複排除し、入力 / キャッシュ書込 / キャッシュ読込 / 出力に分割 |
| セッション別・ハーネス別のディスク使用量 | ✅ | ステータス画面に合計と内訳を表示 |
| タイトル・パス・モデルで検索 | ✅ | |
| UI 三言語対応：English / 简体中文 / 日本語 | ✅ | システム言語に追従、トップバーで切り替え |
| 差分再スキャン | ✅ | 変更のないファイルはメモリキャッシュから返す |
| ローカルモデルプロキシ（`127.0.0.1:8787`） | ⏳ | ルーティング設定と死活監視のみ。リクエスト転送は未実装 |
| ターミナルで再開 | ⏳ | 現状はセッションフォルダを開くだけ |

<img src=".github/assets/screenshot-status.ja.png" alt="Openplane ステータス画面とストレージ内訳" width="100%" />

## トークンの数え方

どのツールも API 呼び出しごとに使用量を記録しますが、そのまま足し合わせられるものはありません。

| | 取得元 | 落とし穴 | Openplane の処理 |
|---|---|---|---|
| **Claude Code** | assistant 行の `message.usage` | ストリーミング時にコンテンツブロックごとに行が分かれ、同じ使用量が重複する（あるセッションでは 4,034 行が実際には 1,558 回の呼び出し） | `message.id` で重複排除し、最後の行を採用 |
| **Kimi Code** | `agents/*/wire.jsonl` 内の `usage.record` イベント | `usageScope: "session"` はコンテキスト圧縮の独立した呼び出しで、合計値ではない | 各レコードを 1 回ずつ集計 |
| **DSH** | マルチフレーム zstd ログ内の `assistant/message` イベントの `data.usage` | アップグレードしたセッションには同じ履歴の v0 と v3 のログが両方残る | 両方ある場合は v3 のみ読む |
| **Codex** | `token_count` イベント（新しい版では `token_usage_record` も） | `total_token_usage` はプロセス単位で再開時にリセット、`input_tokens` はキャッシュ分を含む、古い `token_count` はコンテキスト圧縮の呼び出しを記録しない | 呼び出し単位で重複排除して合算し、`token_usage_record` がある区間はそちらを優先、入力からキャッシュ分を差し引く |

セッション合計は、メインエージェントとすべてのサブエージェントの和です。検証方法：

- **Claude Code**：同じプロセス区間で自身の `cost-state` 記録と照合し、入力・キャッシュ読込・出力が完全に一致。
- **DSH**：DSH 自身の投影キャッシュと、キャッシュ作成時点までのイベントで完全に一致。
- **Codex**：両方の記録を持つ 12 ファイル中 8 ファイルで完全一致。残り 4 ファイルの差はちょうどコンテキスト圧縮の呼び出し分。

初期の Codex alpha 版のセッションは内訳のない合計値しか持たないため、推測せず「内訳なし」として表示します。

既知の制限：公式記録は `--resume` でリセットされるため、Openplane は会話ログから合計を再構築しています。公式記録にはタイトル生成など会話ログに残らない副次的な呼び出しも含まれるため、Openplane の Claude Code 合計は 1〜5% ほど少なく出ることがあります。

## はじめに

**必要なもの（Windows）**

- [Rust](https://rustup.rs)（MSVC ツールチェーン）
- [Visual Studio Build Tools](https://aka.ms/vs/17/release/vs_BuildTools.exe)（MSVC と Windows SDK）
- Node.js 18+
- WebView2（Windows 10/11 に標準搭載）

```powershell
git clone https://github.com/arvelvale/openplane.git
cd openplane
npm install
npm run dev        # デスクトップアプリ。実際のセッションを読み込む
```

UI だけ見たい場合は Rust 不要です。

```powershell
npm run preview    # http://127.0.0.1:1420（モックデータ）
```

> [!TIP]
> Rust のビルドは Git Bash ではなく PowerShell で実行してください。Git には同名の `link.exe` が同梱されていますが、MSVC のリンカーではありません。

## ディレクトリ構成

```text
openplane/
├─ ui/                      # index.html · styles.css · app.js · i18n.js（WebView とブラウザで共用）
├─ src-tauri/
│  └─ src/
│     ├─ adapters/
│     │  ├─ mod.rs          # SessionSummary、TokenUsage、キャッシュ、共通処理
│     │  ├─ claude_code.rs
│     │  └─ kimi_code.rs
│     ├─ proxy.rs           # 8787 死活監視 + ~/.openplane/proxy.json
│     └─ lib.rs             # Tauri コマンド
├─ scripts/preview.mjs      # 依存ゼロの静的プレビュー
├─ docs/                    # 設計ドキュメント（中国語）
└─ DESIGN.md                # ビジュアル仕様
```

## プライバシー

- 読み取り専用：各ハーネスのセッションフォルダには一切書き込みません。
- 書き込むのはルーティング設定の `~/.openplane/proxy.json` だけです。
- 通信なし、テレメトリなし。プロキシは `127.0.0.1` のみで待ち受けます。
- 貼り付けた秘密情報を含む可能性のあるフィールド（Kimi の `lastPrompt` など）は読み取りません。

## ロードマップ

- [x] Tauri 2 シェルとセッションハブ
- [x] Claude Code・Kimi Code・DSH・Codex アダプター
- [x] トークンとディスク使用量の集計
- [ ] 永続インデックスによる高速起動
- [ ] OpenAI 互換 / Anthropic 形式の本物のローカルプロキシ
- [ ] 各 CLI でのセッション再開
- [ ] トレイ、グローバルショートカット、インストーラー

設計ドキュメント（中国語）：[プロジェクト概要](docs/00-项目定位.md) · [アーキテクチャ](docs/01-架构与技术路线.md) · [ロードマップ](docs/02-MVP路线图.md)
