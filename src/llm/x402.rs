//! x402 router provider.
//!
//! Outbound requests are sent to an OpenAI-compatible router endpoint and
//! authenticated with an ERC-2612 permit payload in a payment header.

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::config::X402Config;
use crate::error::LlmError;
use crate::llm::provider::{
    ChatMessage, CompletionRequest, CompletionResponse, FinishReason, LlmProvider, ModelMetadata,
    Role, ToolCall, ToolCompletionRequest, ToolCompletionResponse,
};

const PROVIDER_NAME: &str = "x402";
const DEFAULT_PAYMENT_HEADER: &str = "PAYMENT-SIGNATURE";
const PAYMENT_REQUIRED_HEADER: &str = "PAYMENT-REQUIRED";
const BASE_NETWORK: &str = "eip155:8453";
const BASE_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
const DEFAULT_TOKEN_NAME: &str = "USD Coin";
const DEFAULT_TOKEN_VERSION: &str = "2";
const DEFAULT_PERMIT_VALIDITY_SECS: u64 = 3600;

/// x402-backed OpenAI-compatible provider.
pub struct X402Provider {
    client: Client,
    config: X402Config,
    active_model: std::sync::RwLock<String>,
    router_config_cache: std::sync::RwLock<Option<RouterPaymentConfig>>,
    permit_cache: std::sync::RwLock<Option<CachedPermit>>,
}

#[derive(Debug, Clone)]
struct RouterPaymentConfig {
    network: String,
    asset: String,
    pay_to: String,
    facilitator_signer: String,
    token_name: String,
    token_version: String,
    payment_header: String,
}

#[derive(Debug, Clone)]
struct CachedPermit {
    payment_sig: String,
    deadline: u64,
    max_value: String,
    network: String,
    asset: String,
    pay_to: String,
}

impl X402Provider {
    pub fn new(config: X402Config) -> Result<Self, LlmError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        Ok(Self {
            client,
            active_model: std::sync::RwLock::new(config.model.clone()),
            router_config_cache: std::sync::RwLock::new(None),
            permit_cache: std::sync::RwLock::new(None),
            config,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/v1/{}",
            self.config.base_url,
            path.trim_start_matches('/')
        )
    }

    async fn send_chat_request<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        body: &T,
    ) -> Result<R, LlmError> {
        let url = self.api_url("chat/completions");
        let mut cap_override: Option<String> = None;
        let mut min_deadline_exclusive: Option<u64> = None;

        for attempt in 0..=1 {
            let router_config = self.get_router_config().await?;
            let permit = self
                .get_or_create_permit(
                    &router_config,
                    cap_override.as_deref(),
                    min_deadline_exclusive,
                )
                .await?;

            let response = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header(
                    router_config.payment_header.as_str(),
                    permit.payment_sig.as_str(),
                )
                .json(body)
                .send()
                .await
                .map_err(|e| LlmError::RequestFailed {
                    provider: PROVIDER_NAME.to_string(),
                    reason: e.to_string(),
                })?;

            let status = response.status();

            if status.is_success() {
                let response_text = response.text().await.unwrap_or_default();
                return serde_json::from_str(&response_text).map_err(|e| {
                    LlmError::InvalidResponse {
                        provider: PROVIDER_NAME.to_string(),
                        reason: format!("JSON parse error: {}. Raw: {}", e, response_text),
                    }
                });
            }

            if (status.as_u16() == 401 || status.as_u16() == 402) && attempt == 0 {
                let payment_required = response
                    .headers()
                    .get(PAYMENT_REQUIRED_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(ToOwned::to_owned);
                let response_text = response.text().await.unwrap_or_default();

                if let Some(payment_required) = payment_required
                    && let Some(challenge) = decode_payment_required_header(&payment_required)
                    && let Some(requirement) = challenge.accepts.first()
                {
                    let updated = apply_payment_requirement(&router_config, requirement)?;
                    self.set_router_config(updated);

                    cap_override = extract_required_max_amount(requirement);
                    if let Some(required_cap) = cap_override.as_deref() {
                        self.ensure_cap_within_limit(required_cap)?;
                    }

                    self.invalidate_permit_cache();
                    min_deadline_exclusive = Some(permit.deadline);
                    continue;
                }

                return Err(LlmError::RequestFailed {
                    provider: PROVIDER_NAME.to_string(),
                    reason: format!("HTTP {}: {}", status, response_text),
                });
            }

            let response_text = response.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), response_text));
        }

        Err(LlmError::RequestFailed {
            provider: PROVIDER_NAME.to_string(),
            reason: "x402 retry loop exited unexpectedly".to_string(),
        })
    }

    async fn fetch_models(&self) -> Result<Vec<ApiModelEntry>, LlmError> {
        // Intentionally unsigned per product requirement.
        let response = self
            .client
            .get(self.api_url("models"))
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("Failed to fetch models: {}", e),
            })?;

        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("HTTP {}: {}", status, response_text),
            });
        }

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ApiModelEntry>,
        }

        let parsed: ModelsResponse =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("JSON parse error: {}", e),
            })?;

        Ok(parsed.data)
    }

    async fn get_router_config(&self) -> Result<RouterPaymentConfig, LlmError> {
        if let Some(cached) = self
            .router_config_cache
            .read()
            .expect("router_config_cache lock poisoned")
            .clone()
        {
            return Ok(cached);
        }

        let response = self
            .client
            .get(self.api_url("config"))
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("Failed to fetch /v1/config: {}", e),
            })?;

        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!(
                    "Failed to fetch /v1/config (HTTP {}): {}",
                    status, response_text
                ),
            });
        }

        let config_response: RouterConfigResponse =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("Invalid /v1/config response: {}", e),
            })?;

        let parsed = parse_router_config(config_response)?;
        self.set_router_config(parsed.clone());
        Ok(parsed)
    }

    fn set_router_config(&self, config: RouterPaymentConfig) {
        *self
            .router_config_cache
            .write()
            .expect("router_config_cache lock poisoned") = Some(config);
    }

    fn invalidate_permit_cache(&self) {
        *self
            .permit_cache
            .write()
            .expect("permit_cache lock poisoned") = None;
    }

    fn ensure_cap_within_limit(&self, required_cap: &str) -> Result<(), LlmError> {
        let required = parse_u128_dec(required_cap).ok_or_else(|| LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!("invalid required cap in PAYMENT-REQUIRED: {required_cap}"),
        })?;
        let configured =
            parse_u128_dec(&self.config.permit_cap).ok_or_else(|| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: "invalid configured X402_PERMIT_CAP".to_string(),
            })?;

        if required > configured {
            return Err(LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!(
                    "required cap {} exceeds configured X402_PERMIT_CAP {}",
                    required_cap, self.config.permit_cap
                ),
            });
        }
        Ok(())
    }

    fn resolve_permit_cap(&self, cap_override: Option<&str>) -> Result<String, LlmError> {
        match cap_override {
            Some(value) => {
                self.ensure_cap_within_limit(value)?;
                Ok(value.to_string())
            }
            None => Ok(self.config.permit_cap.clone()),
        }
    }

    async fn get_or_create_permit(
        &self,
        router: &RouterPaymentConfig,
        cap_override: Option<&str>,
        min_deadline_exclusive: Option<u64>,
    ) -> Result<CachedPermit, LlmError> {
        let permit_cap = self.resolve_permit_cap(cap_override)?;
        let now = unix_timestamp_secs();

        if let Some(cached) = self
            .permit_cache
            .read()
            .expect("permit_cache lock poisoned")
            .clone()
        {
            let deadline_ok = min_deadline_exclusive
                .map(|min| cached.deadline > min)
                .unwrap_or(true);
            if cached.network == router.network
                && cached.asset == router.asset
                && cached.pay_to == router.pay_to
                && cached.max_value == permit_cap
                && cached.deadline > now + 5
                && deadline_ok
            {
                return Ok(cached);
            }
        }

        let fresh = self
            .create_permit(router, &permit_cap, min_deadline_exclusive)
            .await?;
        *self
            .permit_cache
            .write()
            .expect("permit_cache lock poisoned") = Some(fresh.clone());
        Ok(fresh)
    }

    async fn create_permit(
        &self,
        router: &RouterPaymentConfig,
        permit_cap: &str,
        min_deadline_exclusive: Option<u64>,
    ) -> Result<CachedPermit, LlmError> {
        let secp = Secp256k1::new();
        let private_key = parse_private_key(self.config.private_key.expose_secret())?;
        let owner = owner_address(&secp, &private_key);

        let nonce = self.fetch_permit_nonce(&router.asset, &owner).await?;

        let mut deadline = unix_timestamp_secs().saturating_add(DEFAULT_PERMIT_VALIDITY_SECS);
        if let Some(min_deadline) = min_deadline_exclusive
            && deadline <= min_deadline
        {
            deadline = min_deadline.saturating_add(1);
        }

        let chain_id =
            chain_id_from_caip2(&router.network).ok_or_else(|| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: format!(
                    "invalid router network '{}': expected CAIP-2 eip155:*",
                    router.network
                ),
            })?;

        let digest = build_eip712_permit_digest(
            &router.token_name,
            &router.token_version,
            chain_id,
            &router.asset,
            &owner,
            &router.facilitator_signer,
            permit_cap,
            nonce,
            deadline,
        )?;

        let signature = sign_digest(&secp, &private_key, digest)?;

        let payload = PaymentPayload {
            x402_version: 2,
            accepted: PaymentAccepted {
                scheme: "upto".to_string(),
                network: router.network.clone(),
                asset: router.asset.clone(),
                pay_to: router.pay_to.clone(),
                extra: PaymentAcceptedExtra {
                    name: router.token_name.clone(),
                    version: router.token_version.clone(),
                },
            },
            payload: PaymentPayloadInner {
                authorization: PaymentAuthorization {
                    from: owner.clone(),
                    to: router.facilitator_signer.clone(),
                    value: permit_cap.to_string(),
                    valid_before: deadline.to_string(),
                    nonce: nonce.to_string(),
                },
                signature,
            },
        };

        let payload_json =
            serde_json::to_string(&payload).map_err(|e| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("failed to encode payment payload: {}", e),
            })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(payload_json);

        Ok(CachedPermit {
            payment_sig: encoded,
            deadline,
            max_value: permit_cap.to_string(),
            network: router.network.clone(),
            asset: router.asset.clone(),
            pay_to: router.pay_to.clone(),
        })
    }

    async fn fetch_permit_nonce(&self, asset: &str, owner: &str) -> Result<u128, LlmError> {
        let owner_bytes = parse_address(owner)?;
        let selector = keccak256(b"nonces(address)");

        let mut data = Vec::with_capacity(4 + 32);
        data.extend_from_slice(&selector[..4]);
        data.extend_from_slice(&[0_u8; 12]);
        data.extend_from_slice(&owner_bytes);

        let call_data = format!("0x{}", hex::encode(data));

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [
                {
                    "to": asset,
                    "data": call_data
                },
                "latest"
            ]
        });

        let response = self
            .client
            .post(&self.config.rpc_url)
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("failed to fetch permit nonce: {}", e),
            })?;

        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("nonce RPC failed (HTTP {}): {}", status, response_text),
            });
        }

        let rpc: RpcResponse =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("invalid nonce RPC response: {}", e),
            })?;

        if let Some(err) = rpc.error {
            return Err(LlmError::RequestFailed {
                provider: PROVIDER_NAME.to_string(),
                reason: format!("nonce RPC error {}: {}", err.code, err.message),
            });
        }

        let result = rpc.result.ok_or_else(|| LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: "nonce RPC missing result".to_string(),
        })?;

        parse_hex_u128(&result).ok_or_else(|| LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!("invalid nonce hex '{}': expected uint256 hex", result),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ApiModelEntry {
    id: String,
    #[serde(default)]
    context_length: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RouterConfigResponse {
    #[serde(default)]
    networks: Vec<RouterNetworkConfig>,
    #[serde(default)]
    payment_header: Option<String>,
    #[serde(default)]
    eip712_config: Option<RouterEip712Config>,
}

#[derive(Debug, Deserialize)]
struct RouterNetworkConfig {
    #[serde(default)]
    network_id: Option<String>,
    #[serde(default)]
    asset: Option<RouterAssetConfig>,
    #[serde(default)]
    pay_to: Option<String>,
    #[serde(default)]
    active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RouterAssetConfig {
    #[serde(default)]
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RouterEip712Config {
    #[serde(default)]
    domain_name: Option<String>,
    #[serde(default)]
    domain_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaymentRequiredHeader {
    #[serde(default)]
    accepts: Vec<PaymentRequirement>,
}

#[derive(Debug, Clone, Deserialize)]
struct PaymentRequirement {
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default, rename = "payTo")]
    pay_to: Option<String>,
    #[serde(default, rename = "pay_to")]
    pay_to_alt: Option<String>,
    #[serde(default)]
    extra: Option<PaymentRequirementExtra>,
}

#[derive(Debug, Clone, Deserialize)]
struct PaymentRequirementExtra {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "maxAmountRequired")]
    max_amount_required: Option<String>,
    #[serde(default, rename = "max_amount_required")]
    max_amount_required_alt: Option<String>,
    #[serde(default, rename = "maxAmount")]
    max_amount: Option<String>,
    #[serde(default, rename = "max_amount")]
    max_amount_alt: Option<String>,
}

#[derive(Debug, Serialize)]
struct PaymentPayload {
    #[serde(rename = "x402Version")]
    x402_version: u8,
    accepted: PaymentAccepted,
    payload: PaymentPayloadInner,
}

#[derive(Debug, Serialize)]
struct PaymentAccepted {
    scheme: String,
    network: String,
    asset: String,
    #[serde(rename = "payTo")]
    pay_to: String,
    extra: PaymentAcceptedExtra,
}

#[derive(Debug, Serialize)]
struct PaymentAcceptedExtra {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct PaymentPayloadInner {
    authorization: PaymentAuthorization,
    signature: String,
}

#[derive(Debug, Serialize)]
struct PaymentAuthorization {
    from: String,
    to: String,
    value: String,
    #[serde(rename = "validBefore")]
    valid_before: String,
    nonce: String,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<RpcErrorObject>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorObject {
    code: i64,
    message: String,
}

fn parse_router_config(response: RouterConfigResponse) -> Result<RouterPaymentConfig, LlmError> {
    let network = response
        .networks
        .iter()
        .find(|n| {
            n.network_id.as_deref() == Some(BASE_NETWORK)
                && n.active.unwrap_or(true)
                && n.pay_to.as_deref().is_some()
        })
        .or_else(|| {
            response
                .networks
                .iter()
                .find(|n| n.active.unwrap_or(true) && n.pay_to.as_deref().is_some())
        })
        .ok_or_else(|| LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: "router /v1/config missing active network payment config".to_string(),
        })?;

    let network_id = network
        .network_id
        .clone()
        .unwrap_or_else(|| BASE_NETWORK.to_string());
    if network_id != BASE_NETWORK {
        return Err(LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!(
                "unsupported network '{}': only {} is supported",
                network_id, BASE_NETWORK
            ),
        });
    }

    let asset = network
        .asset
        .as_ref()
        .and_then(|a| a.address.clone())
        .unwrap_or_else(|| BASE_USDC.to_string());
    let normalized_asset = normalize_address(&asset).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid asset address in /v1/config: {asset}"),
    })?;

    if normalized_asset != BASE_USDC {
        return Err(LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!("unsupported asset '{}': only Base USDC is supported", asset),
        });
    }

    let pay_to_raw = network
        .pay_to
        .clone()
        .ok_or_else(|| LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: "router /v1/config missing pay_to".to_string(),
        })?;
    let pay_to = normalize_address(&pay_to_raw).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid pay_to address in /v1/config: {pay_to_raw}"),
    })?;

    let payment_header = response
        .payment_header
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(DEFAULT_PAYMENT_HEADER)
        .to_string();

    let token_name = response
        .eip712_config
        .as_ref()
        .and_then(|cfg| cfg.domain_name.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TOKEN_NAME.to_string());
    let token_version = response
        .eip712_config
        .as_ref()
        .and_then(|cfg| cfg.domain_version.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TOKEN_VERSION.to_string());

    Ok(RouterPaymentConfig {
        network: network_id,
        asset: normalized_asset,
        pay_to: pay_to.clone(),
        facilitator_signer: pay_to,
        token_name,
        token_version,
        payment_header,
    })
}

fn decode_payment_required_header(value: &str) -> Option<PaymentRequiredHeader> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(value))
        .ok()?;

    serde_json::from_slice::<PaymentRequiredHeader>(&decoded).ok()
}

fn extract_required_max_amount(requirement: &PaymentRequirement) -> Option<String> {
    let extra = requirement.extra.as_ref()?;
    let candidate = extra
        .max_amount_required
        .as_deref()
        .or(extra.max_amount_required_alt.as_deref())
        .or(extra.max_amount.as_deref())
        .or(extra.max_amount_alt.as_deref())?
        .trim();

    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn requirement_pay_to(requirement: &PaymentRequirement) -> Option<String> {
    requirement
        .pay_to
        .as_deref()
        .or(requirement.pay_to_alt.as_deref())
        .map(str::to_string)
}

fn apply_payment_requirement(
    base: &RouterPaymentConfig,
    requirement: &PaymentRequirement,
) -> Result<RouterPaymentConfig, LlmError> {
    let network = requirement
        .network
        .clone()
        .unwrap_or_else(|| base.network.clone());
    if network != BASE_NETWORK {
        return Err(LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!(
                "unsupported challenge network '{}': only {} is supported",
                network, BASE_NETWORK
            ),
        });
    }

    let asset = requirement
        .asset
        .clone()
        .unwrap_or_else(|| base.asset.clone());
    let normalized_asset = normalize_address(&asset).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid challenge asset address: {asset}"),
    })?;
    if normalized_asset != BASE_USDC {
        return Err(LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: format!(
                "unsupported challenge asset '{}': only Base USDC is supported",
                asset
            ),
        });
    }

    let pay_to_raw = requirement_pay_to(requirement).unwrap_or_else(|| base.pay_to.clone());
    let pay_to = normalize_address(&pay_to_raw).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid challenge payTo/pay_to address: {pay_to_raw}"),
    })?;

    let token_name = requirement
        .extra
        .as_ref()
        .and_then(|e| e.name.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| base.token_name.clone());
    let token_version = requirement
        .extra
        .as_ref()
        .and_then(|e| e.version.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| base.token_version.clone());

    Ok(RouterPaymentConfig {
        network,
        asset: normalized_asset,
        pay_to: pay_to.clone(),
        facilitator_signer: pay_to,
        token_name,
        token_version,
        payment_header: base.payment_header.clone(),
    })
}

fn map_http_error(status_code: u16, response_text: String) -> LlmError {
    if status_code == 401 || status_code == 402 || status_code == 403 {
        return LlmError::AuthFailed {
            provider: PROVIDER_NAME.to_string(),
        };
    }
    if status_code == 429 {
        return LlmError::RateLimited {
            provider: PROVIDER_NAME.to_string(),
            retry_after: None,
        };
    }
    LlmError::RequestFailed {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("HTTP {}: {}", status_code, response_text),
    }
}

fn chain_id_from_caip2(network: &str) -> Option<u64> {
    let (_, chain) = network.split_once(':')?;
    chain.parse().ok()
}

fn parse_private_key(private_key: &str) -> Result<SecretKey, LlmError> {
    let trimmed = private_key.trim();
    let hex_key = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let bytes = hex::decode(hex_key).map_err(|_| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: "invalid x402 private key hex".to_string(),
    })?;

    if bytes.len() != 32 {
        return Err(LlmError::InvalidResponse {
            provider: PROVIDER_NAME.to_string(),
            reason: "invalid x402 private key length".to_string(),
        });
    }

    SecretKey::from_slice(&bytes).map_err(|_| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: "invalid x402 private key".to_string(),
    })
}

fn owner_address(secp: &Secp256k1<secp256k1::All>, private_key: &SecretKey) -> String {
    let public_key = PublicKey::from_secret_key(secp, private_key);
    let uncompressed = public_key.serialize_uncompressed();
    let hash = keccak256(&uncompressed[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

fn build_eip712_permit_digest(
    token_name: &str,
    token_version: &str,
    chain_id: u64,
    verifying_contract: &str,
    owner: &str,
    spender: &str,
    value: &str,
    nonce: u128,
    deadline: u64,
) -> Result<[u8; 32], LlmError> {
    let domain_type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let permit_type_hash = keccak256(
        b"Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)",
    );

    let name_hash = keccak256(token_name.as_bytes());
    let version_hash = keccak256(token_version.as_bytes());

    let verifying_contract_word = address_word(parse_address(verifying_contract)?);
    let owner_word = address_word(parse_address(owner)?);
    let spender_word = address_word(parse_address(spender)?);

    let value_u128 = parse_u128_dec(value).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid permit value '{}': expected decimal uint", value),
    })?;

    let mut domain_encoded = Vec::with_capacity(32 * 5);
    domain_encoded.extend_from_slice(&domain_type_hash);
    domain_encoded.extend_from_slice(&name_hash);
    domain_encoded.extend_from_slice(&version_hash);
    domain_encoded.extend_from_slice(&u256_word_from_u64(chain_id));
    domain_encoded.extend_from_slice(&verifying_contract_word);
    let domain_separator = keccak256(&domain_encoded);

    let mut permit_encoded = Vec::with_capacity(32 * 6);
    permit_encoded.extend_from_slice(&permit_type_hash);
    permit_encoded.extend_from_slice(&owner_word);
    permit_encoded.extend_from_slice(&spender_word);
    permit_encoded.extend_from_slice(&u256_word_from_u128(value_u128));
    permit_encoded.extend_from_slice(&u256_word_from_u128(nonce));
    permit_encoded.extend_from_slice(&u256_word_from_u64(deadline));
    let permit_hash = keccak256(&permit_encoded);

    let mut digest = Vec::with_capacity(2 + 32 + 32);
    digest.extend_from_slice(&[0x19, 0x01]);
    digest.extend_from_slice(&domain_separator);
    digest.extend_from_slice(&permit_hash);

    Ok(keccak256(&digest))
}

fn sign_digest(
    secp: &Secp256k1<secp256k1::All>,
    private_key: &SecretKey,
    digest: [u8; 32],
) -> Result<String, LlmError> {
    let message = Message::from_digest(digest);
    let signature = secp.sign_ecdsa_recoverable(&message, private_key);
    let (recovery_id, compact) = signature.serialize_compact();

    let mut full = [0_u8; 65];
    full[..64].copy_from_slice(&compact);
    full[64] = i32::from(recovery_id) as u8 + 27;

    Ok(format!("0x{}", hex::encode(full)))
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn parse_u128_dec(value: &str) -> Option<u128> {
    value.trim().parse::<u128>().ok()
}

fn parse_hex_u128(value: &str) -> Option<u128> {
    let trimmed = value.trim();
    let hex_value = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed)
        .trim_start_matches('0');

    if hex_value.is_empty() {
        return Some(0);
    }

    if hex_value.len() > 32 {
        return None;
    }

    u128::from_str_radix(hex_value, 16).ok()
}

fn normalize_address(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let prefixed = if let Some(rest) = trimmed.strip_prefix("0X") {
        format!("0x{}", rest)
    } else if trimmed.starts_with("0x") {
        trimmed.to_string()
    } else {
        return None;
    };

    if prefixed.len() != 42 {
        return None;
    }
    if !prefixed
        .as_bytes()
        .iter()
        .skip(2)
        .all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }

    Some(format!("0x{}", prefixed[2..].to_lowercase()))
}

fn parse_address(value: &str) -> Result<[u8; 20], LlmError> {
    let normalized = normalize_address(value).ok_or_else(|| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid address '{}': expected 0x + 40 hex chars", value),
    })?;

    let raw = hex::decode(&normalized[2..]).map_err(|_| LlmError::InvalidResponse {
        provider: PROVIDER_NAME.to_string(),
        reason: format!("invalid address '{}': hex decode failed", value),
    })?;

    let mut out = [0_u8; 20];
    out.copy_from_slice(&raw);
    Ok(out)
}

fn address_word(address: [u8; 20]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out[12..].copy_from_slice(&address);
    out
}

fn u256_word_from_u64(value: u64) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out[24..].copy_from_slice(&value.to_be_bytes());
    out
}

fn u256_word_from_u128(value: u128) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

fn unix_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[async_trait]
impl LlmProvider for X402Provider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let model = req.model.unwrap_or_else(|| self.active_model_name());
        let messages: Vec<ChatCompletionMessage> = req
            .messages
            .into_iter()
            .map(ChatCompletionMessage::from)
            .collect();

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tools: None,
            tool_choice: None,
        };

        let response: ChatCompletionResponse = self.send_chat_request(&request).await?;

        let choice =
            response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: PROVIDER_NAME.to_string(),
                    reason: "No choices in response".to_string(),
                })?;

        let content = choice.message.content.unwrap_or_default();
        let finish_reason = match choice.finish_reason.as_deref() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("tool_calls") => FinishReason::ToolUse,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => FinishReason::Unknown,
        };

        Ok(CompletionResponse {
            content,
            finish_reason,
            input_tokens: response.usage.prompt_tokens,
            output_tokens: response.usage.completion_tokens,
            response_id: None,
        })
    }

    async fn complete_with_tools(
        &self,
        req: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError> {
        let model = req.model.unwrap_or_else(|| self.active_model_name());
        let messages: Vec<ChatCompletionMessage> = req
            .messages
            .into_iter()
            .map(ChatCompletionMessage::from)
            .collect();

        let tools: Vec<ChatCompletionTool> = req
            .tools
            .into_iter()
            .map(|t| ChatCompletionTool {
                tool_type: "function".to_string(),
                function: ChatCompletionFunction {
                    name: t.name,
                    description: Some(t.description),
                    parameters: Some(t.parameters),
                },
            })
            .collect();

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: req.tool_choice,
        };

        let response: ChatCompletionResponse = self.send_chat_request(&request).await?;
        let choice =
            response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: PROVIDER_NAME.to_string(),
                    reason: "No choices in response".to_string(),
                })?;

        let content = choice.message.content;
        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Object(Default::default())),
            })
            .collect();

        let finish_reason = match choice.finish_reason.as_deref() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("tool_calls") => FinishReason::ToolUse,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => {
                if !tool_calls.is_empty() {
                    FinishReason::ToolUse
                } else {
                    FinishReason::Unknown
                }
            }
        };

        Ok(ToolCompletionResponse {
            content,
            tool_calls,
            finish_reason,
            input_tokens: response.usage.prompt_tokens,
            output_tokens: response.usage.completion_tokens,
            response_id: None,
        })
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn cost_per_token(&self) -> (Decimal, Decimal) {
        // Conservative default estimate.
        (dec!(0.0000025), dec!(0.00001))
    }

    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let models = self.fetch_models().await?;
        Ok(models.into_iter().map(|m| m.id).collect())
    }

    async fn model_metadata(&self) -> Result<ModelMetadata, LlmError> {
        let active = self.active_model_name();
        let models = self.fetch_models().await?;
        let current = models.iter().find(|m| m.id == active);
        Ok(ModelMetadata {
            id: active,
            context_length: current.and_then(|m| m.context_length),
        })
    }

    fn active_model_name(&self) -> String {
        self.active_model
            .read()
            .expect("active_model lock poisoned")
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<(), LlmError> {
        let mut guard = self
            .active_model
            .write()
            .expect("active_model lock poisoned");
        *guard = model.to_string();
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatCompletionMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatCompletionTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatCompletionToolCall>>,
}

impl From<ChatMessage> for ChatCompletionMessage {
    fn from(msg: ChatMessage) -> Self {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        let tool_calls = msg.tool_calls.map(|calls| {
            calls
                .into_iter()
                .map(|tc| ChatCompletionToolCall {
                    id: tc.id,
                    call_type: "function".to_string(),
                    function: ChatCompletionToolCallFunction {
                        name: tc.name,
                        arguments: tc.arguments.to_string(),
                    },
                })
                .collect()
        });

        let content = if role == "assistant" && tool_calls.is_some() && msg.content.is_empty() {
            None
        } else {
            Some(msg.content)
        };

        Self {
            role: role.to_string(),
            content,
            tool_call_id: msg.tool_call_id,
            name: msg.name,
            tool_calls,
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ChatCompletionFunction,
}

#[derive(Debug, Serialize)]
struct ChatCompletionFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
    usage: ChatCompletionUsage,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionToolCall>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ChatCompletionToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::Engine;
    use secrecy::SecretString;
    use tokio::sync::Mutex;

    const TEST_USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const TEST_PAY_TO: &str = "0x1111111111111111111111111111111111111111";

    #[derive(Default)]
    struct TestState {
        payment_headers: Mutex<Vec<Option<String>>>,
        chat_calls: Mutex<u32>,
    }

    fn test_success_response(content: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [
                {
                    "message": { "content": content },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 7
            }
        })
    }

    fn test_payment_required_header() -> String {
        let payload = serde_json::json!({
            "x402Version": 2,
            "accepts": [{
                "network": "eip155:8453",
                "asset": TEST_USDC_BASE,
                "payTo": TEST_PAY_TO,
                "extra": {
                    "name": "USD Coin",
                    "version": "2",
                    "maxAmountRequired": "1000000"
                }
            }]
        });
        base64::engine::general_purpose::STANDARD.encode(payload.to_string())
    }

    async fn config_handler() -> impl IntoResponse {
        Json(serde_json::json!({
            "api_version": "1",
            "networks": [{
                "network_id": "eip155:8453",
                "chain_id": 8453,
                "asset": {
                    "address": TEST_USDC_BASE,
                    "symbol": "USDC",
                    "decimals": 6
                },
                "pay_to": TEST_PAY_TO,
                "active": true
            }],
            "payment_required": true,
            "payment_header": "PAYMENT-SIGNATURE",
            "eip712_config": {
                "domain_name": "USD Coin",
                "domain_version": "2"
            }
        }))
    }

    async fn models_handler(headers: HeaderMap) -> impl IntoResponse {
        assert!(
            headers.get("PAYMENT-SIGNATURE").is_none(),
            "/v1/models must remain unsigned"
        );
        Json(serde_json::json!({
            "data": [
                { "id": "x402-model", "context_length": 131072 }
            ]
        }))
    }

    async fn rpc_handler() -> impl IntoResponse {
        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x0"
        }))
    }

    async fn strict_payment_handler(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let payment = headers
            .get("PAYMENT-SIGNATURE")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);
        state.payment_headers.lock().await.push(payment.clone());

        if payment.is_none() {
            return (
                StatusCode::PAYMENT_REQUIRED,
                Json(serde_json::json!({ "error": "payment required" })),
            )
                .into_response();
        }

        (StatusCode::OK, Json(test_success_response("ok"))).into_response()
    }

    async fn payment_required_then_ok_handler(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let mut calls = state.chat_calls.lock().await;
        let payment = headers
            .get("PAYMENT-SIGNATURE")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);
        state.payment_headers.lock().await.push(payment);

        if *calls == 0 {
            *calls += 1;
            let mut response = (
                StatusCode::PAYMENT_REQUIRED,
                Json(serde_json::json!({
                    "code": "PAYMENT_REQUIRED",
                    "error": "permit expired"
                })),
            )
                .into_response();
            response.headers_mut().insert(
                "PAYMENT-REQUIRED",
                HeaderValue::from_str(&test_payment_required_header()).expect("valid header"),
            );
            return response;
        }

        *calls += 1;
        (StatusCode::OK, Json(test_success_response("retried-ok"))).into_response()
    }

    async fn start_test_router(
        state: Arc<TestState>,
        chat_handler: axum::routing::MethodRouter<Arc<TestState>>,
    ) -> std::net::SocketAddr {
        let app = Router::new()
            .route("/v1/config", get(config_handler))
            .route("/v1/models", get(models_handler))
            .route("/v1/chat/completions", chat_handler)
            .route("/rpc", post(rpc_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind should succeed");
        let addr = listener.local_addr().expect("addr should be available");

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    fn test_config(base_url: String, rpc_url: String) -> X402Config {
        X402Config {
            base_url,
            rpc_url,
            private_key: SecretString::from(
                "0x1111111111111111111111111111111111111111111111111111111111111111",
            ),
            network: "eip155:8453".to_string(),
            permit_cap: "10000000".to_string(),
            model: "x402-model".to_string(),
        }
    }

    #[test]
    fn test_new_provider_and_model_name() {
        let cfg = test_config(
            "https://router.example.com".to_string(),
            "https://mainnet.base.org".to_string(),
        );

        let provider = X402Provider::new(cfg).expect("provider should build");
        assert_eq!(provider.model_name(), "x402-model");
        assert_eq!(provider.active_model_name(), "x402-model");
    }

    #[test]
    fn decode_payment_required_header_extracts_requirements() {
        let encoded = test_payment_required_header();
        let decoded = decode_payment_required_header(&encoded).expect("header should decode");

        assert_eq!(decoded.accepts.len(), 1);
        let first = &decoded.accepts[0];
        assert_eq!(first.network.as_deref(), Some("eip155:8453"));
        assert_eq!(
            extract_required_max_amount(first).as_deref(),
            Some("1000000")
        );
    }

    #[test]
    fn router_config_rejects_non_base_network() {
        let bad = RouterConfigResponse {
            networks: vec![RouterNetworkConfig {
                network_id: Some("eip155:1".to_string()),
                asset: Some(RouterAssetConfig {
                    address: Some(TEST_USDC_BASE.to_string()),
                }),
                pay_to: Some(TEST_PAY_TO.to_string()),
                active: Some(true),
            }],
            payment_header: Some("PAYMENT-SIGNATURE".to_string()),
            eip712_config: None,
        };

        let err = parse_router_config(bad).expect_err("non-base network must fail");
        assert!(
            err.to_string().contains("unsupported network"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn router_config_rejects_non_usdc_asset() {
        let bad = RouterConfigResponse {
            networks: vec![RouterNetworkConfig {
                network_id: Some("eip155:8453".to_string()),
                asset: Some(RouterAssetConfig {
                    address: Some("0x0000000000000000000000000000000000000001".to_string()),
                }),
                pay_to: Some(TEST_PAY_TO.to_string()),
                active: Some(true),
            }],
            payment_header: Some("PAYMENT-SIGNATURE".to_string()),
            eip712_config: None,
        };

        let err = parse_router_config(bad).expect_err("non-usdc asset must fail");
        assert!(
            err.to_string().contains("unsupported asset"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn models_endpoint_is_unsigned() {
        let state = Arc::new(TestState::default());
        let addr = start_test_router(state, post(strict_payment_handler)).await;
        let cfg = test_config(format!("http://{addr}"), format!("http://{addr}/rpc"));
        let provider = X402Provider::new(cfg).expect("provider should build");

        let models = provider.list_models().await.expect("models should load");
        assert_eq!(models, vec!["x402-model".to_string()]);
    }

    #[tokio::test]
    async fn complete_uses_payment_signature_header() {
        let state = Arc::new(TestState::default());
        let addr = start_test_router(state.clone(), post(strict_payment_handler)).await;
        let cfg = test_config(format!("http://{addr}"), format!("http://{addr}/rpc"));
        let provider = X402Provider::new(cfg).expect("provider should build");

        let req = CompletionRequest::new(vec![ChatMessage::user("hello")]);
        let result = provider.complete(req).await;
        assert!(
            result.is_ok(),
            "completion should succeed with payment header"
        );

        let headers = state.payment_headers.lock().await;
        assert_eq!(headers.len(), 1, "expected one completion call");
        assert!(
            headers[0].is_some(),
            "PAYMENT-SIGNATURE should be present on completion requests"
        );
    }

    #[tokio::test]
    async fn complete_retries_once_on_payment_required() {
        let state = Arc::new(TestState::default());
        let addr = start_test_router(state.clone(), post(payment_required_then_ok_handler)).await;
        let cfg = test_config(format!("http://{addr}"), format!("http://{addr}/rpc"));
        let provider = X402Provider::new(cfg).expect("provider should build");

        let req = CompletionRequest::new(vec![ChatMessage::user("hello")]);
        let result = provider.complete(req).await;
        assert!(
            result.is_ok(),
            "completion should succeed after refreshing permit once"
        );
        assert_eq!(
            result.expect("result should be ok").content,
            "retried-ok".to_string()
        );

        let calls = *state.chat_calls.lock().await;
        assert_eq!(calls, 2, "provider should perform one retry after 402");

        let headers = state.payment_headers.lock().await;
        assert_eq!(headers.len(), 2, "expected initial + retry request");
        assert!(headers[0].is_some() && headers[1].is_some());
        assert_ne!(
            headers[0], headers[1],
            "retry should refresh permit payload"
        );
    }
}
