//! 代理配置：`~/.openplane/proxy.json`
//!
//! 密钥只存**环境变量名**，永远不写进配置、不回传界面。

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub base_url: String,
    /// 存的是环境变量名，不是密钥本身
    pub api_key_env: String,
    pub wire: Wire,
    /// 归属判断：模型名以任一前缀开头就走这个供应商
    #[serde(default)]
    pub model_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    /// 应用启动时自动拉起代理
    #[serde(default)]
    pub auto_start: bool,
    /// harness id → 模型名；请求带 `x-openplane-harness` 头时覆盖模型。
    /// `default` 是兜底（请求没带头时不覆盖，只有显式写了 `default` 才覆盖）
    #[serde(default)]
    pub routes: BTreeMap<String, String>,
    /// 缺省或为空时补上内置的几家，避免老配置文件（只有 routes）导致无供应商可用
    #[serde(default = "default_providers")]
    pub providers: BTreeMap<String, Provider>,
}

fn default_listen() -> String {
    DEFAULT_LISTEN.into()
}

fn default_providers() -> BTreeMap<String, Provider> {
    let p = |base_url: &str, env: &str, wire: Wire, prefixes: &[&str]| Provider {
        base_url: base_url.into(),
        api_key_env: env.into(),
        wire,
        model_prefixes: prefixes.iter().map(|s| s.to_string()).collect(),
    };
    BTreeMap::from([
        (
            "anthropic".into(),
            p("https://api.anthropic.com/v1", "ANTHROPIC_API_KEY", Wire::Anthropic, &["claude"]),
        ),
        (
            "openai".into(),
            p("https://api.openai.com/v1", "OPENAI_API_KEY", Wire::Openai, &["gpt", "o1", "o3"]),
        ),
        (
            "moonshot".into(),
            p("https://api.moonshot.cn/v1", "MOONSHOT_API_KEY", Wire::Openai, &["kimi", "moonshot"]),
        ),
        (
            "deepseek".into(),
            p("https://api.deepseek.com/v1", "DEEPSEEK_API_KEY", Wire::Openai, &["deepseek"]),
        ),
    ])
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            auto_start: false,
            routes: BTreeMap::new(),
            providers: default_providers(),
        }
    }
}

impl ProxyConfig {
    /// 按模型名选供应商：只认 `model_prefixes` 前缀，且协议形状要对得上。
    /// 匹配不到宁可报错，也不随便挑一家——否则错误信息会指向用户根本没提到的供应商
    pub fn provider_for(&self, model: &str, wire: Wire) -> Option<(&str, &Provider)> {
        let lower = model.to_ascii_lowercase();
        self.providers
            .iter()
            .find(|(_, p)| {
                p.wire == wire && p.model_prefixes.iter().any(|pre| lower.starts_with(&pre.to_ascii_lowercase()))
            })
            .map(|(name, p)| (name.as_str(), p))
    }

    /// 模型归属（不限协议形状），用于 `/v1/models` 的 owned_by
    pub fn owner_of(&self, model: &str) -> Option<&str> {
        let lower = model.to_ascii_lowercase();
        self.providers
            .iter()
            .find(|(_, p)| p.model_prefixes.iter().any(|pre| lower.starts_with(&pre.to_ascii_lowercase())))
            .map(|(name, _)| name.as_str())
    }

    /// 该 harness 要覆盖成的模型
    pub fn route_model(&self, harness: Option<&str>) -> Option<&str> {
        harness
            .and_then(|h| self.routes.get(h))
            .or_else(|| self.routes.get("default"))
            .map(String::as_str)
    }
}

pub fn config_path() -> Option<PathBuf> {
    crate::adapters::home_dir().map(|h| h.join(".openplane").join("proxy.json"))
}

/// 读配置；文件不存在或损坏时返回默认配置（不覆盖用户文件）
pub fn load() -> ProxyConfig {
    let mut cfg: ProxyConfig = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if cfg.providers.is_empty() {
        cfg.providers = default_providers();
    }
    cfg
}

pub fn save(cfg: &ProxyConfig) -> Result<(), String> {
    let path = config_path().ok_or("cannot resolve ~/.openplane")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// 首次运行写出带默认供应商的配置，方便用户照着改
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
    fn provider_matching_by_prefix_and_wire() {
        let cfg = ProxyConfig::default();
        assert_eq!(cfg.provider_for("claude-sonnet-5", Wire::Anthropic).unwrap().0, "anthropic");
        assert_eq!(cfg.provider_for("kimi-k2.5", Wire::Openai).unwrap().0, "moonshot");
        assert_eq!(cfg.provider_for("deepseek-v3.2", Wire::Openai).unwrap().0, "deepseek");
        assert_eq!(cfg.provider_for("gpt-5.3-codex", Wire::Openai).unwrap().0, "openai");
        // 前缀都不匹配 → None，让调用方报明确错误，而不是挑一家顶上
        assert!(cfg.provider_for("llama-3", Wire::Openai).is_none());
        // 协议形状不匹配也不串：gpt 系列没有 Anthropic 形状的供应商
        assert!(cfg.provider_for("gpt-5.2", Wire::Anthropic).is_none());
        assert_eq!(cfg.owner_of("claude-opus-5"), Some("anthropic"));
        assert_eq!(cfg.owner_of("llama-3"), None);
    }

    #[test]
    fn route_lookup_prefers_harness_then_default() {
        let mut cfg = ProxyConfig::default();
        cfg.routes.insert("default".into(), "claude-sonnet-5".into());
        cfg.routes.insert("kimi".into(), "kimi-k2.5".into());
        assert_eq!(cfg.route_model(Some("kimi")), Some("kimi-k2.5"));
        assert_eq!(cfg.route_model(Some("dsh")), Some("claude-sonnet-5"));
        assert_eq!(cfg.route_model(None), Some("claude-sonnet-5"));
        cfg.routes.remove("default");
        assert_eq!(cfg.route_model(None), None);
    }
}
