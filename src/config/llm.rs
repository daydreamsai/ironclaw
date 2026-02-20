use std::path::PathBuf;

use secrecy::SecretString;

use crate::config::helpers::{optional_env, parse_optional_env};
use crate::error::ConfigError;
use crate::settings::Settings;

/// Which LLM backend to use.
///
/// Defaults to `NearAi` to keep IronClaw close to the NEAR ecosystem.
/// Users can override with `LLM_BACKEND` env var to use their own API keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmBackend {
    /// NEAR AI proxy (default) -- session or API key auth
    #[default]
    NearAi,
    /// Direct OpenAI API
    OpenAi,
    /// Direct Anthropic API
    Anthropic,
    /// Local Ollama instance
    Ollama,
    /// Any OpenAI-compatible endpoint (e.g. vLLM, LiteLLM, Together)
    OpenAiCompatible,
    /// Tinfoil private inference
    Tinfoil,
    /// x402 payment-authenticated OpenAI-compatible router
    X402,
}

impl std::str::FromStr for LlmBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nearai" | "near_ai" | "near" => Ok(Self::NearAi),
            "openai" | "open_ai" => Ok(Self::OpenAi),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            "openai_compatible" | "openai-compatible" | "compatible" => Ok(Self::OpenAiCompatible),
            "tinfoil" => Ok(Self::Tinfoil),
            "x402" => Ok(Self::X402),
            _ => Err(format!(
                "invalid LLM backend '{}', expected one of: nearai, openai, anthropic, ollama, openai_compatible, tinfoil, x402",
                s
            )),
        }
    }
}

impl std::fmt::Display for LlmBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NearAi => write!(f, "nearai"),
            Self::OpenAi => write!(f, "openai"),
            Self::Anthropic => write!(f, "anthropic"),
            Self::Ollama => write!(f, "ollama"),
            Self::OpenAiCompatible => write!(f, "openai_compatible"),
            Self::Tinfoil => write!(f, "tinfoil"),
            Self::X402 => write!(f, "x402"),
        }
    }
}

/// Configuration for direct OpenAI API access.
#[derive(Debug, Clone)]
pub struct OpenAiDirectConfig {
    pub api_key: SecretString,
    pub model: String,
}

/// Configuration for direct Anthropic API access.
#[derive(Debug, Clone)]
pub struct AnthropicDirectConfig {
    pub api_key: SecretString,
    pub model: String,
}

/// Configuration for local Ollama.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
}

/// Configuration for any OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: Option<SecretString>,
    pub model: String,
}

/// Configuration for Tinfoil private inference.
#[derive(Debug, Clone)]
pub struct TinfoilConfig {
    pub api_key: SecretString,
    pub model: String,
}

/// Configuration for x402 router integration.
#[derive(Debug, Clone)]
pub struct X402Config {
    pub base_url: String,
    /// EVM JSON-RPC URL used to read ERC-2612 nonces.
    pub rpc_url: String,
    pub private_key: SecretString,
    pub network: String,
    /// Permit cap in token base units (USDC has 6 decimals).
    pub permit_cap: String,
    pub model: String,
}

/// LLM provider configuration.
///
/// NEAR AI remains the default backend. Users can switch to other providers
/// by setting `LLM_BACKEND` (e.g. `openai`, `anthropic`, `ollama`).
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Which backend to use (default: NearAi)
    pub backend: LlmBackend,
    /// NEAR AI config (always populated for NEAR AI embeddings, etc.)
    pub nearai: NearAiConfig,
    /// Direct OpenAI config (populated when backend=openai)
    pub openai: Option<OpenAiDirectConfig>,
    /// Direct Anthropic config (populated when backend=anthropic)
    pub anthropic: Option<AnthropicDirectConfig>,
    /// Ollama config (populated when backend=ollama)
    pub ollama: Option<OllamaConfig>,
    /// OpenAI-compatible config (populated when backend=openai_compatible)
    pub openai_compatible: Option<OpenAiCompatibleConfig>,
    /// Tinfoil config (populated when backend=tinfoil)
    pub tinfoil: Option<TinfoilConfig>,
    /// x402 config (populated when backend=x402)
    pub x402: Option<X402Config>,
}

/// API mode for NEAR AI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NearAiApiMode {
    /// Use the Responses API (chat-api proxy) - session-based auth
    #[default]
    Responses,
    /// Use the Chat Completions API (cloud-api) - API key auth
    ChatCompletions,
}

impl std::str::FromStr for NearAiApiMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "responses" | "response" => Ok(Self::Responses),
            "chat_completions" | "chatcompletions" | "chat" | "completions" => {
                Ok(Self::ChatCompletions)
            }
            _ => Err(format!(
                "invalid API mode '{}', expected 'responses' or 'chat_completions'",
                s
            )),
        }
    }
}

/// NEAR AI chat-api configuration.
#[derive(Debug, Clone)]
pub struct NearAiConfig {
    /// Model to use (e.g., "claude-3-5-sonnet-20241022", "gpt-4o")
    pub model: String,
    /// Cheap/fast model for lightweight tasks (heartbeat, routing, evaluation).
    /// Falls back to the main model if not set.
    pub cheap_model: Option<String>,
    /// Base URL for the NEAR AI API (default: https://private.near.ai).
    pub base_url: String,
    /// Base URL for auth/refresh endpoints (default: https://private.near.ai)
    pub auth_base_url: String,
    /// Path to session file (default: ~/.ironclaw/session.json)
    pub session_path: PathBuf,
    /// API mode: "responses" (chat-api) or "chat_completions" (cloud-api)
    pub api_mode: NearAiApiMode,
    /// API key for cloud-api (required for chat_completions mode)
    pub api_key: Option<SecretString>,
    /// Optional fallback model for failover (default: None).
    /// When set, a secondary provider is created with this model and wrapped
    /// in a `FailoverProvider` so transient errors on the primary model
    /// automatically fall through to the fallback.
    pub fallback_model: Option<String>,
    /// Maximum number of retries for transient errors (default: 3).
    /// With the default of 3, the provider makes up to 4 total attempts
    /// (1 initial + 3 retries) before giving up.
    pub max_retries: u32,
    /// Consecutive transient failures before the circuit breaker opens.
    /// None = disabled (default). E.g. 5 means after 5 consecutive failures
    /// all requests are rejected until recovery timeout elapses.
    pub circuit_breaker_threshold: Option<u32>,
    /// How long (seconds) the circuit stays open before allowing a probe (default: 30).
    pub circuit_breaker_recovery_secs: u64,
    /// Enable in-memory response caching for `complete()` calls.
    /// Saves tokens on repeated prompts within a session. Default: false.
    pub response_cache_enabled: bool,
    /// TTL in seconds for cached responses (default: 3600 = 1 hour).
    pub response_cache_ttl_secs: u64,
    /// Max cached responses before LRU eviction (default: 1000).
    pub response_cache_max_entries: usize,
    /// Cooldown duration in seconds for the failover provider (default: 300).
    /// When a provider accumulates enough consecutive failures it is skipped
    /// for this many seconds.
    pub failover_cooldown_secs: u64,
    /// Number of consecutive retryable failures before a provider enters
    /// cooldown (default: 3).
    pub failover_cooldown_threshold: u32,
}

impl LlmConfig {
    pub(crate) fn resolve(settings: &Settings) -> Result<Self, ConfigError> {
        // Determine backend: env var > settings > default (NearAi)
        let backend: LlmBackend = if let Some(b) = optional_env("LLM_BACKEND")? {
            b.parse().map_err(|e| ConfigError::InvalidValue {
                key: "LLM_BACKEND".to_string(),
                message: e,
            })?
        } else if let Some(ref b) = settings.llm_backend {
            match b.parse() {
                Ok(backend) => backend,
                Err(e) => {
                    tracing::warn!(
                        "Invalid llm_backend '{}' in settings: {}. Using default NearAi.",
                        b,
                        e
                    );
                    LlmBackend::NearAi
                }
            }
        } else {
            LlmBackend::NearAi
        };

        // Resolve NEAR AI config only when backend is NearAi (or when explicitly configured)
        let nearai_api_key = optional_env("NEARAI_API_KEY")?.map(SecretString::from);

        let api_mode = if let Some(mode_str) = optional_env("NEARAI_API_MODE")? {
            mode_str.parse().map_err(|e| ConfigError::InvalidValue {
                key: "NEARAI_API_MODE".to_string(),
                message: e,
            })?
        } else if nearai_api_key.is_some() {
            NearAiApiMode::ChatCompletions
        } else {
            NearAiApiMode::Responses
        };

        let nearai = NearAiConfig {
            model: optional_env("NEARAI_MODEL")?
                .or_else(|| settings.selected_model.clone())
                .unwrap_or_else(|| {
                    "fireworks::accounts/fireworks/models/llama4-maverick-instruct-basic"
                        .to_string()
                }),
            cheap_model: optional_env("NEARAI_CHEAP_MODEL")?,
            base_url: optional_env("NEARAI_BASE_URL")?
                .unwrap_or_else(|| "https://private.near.ai".to_string()),
            auth_base_url: optional_env("NEARAI_AUTH_URL")?
                .unwrap_or_else(|| "https://private.near.ai".to_string()),
            session_path: optional_env("NEARAI_SESSION_PATH")?
                .map(PathBuf::from)
                .unwrap_or_else(default_session_path),
            api_mode,
            api_key: nearai_api_key,
            fallback_model: optional_env("NEARAI_FALLBACK_MODEL")?,
            max_retries: parse_optional_env("NEARAI_MAX_RETRIES", 3)?,
            circuit_breaker_threshold: optional_env("CIRCUIT_BREAKER_THRESHOLD")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "CIRCUIT_BREAKER_THRESHOLD".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?,
            circuit_breaker_recovery_secs: parse_optional_env("CIRCUIT_BREAKER_RECOVERY_SECS", 30)?,
            response_cache_enabled: parse_optional_env("RESPONSE_CACHE_ENABLED", false)?,
            response_cache_ttl_secs: parse_optional_env("RESPONSE_CACHE_TTL_SECS", 3600)?,
            response_cache_max_entries: parse_optional_env("RESPONSE_CACHE_MAX_ENTRIES", 1000)?,
            failover_cooldown_secs: parse_optional_env("LLM_FAILOVER_COOLDOWN_SECS", 300)?,
            failover_cooldown_threshold: parse_optional_env("LLM_FAILOVER_THRESHOLD", 3)?,
        };

        // Resolve provider-specific configs based on backend
        let openai = if backend == LlmBackend::OpenAi {
            let api_key = optional_env("OPENAI_API_KEY")?
                .map(SecretString::from)
                .ok_or_else(|| ConfigError::MissingRequired {
                    key: "OPENAI_API_KEY".to_string(),
                    hint: "Set OPENAI_API_KEY when LLM_BACKEND=openai".to_string(),
                })?;
            let model = optional_env("OPENAI_MODEL")?.unwrap_or_else(|| "gpt-4o".to_string());
            Some(OpenAiDirectConfig { api_key, model })
        } else {
            None
        };

        let anthropic = if backend == LlmBackend::Anthropic {
            let api_key = optional_env("ANTHROPIC_API_KEY")?
                .map(SecretString::from)
                .ok_or_else(|| ConfigError::MissingRequired {
                    key: "ANTHROPIC_API_KEY".to_string(),
                    hint: "Set ANTHROPIC_API_KEY when LLM_BACKEND=anthropic".to_string(),
                })?;
            let model = optional_env("ANTHROPIC_MODEL")?
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
            Some(AnthropicDirectConfig { api_key, model })
        } else {
            None
        };

        let ollama = if backend == LlmBackend::Ollama {
            let base_url = optional_env("OLLAMA_BASE_URL")?
                .or_else(|| settings.ollama_base_url.clone())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let model = optional_env("OLLAMA_MODEL")?.unwrap_or_else(|| "llama3".to_string());
            Some(OllamaConfig { base_url, model })
        } else {
            None
        };

        let openai_compatible = if backend == LlmBackend::OpenAiCompatible {
            let base_url = optional_env("LLM_BASE_URL")?
                .or_else(|| settings.openai_compatible_base_url.clone())
                .ok_or_else(|| ConfigError::MissingRequired {
                    key: "LLM_BASE_URL".to_string(),
                    hint: "Set LLM_BASE_URL when LLM_BACKEND=openai_compatible".to_string(),
                })?;
            let api_key = optional_env("LLM_API_KEY")?.map(SecretString::from);
            let model = optional_env("LLM_MODEL")?
                .or_else(|| settings.selected_model.clone())
                .unwrap_or_else(|| "default".to_string());
            Some(OpenAiCompatibleConfig {
                base_url,
                api_key,
                model,
            })
        } else {
            None
        };

        let tinfoil = if backend == LlmBackend::Tinfoil {
            let api_key = optional_env("TINFOIL_API_KEY")?
                .map(SecretString::from)
                .ok_or_else(|| ConfigError::MissingRequired {
                    key: "TINFOIL_API_KEY".to_string(),
                    hint: "Set TINFOIL_API_KEY when LLM_BACKEND=tinfoil".to_string(),
                })?;
            let model = optional_env("TINFOIL_MODEL")?.unwrap_or_else(|| "kimi-k2-5".to_string());
            Some(TinfoilConfig { api_key, model })
        } else {
            None
        };

        let x402 = if backend == LlmBackend::X402 {
            let base_url = optional_env("X402_BASE_URL")?
                .or_else(|| settings.x402_base_url.clone())
                .ok_or_else(|| ConfigError::MissingRequired {
                    key: "X402_BASE_URL".to_string(),
                    hint: "Set X402_BASE_URL when LLM_BACKEND=x402".to_string(),
                })?;
            let rpc_url = optional_env("X402_RPC_URL")?
                .or_else(|| settings.x402_rpc_url.clone())
                .unwrap_or_else(|| "https://mainnet.base.org".to_string());

            let raw_key =
                optional_env("X402_PRIVATE_KEY")?.ok_or_else(|| ConfigError::MissingRequired {
                    key: "X402_PRIVATE_KEY".to_string(),
                    hint: "Set X402_PRIVATE_KEY when LLM_BACKEND=x402".to_string(),
                })?;
            let private_key =
                normalize_private_key(&raw_key).ok_or_else(|| ConfigError::InvalidValue {
                    key: "X402_PRIVATE_KEY".to_string(),
                    message: "must be 0x-prefixed 64 hex characters".to_string(),
                })?;

            let network = optional_env("X402_NETWORK")?
                .or_else(|| settings.x402_network.clone())
                .unwrap_or_else(|| "eip155:8453".to_string());
            if network != "eip155:8453" {
                return Err(ConfigError::InvalidValue {
                    key: "X402_NETWORK".to_string(),
                    message: "only eip155:8453 (Base) is supported".to_string(),
                });
            }

            let permit_cap = optional_env("X402_PERMIT_CAP")?
                .or_else(|| settings.x402_permit_cap.clone())
                .unwrap_or_else(|| "10000000".to_string());
            let cap_valid = permit_cap.parse::<u128>().map(|n| n > 0).unwrap_or(false);
            if !cap_valid {
                return Err(ConfigError::InvalidValue {
                    key: "X402_PERMIT_CAP".to_string(),
                    message: "must be a positive integer in base units".to_string(),
                });
            }

            let model = optional_env("LLM_MODEL")?
                .or_else(|| settings.selected_model.clone())
                .unwrap_or_else(|| "default".to_string());

            Some(X402Config {
                base_url,
                rpc_url,
                private_key: SecretString::from(private_key),
                network,
                permit_cap,
                model,
            })
        } else {
            None
        };

        Ok(Self {
            backend,
            nearai,
            openai,
            anthropic,
            ollama,
            openai_compatible,
            tinfoil,
            x402,
        })
    }
}

/// Normalize and validate a private key as 0x-prefixed 64 hex chars.
fn normalize_private_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let normalized = if let Some(rest) = trimmed.strip_prefix("0X") {
        format!("0x{rest}")
    } else {
        trimmed.to_string()
    };

    if normalized.len() != 66 || !normalized.starts_with("0x") {
        return None;
    }
    if normalized
        .as_bytes()
        .iter()
        .skip(2)
        .all(|b| b.is_ascii_hexdigit())
    {
        Some(normalized)
    } else {
        None
    }
}

/// Get the default session file path (~/.ironclaw/session.json).
fn default_session_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ironclaw")
        .join("session.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ENV_MUTEX, clear_injected_vars_for_tests};
    use crate::settings::Settings;

    /// Clear all openai-compatible-related env vars.
    fn clear_openai_compatible_env() {
        // SAFETY: Only called under ENV_MUTEX in tests.
        unsafe {
            std::env::remove_var("LLM_BACKEND");
            std::env::remove_var("LLM_BASE_URL");
            std::env::remove_var("LLM_MODEL");
        }
        clear_injected_vars_for_tests();
    }

    /// Clear all x402-related env vars.
    fn clear_x402_env() {
        // SAFETY: Only called under ENV_MUTEX in tests.
        unsafe {
            std::env::remove_var("LLM_BACKEND");
            std::env::remove_var("X402_BASE_URL");
            std::env::remove_var("X402_PRIVATE_KEY");
            std::env::remove_var("X402_RPC_URL");
            std::env::remove_var("X402_NETWORK");
            std::env::remove_var("X402_PERMIT_CAP");
            std::env::remove_var("LLM_MODEL");
        }
        clear_injected_vars_for_tests();
    }

    #[test]
    fn openai_compatible_uses_selected_model_when_llm_model_unset() {
        let _guard = ENV_MUTEX.lock().expect("env mutex poisoned");
        clear_openai_compatible_env();

        let settings = Settings {
            llm_backend: Some("openai_compatible".to_string()),
            openai_compatible_base_url: Some("https://openrouter.ai/api/v1".to_string()),
            selected_model: Some("openai/gpt-5.1-codex".to_string()),
            ..Default::default()
        };

        let cfg = LlmConfig::resolve(&settings).expect("resolve should succeed");
        let compat = cfg
            .openai_compatible
            .expect("openai-compatible config should be present");

        assert_eq!(compat.model, "openai/gpt-5.1-codex");
    }

    #[test]
    fn openai_compatible_llm_model_env_overrides_selected_model() {
        let _guard = ENV_MUTEX.lock().expect("env mutex poisoned");
        clear_openai_compatible_env();
        // SAFETY: Under ENV_MUTEX.
        unsafe {
            std::env::set_var("LLM_MODEL", "openai/gpt-5-codex");
        }

        let settings = Settings {
            llm_backend: Some("openai_compatible".to_string()),
            openai_compatible_base_url: Some("https://openrouter.ai/api/v1".to_string()),
            selected_model: Some("openai/gpt-5.1-codex".to_string()),
            ..Default::default()
        };

        let cfg = LlmConfig::resolve(&settings).expect("resolve should succeed");
        let compat = cfg
            .openai_compatible
            .expect("openai-compatible config should be present");

        assert_eq!(compat.model, "openai/gpt-5-codex");

        // SAFETY: Under ENV_MUTEX.
        unsafe {
            std::env::remove_var("LLM_MODEL");
        }
    }

    #[test]
    fn x402_resolves_from_env() {
        let _guard = ENV_MUTEX.lock().expect("env mutex poisoned");
        clear_x402_env();

        // SAFETY: Under ENV_MUTEX.
        unsafe {
            std::env::set_var("LLM_BACKEND", "x402");
            std::env::set_var("X402_BASE_URL", "https://router.example.com");
            std::env::set_var(
                "X402_PRIVATE_KEY",
                "0x1111111111111111111111111111111111111111111111111111111111111111",
            );
            std::env::set_var("X402_NETWORK", "eip155:8453");
            std::env::set_var("X402_PERMIT_CAP", "10000000");
            std::env::set_var("LLM_MODEL", "x402/model");
        }

        let settings = Settings::default();
        let cfg = LlmConfig::resolve(&settings).expect("resolve should succeed");
        let x402 = cfg.x402.expect("x402 config should be present");

        assert_eq!(cfg.backend, LlmBackend::X402);
        assert_eq!(x402.base_url, "https://router.example.com");
        assert_eq!(x402.rpc_url, "https://mainnet.base.org");
        assert_eq!(x402.network, "eip155:8453");
        assert_eq!(x402.permit_cap, "10000000");
        assert_eq!(x402.model, "x402/model");

        clear_x402_env();
    }

    #[test]
    fn x402_missing_private_key_fails() {
        let _guard = ENV_MUTEX.lock().expect("env mutex poisoned");
        clear_x402_env();

        // SAFETY: Under ENV_MUTEX.
        unsafe {
            std::env::set_var("LLM_BACKEND", "x402");
            std::env::set_var("X402_BASE_URL", "https://router.example.com");
        }

        let settings = Settings::default();
        let err = LlmConfig::resolve(&settings).expect_err("resolve should fail");

        match err {
            ConfigError::MissingRequired { key, .. } => assert_eq!(key, "X402_PRIVATE_KEY"),
            other => panic!("unexpected error type: {other:?}"),
        }

        clear_x402_env();
    }

    #[test]
    fn x402_resolves_from_settings_when_env_unset() {
        let _guard = ENV_MUTEX.lock().expect("env mutex poisoned");
        clear_x402_env();

        let settings = Settings {
            llm_backend: Some("x402".to_string()),
            x402_base_url: Some("https://router.example.com".to_string()),
            x402_rpc_url: Some("https://mainnet.base.org".to_string()),
            x402_network: Some("eip155:8453".to_string()),
            x402_permit_cap: Some("10000000".to_string()),
            selected_model: Some("x402/model-from-settings".to_string()),
            ..Default::default()
        };

        // SAFETY: Under ENV_MUTEX.
        unsafe {
            std::env::set_var(
                "X402_PRIVATE_KEY",
                "0x1111111111111111111111111111111111111111111111111111111111111111",
            );
        }

        let cfg = LlmConfig::resolve(&settings).expect("resolve should succeed");
        let x402 = cfg.x402.expect("x402 config should be present");

        assert_eq!(cfg.backend, LlmBackend::X402);
        assert_eq!(x402.base_url, "https://router.example.com");
        assert_eq!(x402.rpc_url, "https://mainnet.base.org");
        assert_eq!(x402.network, "eip155:8453");
        assert_eq!(x402.permit_cap, "10000000");
        assert_eq!(x402.model, "x402/model-from-settings");

        clear_x402_env();
    }
}
