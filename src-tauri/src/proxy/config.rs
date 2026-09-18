//! 代理配置：`~/.orrery/proxy.json`
//!
//! 用户在应用内维护：供应商（base_url + 密钥）、模型清单、路由。
//! 密钥可明文写在配置里（自用本地工具的有意选择），也可用环境变量名回退。
//! 解析顺序：`api_key` 非空优先，否则读 `api_key_env` 指向的环境变量。
//! 界面状态只回报「是否已设置」，编辑接口才返回密钥原文。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// 供应商的上游协议形状
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    /// `POST {base_url}/chat/completions`，`Authorization: Bearer`
    Openai,
    /// `POST {base_url}/messages`，`x-api-key` + `anthropic-version`
    Anthropic,
}

impl Wire {
    pub fn as_str(&self) -> &'static str {
        match self {
            Wire::Openai => "openai",
            Wire::Anthropic => "anthropic",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub base_url: String,
    /// 明文密钥（用户选择应用内填写时写入）；空则回退 api_key_env
    #[serde(default)]
    pub api_key: String,
    /// 可选：环境变量名，config 里 api_key 为空时才读
    #[serde(default)]
    pub api_key_env: String,
    pub wire: Wire,
    /// 可选兜底：模型名前缀；models 清单未命中时才用
    #[serde(default)]
    pub model_prefixes: Vec<String>,
}

impl Provider {
    pub fn key_present(&self) -> bool {
        if !self.api_key.trim().is_empty() {
            return true;
        }
        !self.api_key_env.trim().is_empty()
            && std::env::var(self.api_key_env.trim()).is_ok_and(|v| !v.trim().is_empty())
    }

    pub fn key_source(&self) -> &'static str {
        if !self.api_key.trim().is_empty() {
            "config"
        } else if !self.api_key_env.trim().is_empty()
            && std::env::var(self.api_key_env.trim()).is_ok_and(|v| !v.trim().is_empty())
        {
            "env"
        } else {
            "none"
        }
    }

    /// 转发时用的密钥；都没有则 None
    pub fn resolve_key(&self) -> Option<String> {
        let from_cfg = self.api_key.trim();
        if !from_cfg.is_empty() {
            return Some(from_cfg.to_string());
        }
        let env_name = self.api_key_env.trim();
        if env_name.is_empty() {
            return None;
        }
        std::env::var(env_name)
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }
}

/// 用户登记的模型：模型 ID → 供应商
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub auto_start: bool,
    /// harness id → 模型名；请求带 `x-orrery-harness` 时覆盖
    #[serde(default)]
    pub routes: BTreeMap<String, String>,
    /// 用户可增删改；空时补默认骨架（无密钥）
    #[serde(default = "default_providers")]
    pub providers: BTreeMap<String, Provider>,
    /// 用户登记的模型清单；路由下拉与 /v1/models 用它
    #[serde(default = "default_models")]
    pub models: Vec<ModelEntry>,
}

fn default_listen() -> String {
    DEFAULT_LISTEN.into()
}

fn empty_provider(base_url: &str, env: &str, wire: Wire, prefixes: &[&str]) -> Provider {
    Provider {
        base_url: base_url.into(),
        api_key: String::new(),
        api_key_env: env.into(),
        wire,
        model_prefixes: prefixes.iter().map(|s| s.to_string()).collect(),
    }
}

fn default_providers() -> BTreeMap<String, Provider> {
    BTreeMap::from([
        (
            "anthropic".into(),
            empty_provider("https://api.anthropic.com/v1", "ANTHROPIC_API_KEY", Wire::Anthropic, &["claude"]),
        ),
        (
            "openai".into(),
            empty_provider("https://api.openai.com/v1", "OPENAI_API_KEY", Wire::Openai, &["gpt", "o3", "o4"]),
        ),
        (
            "moonshot".into(),
            empty_provider("https://api.moonshot.cn/v1", "MOONSHOT_API_KEY", Wire::Openai, &["kimi", "moonshot"]),
        ),
        (
            "deepseek".into(),
            empty_provider("https://api.deepseek.com/v1", "DEEPSEEK_API_KEY", Wire::Openai, &["deepseek"]),
        ),
    ])
}

fn default_models() -> Vec<ModelEntry> {
    vec![
        ModelEntry { id: "claude-opus-4.7".into(), provider: "anthropic".into() },
        ModelEntry { id: "claude-sonnet-4.6".into(), provider: "anthropic".into() },
        ModelEntry { id: "kimi-k3".into(), provider: "moonshot".into() },
        ModelEntry { id: "gpt-5.2".into(), provider: "openai".into() },
        ModelEntry { id: "gpt-5.3-codex".into(), provider: "openai".into() },
        ModelEntry { id: "deepseek-v3.2".into(), provider: "deepseek".into() },
    ]
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            auto_start: false,
            routes: BTreeMap::new(),
            providers: default_providers(),
            models: default_models(),
        }
    }
}

impl ProxyConfig {
    /// 模型 → 供应商：先查用户 models 清单，再按前缀兜底；协议形状必须一致
    pub fn provider_for(&self, model: &str, wire: Wire) -> Option<(&str, &Provider)> {
        let lower = model.trim().to_ascii_lowercase();
        if lower.is_empty() {
            return None;
        }
        if let Some(entry) = self.models.iter().find(|m| m.id.to_ascii_lowercase() == lower) {
            if let Some((name, p)) = self.providers.get_key_value(&entry.provider) {
                if p.wire == wire {
                    return Some((name.as_str(), p));
                }
                // models 指到了错误 wire 的供应商 → 不硬转，交给前缀/报错
            }
        }
        self.providers
            .iter()
            .find(|(_, p)| {
                p.wire == wire
                    && p
                        .model_prefixes
                        .iter()
                        .any(|pre| lower.starts_with(&pre.to_ascii_lowercase()))
            })
            .map(|(name, p)| (name.as_str(), p))
    }

    /// 模型归属（不限 wire），用于 /v1/models 的 owned_by
    pub fn owner_of(&self, model: &str) -> Option<&str> {
        let lower = model.trim().to_ascii_lowercase();
        if let Some(entry) = self.models.iter().find(|m| m.id.to_ascii_lowercase() == lower) {
            return Some(entry.provider.as_str());
        }
        self.providers
            .iter()
            .find(|(_, p)| p.model_prefixes.iter().any(|pre| lower.starts_with(&pre.to_ascii_lowercase())))
            .map(|(name, _)| name.as_str())
    }

    pub fn route_model(&self, harness: Option<&str>) -> Option<&str> {
        harness
            .and_then(|h| self.routes.get(h))
            .or_else(|| self.routes.get("default"))
            .map(String::as_str)
    }

    /// 模型 ID 是否已登记（路由下拉、校验用）
    pub fn has_model(&self, id: &str) -> bool {
        self.models.iter().any(|m| m.id == id)
    }

    pub fn upsert_model(&mut self, id: &str, provider: &str) -> Result<(), String> {
        let id = id.trim();
        let provider = provider.trim();
        if id.is_empty() {
            return Err("model id is empty".into());
        }
        if !self.providers.contains_key(provider) {
            return Err(format!("provider {provider} does not exist"));
        }
        if let Some(m) = self.models.iter_mut().find(|m| m.id == id) {
            m.provider = provider.to_string();
        } else {
            self.models.push(ModelEntry { id: id.into(), provider: provider.into() });
        }
        Ok(())
    }

    pub fn remove_model(&mut self, id: &str) {
        self.models.retain(|m| m.id != id);
        self.routes.retain(|_, v| v != id);
    }
}

pub fn config_path() -> Option<PathBuf> {
    crate::adapters::data_dir().map(|h| h.join("proxy.json"))
}

pub fn load() -> ProxyConfig {
    let mut cfg: ProxyConfig = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if cfg.providers.is_empty() {
        cfg.providers = default_providers();
    }
    // 老配置没有 models：按前缀生成一版骨架，避免路由下拉空空
    if cfg.models.is_empty() {
        cfg.models = default_models();
        // 只保留 providers 里真实存在的
        cfg.models.retain(|m| cfg.providers.contains_key(&m.provider));
    }
    cfg
}

/// 写配置。这个文件里可能有**明文 API Key**，所以：
///
/// - Unix 上权限收成 `0600`（只有本人可读写）。默认 umask 会给 `0644`，
///   在多用户机器上等于把密钥给同机所有人看
/// - 先写临时文件再 rename：进程中途被杀不会留下截断的配置，
///   否则所有供应商和密钥会一起丢失
///
/// Windows 上用户主目录默认只对本人和管理员开放，不另做 ACL 处理
pub fn save(cfg: &ProxyConfig) -> Result<(), String> {
    let path = config_path().ok_or("cannot resolve ~/.orrery")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    // 上次崩溃可能留下旧的临时文件；mode(0o600) 只在新建时生效，所以先删
    let _ = std::fs::remove_file(&tmp);
    write_private(&tmp, json.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // 创建时就带 0600，避免先以 0644 落盘、再 chmod 之间的窗口
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

pub fn ensure_exists() {
    if let Some(path) = config_path() {
        if !path.exists() {
            let _ = save(&ProxyConfig::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_list_beats_prefix() {
        let mut cfg = ProxyConfig::default();
        // 把 kimi-k3 登记到 anthropic（错误 wire）→ Openai 形状应走前缀 moonshot
        cfg.upsert_model("kimi-k3", "anthropic").unwrap();
        assert_eq!(cfg.provider_for("kimi-k3", Wire::Openai).unwrap().0, "moonshot");
        assert_eq!(cfg.provider_for("kimi-k3", Wire::Anthropic).unwrap().0, "anthropic");

        cfg.upsert_model("my-custom", "deepseek").unwrap();
        assert_eq!(cfg.provider_for("my-custom", Wire::Openai).unwrap().0, "deepseek");
        assert!(cfg.provider_for("my-custom", Wire::Anthropic).is_none());
    }

    #[test]
    fn unknown_model_uses_prefix_or_fails() {
        let cfg = ProxyConfig::default();
        assert_eq!(cfg.provider_for("claude-sonnet-4.6", Wire::Anthropic).unwrap().0, "anthropic");
        assert_eq!(cfg.provider_for("deepseek-v3.2", Wire::Openai).unwrap().0, "deepseek");
        assert!(cfg.provider_for("llama-3", Wire::Openai).is_none());
        assert_eq!(cfg.owner_of("gpt-5.2"), Some("openai"));
    }

    #[test]
    fn key_resolution_prefers_config_over_env() {
        let mut p = empty_provider("https://x", "SOME_MISSING_ENV", Wire::Openai, &[]);
        assert!(!p.key_present());
        assert_eq!(p.resolve_key(), None);
        p.api_key = "  sk-plain  ".into();
        assert!(p.key_present());
        assert_eq!(p.key_source(), "config");
        assert_eq!(p.resolve_key().as_deref(), Some("sk-plain"));
    }

    #[test]
    fn route_lookup_prefers_harness_then_default() {
        let mut cfg = ProxyConfig::default();
        cfg.routes.insert("default".into(), "claude-sonnet-4.6".into());
        cfg.routes.insert("kimi".into(), "kimi-k3".into());
        assert_eq!(cfg.route_model(Some("kimi")), Some("kimi-k3"));
        assert_eq!(cfg.route_model(Some("dsh")), Some("claude-sonnet-4.6"));
        assert_eq!(cfg.route_model(None), Some("claude-sonnet-4.6"));
        cfg.routes.remove("default");
        assert_eq!(cfg.route_model(None), None);
    }

    #[test]
    fn remove_model_clears_routes() {
        let mut cfg = ProxyConfig::default();
        cfg.routes.insert("cc".into(), "claude-sonnet-4.6".into());
        cfg.remove_model("claude-sonnet-4.6");
        assert!(!cfg.has_model("claude-sonnet-4.6"));
        assert!(!cfg.routes.contains_key("cc"));
    }

    /// 配置里可能有明文密钥，Unix 上必须只有本人可读（CI 的 Linux/macOS 会跑这条）
    #[cfg(unix)]
    #[test]
    fn config_file_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orrery-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("proxy.json");
        // 模拟上次崩溃留下的、权限过宽的旧文件
        std::fs::write(&file, b"old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&file);
        write_private(&file, b"{\"api_key\":\"sk-test\"}").unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "含密钥的配置文件权限是 {mode:o}，应为 600");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
