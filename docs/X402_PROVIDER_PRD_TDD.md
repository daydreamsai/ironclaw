# X402 Router Provider - PRD + TDD Plan

## 1. Summary

Implement a new outbound LLM backend in IronClaw that sends OpenAI-compatible inference requests to an x402 router authenticated via wallet-signed ERC-2612 permit payloads.

This document defines product requirements, technical scope, TDD execution, milestones, and implementation issues.

## 2. Context

IronClaw currently supports multiple LLM backends (NEAR AI, OpenAI, Anthropic, Ollama, OpenAI-compatible, Tinfoil) through `LlmProvider` abstraction.

A new backend is needed for crypto-native payment auth on outbound router calls using x402 challenge/response.

## 3. Scope

### 3.1 In Scope

- New LLM backend: `x402`
- Outbound request signing with ERC-2612 permit payloads
- x402 challenge handling via `PAYMENT-REQUIRED`
- Config-driven router metadata fetch from `/v1/config`
- Unsigned `/v1/models` support
- Base network + USDC asset only
- Docs and onboarding updates
- Test suite for provider behavior and error handling

### 3.2 Out of Scope

- Inbound gateway auth changes
- Orchestrator worker token auth changes
- Multi-network support beyond Base
- Multi-asset support beyond USDC
- Streaming token-by-token provider API changes

## 4. Goals and Non-Goals

### 4.1 Goals

- Reliable authenticated calls to x402 router for completions and tool completions
- Deterministic handling of 401/402 payment challenges
- Safe private-key handling and strict configuration validation
- Full test-first implementation path with clear acceptance gates

### 4.2 Non-Goals

- Designing a generic payments framework for all providers
- Runtime wallet UX beyond config/onboarding
- On-chain settlement logic beyond permit signing

## 5. Product Requirements

### 5.1 Functional Requirements

1. IronClaw must support `LLM_BACKEND=x402`.
2. Provider must call x402 router using OpenAI-compatible request format.
3. Provider must not attach payment headers to `/v1/config` and `/v1/models`.
4. Provider must attach `PAYMENT-SIGNATURE` on signed inference endpoints.
5. On `401` or `402`, provider must parse `PAYMENT-REQUIRED`, refresh permit if needed, and retry once.
6. Provider must reject non-Base/non-USDC requirements.
7. Provider must map failures to existing `LlmError` variants.
8. Provider must expose model metadata via `/v1/models` when available.

### 5.2 Non-Functional Requirements

1. No plaintext key logging.
2. Constant-time behavior is not required for outbound header signing, but sensitive values must never be emitted in logs.
3. Retry behavior must be bounded and deterministic.
4. Provider should maintain parity with existing timeout and retry defaults used in other providers.

## 6. Protocol and Contract Assumptions

1. Required request header for payment payload: `PAYMENT-SIGNATURE`.
2. Required challenge response header: `PAYMENT-REQUIRED`.
3. `PAYMENT-REQUIRED` payload follows x402 format (base64-encoded JSON containing accepted requirements).
4. `/v1/models` remains unsigned.
5. Supported chain/asset:
   - Chain: Base (`eip155:8453`)
   - Asset: USDC on Base (`0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`)

## 7. Architecture and File-Level Design

### 7.1 New/Updated Modules

- `src/config/llm.rs`
  - Add `LlmBackend::X402`
  - Add `X402Config`
  - Add env parsing and validation

- `src/config/mod.rs`
  - Extend secret injection mapping for x402 private key

- `src/llm/mod.rs`
  - Register `mod x402;`
  - Add factory creation arm `LlmBackend::X402`

- `src/llm/x402.rs` (new)
  - Implement `LlmProvider`
  - Router config fetch/cache
  - Permit cache
  - Sign + inject payment header
  - 401/402 handling and retry

- `src/setup/wizard.rs`
  - Add x402 provider option
  - Prompt for router URL, private key, cap

- `.env.example`, `README.md`, `src/setup/README.md`
  - Document new backend and required env vars

- `FEATURE_PARITY.md`
  - Update provider support row/status notes when implementation lands

## 8. Configuration Spec

### 8.1 Environment Variables

- `LLM_BACKEND=x402`
- `X402_BASE_URL` (required)
- `X402_PRIVATE_KEY` (required, 0x-prefixed 32-byte hex)
- `X402_NETWORK` (optional, default `eip155:8453`, must currently equal Base)
- `X402_PERMIT_CAP` (optional, default `10000000` base units)
- `LLM_MODEL` (required or fallback to selected model)

### 8.2 Validation Rules

1. Private key must match `^0x[0-9a-fA-F]{64}$`.
2. Network must be Base only for v1.
3. Permit cap must be positive integer string.
4. Base URL must be valid URL.

## 9. Request Lifecycle

1. Provider initializes and fetches `/v1/config` (unsigned).
2. Provider builds permit payload using:
   - token contract nonce (`nonces(owner)`)
   - EIP-712 domain and Permit struct
   - configured cap and deadline
3. Provider calls inference endpoint with `PAYMENT-SIGNATURE`.
4. If response is success, return mapped completion.
5. If `401/402`:
   - parse `PAYMENT-REQUIRED`
   - apply requirement overrides (if compatible with Base+USDC)
   - invalidate cached permit
   - regenerate permit (respecting required max amount)
   - retry once
6. Return mapped `LlmError` on terminal failure.

## 10. Security and Compliance Requirements

1. Private key must be sourced from env or encrypted secrets store.
2. Never log private key, full permit payload, or raw signature.
3. Log only minimal metadata: network, asset, challenge status, retry reason.
4. Do not persist permit signatures beyond in-memory cache.

## 11. TDD Strategy

Implementation must follow Red-Green-Refactor in each milestone.

### 11.1 Unit Test Categories

1. Config parsing and validation
2. Router config parsing and fallback behavior
3. Challenge header decoding (`PAYMENT-REQUIRED`)
4. Permit cache keying/invalidation
5. Error mapping (`401/402/429/5xx`)
6. Base+USDC constraint enforcement

### 11.2 Integration Test Categories

1. Happy path signed completion
2. Happy path signed tool completion
3. Unsigned `/v1/models`
4. `402` challenge then successful retry
5. Invalid challenge payload -> deterministic failure
6. Unsupported network/asset -> deterministic rejection

### 11.3 Test Fixtures

- Mock x402 router server (HTTP)
- Deterministic wallet key fixture
- Static `/v1/config` and challenge payload fixtures

## 12. Milestones

### Milestone 0 - Design and Contracts

### Deliverables

- Finalized config schema
- Finalized parser contracts for `/v1/config` and `PAYMENT-REQUIRED`
- File-level design checked in as doc

### Exit Criteria

- Contract assumptions documented and approved
- No open unknowns about headers/endpoints

### Milestone 1 - Backend + Config Plumbing

### Deliverables

- `LlmBackend::X402`
- `X402Config` parse and validation
- Secret injection mapping for `X402_PRIVATE_KEY`

### TDD Exit Criteria

- Unit tests for valid/invalid env inputs
- Factory selects x402 backend correctly

### Milestone 2 - Provider Skeleton + Unsigned Paths

### Deliverables

- `src/llm/x402.rs` with provider trait implementation
- `/v1/models` unsigned calls working
- Basic completion pass-through scaffolding

### TDD Exit Criteria

- Mocked `/v1/models` integration test passes
- No payment header on config/models endpoints

### Milestone 3 - Permit Signing

### Deliverables

- EIP-712 Permit signing
- On-chain nonce retrieval
- `PAYMENT-SIGNATURE` header injection

### TDD Exit Criteria

- Signature generation unit tests
- Signed completion happy-path integration test

### Milestone 4 - Challenge/Retry

### Deliverables

- `PAYMENT-REQUIRED` decode
- Requirement application
- Permit invalidation + one retry flow

### TDD Exit Criteria

- `402 -> retry -> success` test passes
- invalid challenge payload test passes
- unsupported network/asset test passes

### Milestone 5 - Docs, Wizard, Parity

### Deliverables

- Setup wizard support for x402
- Env/docs updates
- `FEATURE_PARITY.md` status updates

### TDD Exit Criteria

- Wizard config persistence tests
- Docs reflect exact supported scope and limitations

### Milestone 6 - Hardening

### Deliverables

- Logging hardening
- Timeout and retry tuning
- Final integration regression pass

### TDD Exit Criteria

- Existing provider tests still pass
- x402 test suite stable in CI

## 13. Issue Backlog (Implementation Tickets)

### Epic A - Core Backend

#### Issue A1 - Add `x402` backend to config and enums

- Type: Feature
- Files: `src/config/llm.rs`, `src/llm/mod.rs`
- Acceptance:
  - Backend parses from env/settings
  - Factory routes to x402 provider
  - Unit tests added

#### Issue A2 - Add x402 secrets injection mapping

- Type: Feature
- Files: `src/config/mod.rs`
- Acceptance:
  - `X402_PRIVATE_KEY` can be loaded from encrypted secrets
  - Existing mappings unaffected

### Epic B - Provider Runtime

#### Issue B1 - Implement x402 provider skeleton

- Type: Feature
- Files: `src/llm/x402.rs`, `src/llm/mod.rs`
- Acceptance:
  - Implements `LlmProvider`
  - Supports `complete`, `complete_with_tools`, `list_models`

#### Issue B2 - Implement router config client/cache

- Type: Feature
- Files: `src/llm/x402.rs`
- Acceptance:
  - `/v1/config` fetched unsigned
  - config cached and refreshed deterministically

#### Issue B3 - Implement permit signing engine

- Type: Feature
- Files: `src/llm/x402.rs`, `Cargo.toml`
- Acceptance:
  - ERC-2612 typed data signed from configured key
  - nonce read from token contract

#### Issue B4 - Implement signed request pipeline

- Type: Feature
- Files: `src/llm/x402.rs`
- Acceptance:
  - `PAYMENT-SIGNATURE` added only to signed endpoints
  - `/v1/models` remains unsigned

#### Issue B5 - Implement challenge parse/retry

- Type: Feature
- Files: `src/llm/x402.rs`
- Acceptance:
  - Handles `401/402` with `PAYMENT-REQUIRED`
  - Regenerates permit and retries once
  - deterministic failure after retry exhaustion

#### Issue B6 - Enforce Base + USDC constraints

- Type: Feature
- Files: `src/llm/x402.rs`
- Acceptance:
  - Rejects unsupported network/asset from config or challenge
  - Emits explicit error reason

### Epic C - UX and Docs

#### Issue C1 - Add onboarding flow for x402

- Type: Feature
- Files: `src/setup/wizard.rs`, `src/settings.rs` (if needed)
- Acceptance:
  - User can configure router URL/private key/cap
  - Values persist correctly

#### Issue C2 - Update env/docs

- Type: Docs
- Files: `.env.example`, `README.md`, `src/setup/README.md`
- Acceptance:
  - New backend documented with examples and constraints

#### Issue C3 - Update feature parity table

- Type: Docs
- Files: `FEATURE_PARITY.md`
- Acceptance:
  - Provider support and notes updated consistently

### Epic D - Testing

#### Issue D1 - Unit test suite for x402 parser/cache/error mapping

- Type: Test
- Files: `src/llm/x402.rs` tests
- Acceptance:
  - Covers decode, invalidation, mapping branches

#### Issue D2 - Integration tests with mock router

- Type: Test
- Files: `tests/` (new x402 tests)
- Acceptance:
  - Happy path
  - challenge-retry path
  - unsupported requirement path

#### Issue D3 - Regression pass across existing backends

- Type: Test
- Files: existing suites
- Acceptance:
  - No regressions in provider factory or startup

## 14. Risks and Mitigations

1. Risk: x402 challenge schema drift.
- Mitigation: parser tolerant to alternate field casing (`payTo` vs `pay_to`, etc.).

2. Risk: permit invalidation loops.
- Mitigation: one bounded retry per request with clear stop condition.

3. Risk: accidental secret leakage in logs.
- Mitigation: structured log scrubber and strict no-value logging policy.

4. Risk: network calls to nonce endpoint increase latency.
- Mitigation: permit cache reuse until invalidation/expiry.

## 15. Acceptance for PR Merge

1. All milestone tests for implemented scope pass.
2. `cargo test` passes for provider and integration suites touched.
3. Docs updated for new backend and env vars.
4. `FEATURE_PARITY.md` updated for changed behavior status.
5. No secrets in logs under normal or error paths.

## 16. Post-MVP Follow-Ups

1. Add multi-network support.
2. Add additional ERC-20 assets.
3. Add richer permit budget policies per model/route.
4. Add provider-specific observability metrics for payment challenge rates.
