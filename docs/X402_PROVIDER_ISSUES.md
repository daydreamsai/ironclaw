# X402 Provider Issue Set (With Checks)

This is a copy/paste-ready GitHub issue set for the x402 provider project.

Source PRD: `docs/X402_PROVIDER_PRD_TDD.md`

## Labels

Recommended labels:

- `area/llm`
- `area/config`
- `area/setup`
- `area/docs`
- `area/tests`
- `feature`
- `tdd`
- `security`
- `x402`

## Dependency Graph

- TRACK -> A1, A2, B1, B2, B3, B4, B5, B6, C1, C2, C3, D1, D2, D3
- A1 blocks B1
- A2 blocks B1
- B1 blocks B2, B3, B4
- B2 + B3 + B4 block B5
- B5 blocks D2
- C1 + C2 + C3 can run in parallel after A1
- D1 starts after B2/B3/B4
- D3 is final gate

---

## TRACK - X402 Provider Delivery

**Title**: `TRACK: x402 outbound LLM provider (Base + USDC)`

**Body**:

Goal: Deliver x402-authenticated outbound LLM provider with test coverage and docs.

### Child Issues

- [ ] A1 - Add `x402` backend to config and enums
- [ ] A2 - Add x402 secrets injection mapping
- [ ] B1 - Implement x402 provider skeleton
- [ ] B2 - Implement router config client/cache
- [ ] B3 - Implement permit signing engine
- [ ] B4 - Implement signed request pipeline
- [ ] B5 - Implement challenge parse/retry
- [ ] B6 - Enforce Base + USDC constraints
- [ ] C1 - Add onboarding flow for x402
- [ ] C2 - Update env/docs
- [ ] C3 - Update feature parity table
- [ ] D1 - Unit test suite for x402 parser/cache/error mapping
- [ ] D2 - Integration tests with mock router
- [ ] D3 - Regression pass across existing backends

### Global Checks

- [ ] `PAYMENT-SIGNATURE` used for signed inference requests
- [ ] `PAYMENT-REQUIRED` challenge handling implemented
- [ ] `/v1/models` remains unsigned
- [ ] Only Base + USDC supported in v1
- [ ] `FEATURE_PARITY.md` updated before merge
- [ ] CI green for touched suites

---

## A1 - Add `x402` backend to config and enums

**Labels**: `feature`, `area/config`, `area/llm`, `x402`, `tdd`

### Scope

Add `LlmBackend::X402` and parse support in config resolution.

### Checks

- [ ] Add enum variant in `src/config/llm.rs`
- [ ] Extend parsing/display for `x402`
- [ ] Add `X402Config` to `LlmConfig`
- [ ] Add required env parsing and validation
- [ ] Wire factory branch in `src/llm/mod.rs`
- [ ] Unit tests for parse success/failure

### Acceptance

- [ ] `LLM_BACKEND=x402` resolves successfully with valid config
- [ ] Invalid config fails with actionable `ConfigError`

---

## A2 - Add x402 secrets injection mapping

**Labels**: `feature`, `area/config`, `security`, `x402`, `tdd`

### Scope

Allow `X402_PRIVATE_KEY` to be injected from encrypted secret storage.

### Checks

- [ ] Extend mapping in `src/config/mod.rs`
- [ ] Preserve existing secret mappings
- [ ] Add test/validation that explicit env still wins

### Acceptance

- [ ] Private key can be loaded from secrets when env is absent
- [ ] Existing providers unaffected

---

## B1 - Implement x402 provider skeleton

**Labels**: `feature`, `area/llm`, `x402`, `tdd`

### Scope

Create `src/llm/x402.rs` with `LlmProvider` implementation shell.

### Checks

- [ ] Add module and exports in `src/llm/mod.rs`
- [ ] Implement `model_name`, `cost_per_token`
- [ ] Implement `complete`, `complete_with_tools`, `list_models` stubs with typed errors
- [ ] Add provider initialization path

### Acceptance

- [ ] Provider compiles and is constructible via config
- [ ] Unsupported/placeholder paths return deterministic errors

---

## B2 - Implement router config client/cache

**Labels**: `feature`, `area/llm`, `x402`, `tdd`

### Scope

Fetch `/v1/config` unsigned, parse x402 config, cache with refresh policy.

### Checks

- [ ] Add config response structs and parser
- [ ] Add cache keyed by base URL
- [ ] Support `payment_header` fallback to `PAYMENT-SIGNATURE`
- [ ] Handle parser tolerance for field aliases (`payTo`/`pay_to`, etc.)
- [ ] Unit tests for parse + fallback behavior

### Acceptance

- [ ] Config fetched and parsed correctly from router
- [ ] Cache hit path avoids redundant calls

---

## B3 - Implement permit signing engine

**Labels**: `feature`, `area/llm`, `security`, `x402`, `tdd`

### Scope

Implement ERC-2612 permit signing and payload construction.

### Checks

- [ ] Add required EVM crate dependencies in `Cargo.toml`
- [ ] Implement private key normalization and account derivation
- [ ] Implement `nonces(owner)` on-chain read
- [ ] Implement EIP-712 Permit signing
- [ ] Build base64 payment payload
- [ ] Ensure secret-safe logging
- [ ] Unit tests for key validation + payload structure

### Acceptance

- [ ] Provider produces valid payment signature payload for Base USDC
- [ ] No secret value appears in logs

---

## B4 - Implement signed request pipeline

**Labels**: `feature`, `area/llm`, `x402`, `tdd`

### Scope

Attach payment signatures to signed inference routes only.

### Checks

- [ ] Define signed vs unsigned route policy
- [ ] Keep `/v1/config` unsigned
- [ ] Keep `/v1/models` unsigned
- [ ] Add `PAYMENT-SIGNATURE` for completion endpoints
- [ ] Add tests proving header presence/absence by route

### Acceptance

- [ ] Signed calls include payment header
- [ ] Unsigned endpoints never receive payment header

---

## B5 - Implement challenge parse/retry

**Labels**: `feature`, `area/llm`, `x402`, `tdd`

### Scope

Handle `401/402` with `PAYMENT-REQUIRED`, invalidate permit, retry once.

### Checks

- [ ] Parse error body and challenge header
- [ ] Decode base64 x402 challenge payload
- [ ] Apply requirement overrides (cap/payTo/network/asset)
- [ ] Invalidate stale permit cache entries
- [ ] Retry once with refreshed permit
- [ ] Map terminal failure to correct `LlmError`
- [ ] Unit tests for parser and mapping

### Acceptance

- [ ] `402 -> refresh -> retry -> success` path works
- [ ] Terminal failures are deterministic and actionable

---

## B6 - Enforce Base + USDC constraints

**Labels**: `feature`, `area/llm`, `security`, `x402`, `tdd`

### Scope

Restrict v1 to Base + USDC; reject unsupported challenge/config values.

### Checks

- [ ] Validate configured network/asset against allowed constants
- [ ] Validate challenge overrides against same constraints
- [ ] Return explicit `RequestFailed` reason on violation
- [ ] Add unit tests for negative/positive cases

### Acceptance

- [ ] Unsupported chain/asset is rejected early
- [ ] Base + USDC path remains functional

---

## C1 - Add onboarding flow for x402

**Labels**: `feature`, `area/setup`, `x402`, `tdd`

### Scope

Add setup wizard option and prompts for x402.

### Checks

- [ ] Add provider option in `src/setup/wizard.rs`
- [ ] Prompt for router URL/private key/cap
- [ ] Store private key via secrets flow
- [ ] Persist bootstrap env values where required
- [ ] Add setup tests for round-trip and persistence

### Acceptance

- [ ] User can fully configure x402 via onboarding

---

## C2 - Update env/docs

**Labels**: `area/docs`, `x402`

### Scope

Document configuration and constraints.

### Checks

- [ ] Update `.env.example`
- [ ] Update `README.md`
- [ ] Update `src/setup/README.md`
- [ ] Include explicit note: `/v1/models` unsigned
- [ ] Include explicit note: Base + USDC only

### Acceptance

- [ ] Docs match actual runtime behavior

---

## C3 - Update feature parity table

**Labels**: `area/docs`, `x402`

### Scope

Reflect newly implemented provider support.

### Checks

- [ ] Update provider row(s) in `FEATURE_PARITY.md`
- [ ] Update notes/priorities/status markers consistently
- [ ] Verify no mismatch with implemented behavior

### Acceptance

- [ ] Feature parity file is accurate at merge time

---

## D1 - Unit tests for x402 internals

**Labels**: `area/tests`, `x402`, `tdd`

### Scope

Comprehensive unit coverage of parsing, cache, and mapping.

### Checks

- [ ] Config parser tests
- [ ] Challenge decoder tests
- [ ] Permit cache keying/invalidation tests
- [ ] Error mapping tests (`401/402/429/5xx`)
- [ ] Constraint enforcement tests

### Acceptance

- [ ] Core behavior branches covered and stable

---

## D2 - Integration tests with mock router

**Labels**: `area/tests`, `x402`, `tdd`

### Scope

End-to-end provider behavior against mocked router responses.

### Checks

- [ ] Happy path completion
- [ ] Happy path tool completion
- [ ] Unsigned `/v1/models`
- [ ] `402` challenge + retry success
- [ ] malformed challenge failure
- [ ] unsupported network/asset failure

### Acceptance

- [ ] Integration suite proves signed/unsigned and retry contracts

---

## D3 - Regression pass across backends

**Labels**: `area/tests`, `area/llm`, `tdd`

### Scope

Ensure x402 integration does not regress existing providers.

### Checks

- [ ] Run existing llm/config/setup test subsets
- [ ] Validate provider factory behavior across all backends
- [ ] Verify startup path still works for non-x402 backends
- [ ] Run formatter/lint/test commands required by repo policy

### Acceptance

- [ ] No regressions detected in existing provider paths

---

## Merge Checklist (Attach to PR)

- [ ] All linked issues resolved or explicitly deferred
- [ ] `FEATURE_PARITY.md` updated
- [ ] `docs/X402_PROVIDER_PRD_TDD.md` still accurate
- [ ] No plaintext secrets in logs
- [ ] New tests added and passing
- [ ] Existing tests passing for touched areas
