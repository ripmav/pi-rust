//! Core-native oauth builtins (WP-1404/E3S4): the credential lifecycle for
//! the upstream builtin oauth providers the core can serve natively.
//!
//! v0 owns exactly `kimi-coding` — the one oauth builtin whose wire API
//! (`openai-completions`) a built-in driver speaks. The other four (radius,
//! github-copilot, xai, meta) have no core driver for their wire APIs yet:
//! they stay sidecar-side (login via the node helper `sidecar/auth-login.mjs`,
//! streaming through the sidecar's upstream runtime) until E4S2 (M15
//! provider plugin platform) is green — D15 answered by the PO 2026-11-24
//! ("API interaktionen, auch logins soll der core machen" — the core does
//! all API interactions incl. logins, as auto-loaded provider libraries); no
//! capability regression mid-epic (E3 sequencing rule).
//!
//! Upstream protocol/credential semantics (pinned 64a15f4), mirrored here:
//! - `packages/ai/src/auth/oauth/kimi-coding.ts` — RFC 8628 device-code
//!   login (client id, endpoints, form fields, pending/slow_down/denied/
//!   expired handling, 30 s request timeout) and token refresh (429/5xx
//!   retried 3x with 1/2/4 s backoff; 401/403/invalid_grant = dead
//!   credential, no retry).
//! - `packages/ai/src/auth/resolve.ts` — 5-minute validity margin: a stored
//!   credential with < 5 min remaining is refreshed before use; the rotated
//!   credential is written back (auth.json, upstream shape).
//! - `packages/coding-agent/src/core/auth-storage.ts` — auth.json is
//!   0600-on-create; one credential per provider, other entries preserved.
//!
//! Redaction (WP-904): key material never appears in errors, logs or output.

use crate::corelog;
use anyhow::{bail, Context, Error, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

/// The only oauth builtin the core serves natively in v0 (D15: the core
/// owns what it can stream; the other four stay sidecar-side until E4S2).
pub const KIMI_CODING: &str = "kimi-coding";

/// Upstream `kimi-coding.ts` constants (pinned 64a15f4).
const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";
const DEVICE_CODE_TIMEOUT_SECONDS: u64 = 15 * 60;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH_MAX_RETRIES: u8 = 3;
/// Upstream `resolve.ts`: tokens with less than five minutes remaining are
/// refreshed before use.
const MINIMUM_VALIDITY: Duration = Duration::from_secs(5 * 60);

/// A stored oauth credential (the `auth.json` entry shape, upstream
/// `OAuthCredential`: epoch-ms `expires`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthCredential {
    pub access: String,
    pub refresh: String,
    pub expires: i64,
}

/// Upstream `trustedHttpUrl`: an `http:`/`https:` URL, else untrusted.
fn trusted_http_url(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The kimi oauth host: `KIMI_CODE_OAUTH_HOST` or `KIMI_OAUTH_HOST` override
/// (trailing slashes stripped), else the upstream default.
pub fn kimi_oauth_host() -> String {
    let override_ = std::env::var("KIMI_CODE_OAUTH_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("KIMI_OAUTH_HOST")
                .ok()
                .filter(|v| !v.trim().is_empty())
        });
    override_
        .unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Upstream `resolve.ts` expiry rule: `now + 5min >= expires` = expired.
pub fn is_expired(cred: &OauthCredential) -> bool {
    now_ms() + MINIMUM_VALIDITY.as_millis() as i64 >= cred.expires
}

// ---------------------------------------------------------------------------
// Credential store (auth.json)

/// Reads the stored credential for `provider` (typed error on a malformed
/// auth.json; `None` when the file or entry is absent, or the entry is not
/// an oauth credential).
pub fn stored(agent_dir: &Path, provider: &str) -> Result<Option<OauthCredential>> {
    let path = agent_dir.join("auth.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let data: Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    let Some(cred) = data.get(provider) else {
        return Ok(None);
    };
    if cred.get("type").and_then(Value::as_str) != Some("oauth") {
        return Ok(None);
    }
    Ok(Some(OauthCredential {
        access: cred
            .get("access")
            .and_then(Value::as_str)
            .with_context(|| format!("auth.json {provider} credential has no `access`"))?
            .to_string(),
        refresh: cred
            .get("refresh")
            .and_then(Value::as_str)
            .with_context(|| format!("auth.json {provider} credential has no `refresh`"))?
            .to_string(),
        expires: cred
            .get("expires")
            .and_then(Value::as_i64)
            .with_context(|| format!("auth.json {provider} credential has no `expires`"))?,
    }))
}

/// Writes the oauth credential for `provider` (the upstream auth.json shape;
/// other provider entries preserved, 0600-on-create via
/// [`crate::auth::save_credential`]).
pub fn save(agent_dir: &Path, provider: &str, cred: &OauthCredential) -> Result<()> {
    crate::auth::save_credential(
        agent_dir,
        provider,
        &json!({
            "type": "oauth",
            "access": cred.access,
            "refresh": cred.refresh,
            "expires": cred.expires,
        }),
    )
}

// ---------------------------------------------------------------------------
// Kimi Code oauth flow (device code + refresh)

/// One form-POST to the kimi oauth endpoint: (status, parsed JSON, body text).
async fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<(u16, Value, String)> {
    let body: BTreeMap<&str, &str> = fields.iter().copied().collect();
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client");
    let resp = client
        .post(url)
        .form(&body)
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| "requesting the kimi oauth endpoint")?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, json, text))
}

#[derive(Debug)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    // `verification_uri` is validated (trusted http(s)) but not kept — the
    // flow displays/opens `verification_uri_complete` (upstream notifies it).
    verification_uri_complete: String,
    interval: u64,
    expires_in: u64,
}

/// `POST {host}/api/oauth/device_authorization` (upstream startDeviceAuthorization).
async fn device_authorization(host: &str) -> Result<DeviceCode> {
    let (status, json, text) = post_form(
        &format!("{host}/api/oauth/device_authorization"),
        &[("client_id", KIMI_CLIENT_ID)],
    )
    .await?;
    if !(200..300).contains(&status) {
        let detail = if text.is_empty() {
            String::new()
        } else {
            format!(": {text}")
        };
        bail!("Kimi Code device authorization failed with status {status}{detail}");
    }
    let device_code = json.get("device_code").and_then(Value::as_str);
    let user_code = json.get("user_code").and_then(Value::as_str);
    let verification_uri = json
        .get("verification_uri")
        .and_then(Value::as_str)
        .filter(|v| trusted_http_url(v));
    let verification_uri_complete = json
        .get("verification_uri_complete")
        .and_then(Value::as_str)
        .filter(|v| trusted_http_url(v));
    if device_code.is_none()
        || user_code.is_none()
        || verification_uri.is_none()
        || verification_uri_complete.is_none()
    {
        bail!("Invalid Kimi Code device authorization response: {json}");
    }
    let interval = json
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(DEVICE_CODE_TIMEOUT_SECONDS);
    Ok(DeviceCode {
        device_code: device_code.unwrap().to_string(),
        user_code: user_code.unwrap().to_string(),
        verification_uri_complete: verification_uri_complete.unwrap().to_string(),
        interval: interval.max(1), // MINIMUM_INTERVAL_MS
        expires_in,
    })
}

/// `access_token`/`refresh_token`/`expires_in` -> credential (upstream
/// parseTokenResponse: all required, expires = now + expires_in).
fn token_response(json: &Value, operation: &str) -> Result<OauthCredential> {
    let access = json
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let refresh = json
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|s| *s > 0);
    match (access, refresh, expires_in) {
        (Some(access), Some(refresh), Some(expires_in)) => Ok(OauthCredential {
            access: access.to_string(),
            refresh: refresh.to_string(),
            expires: now_ms() + expires_in as i64 * 1000,
        }),
        _ => bail!("Kimi Code token {operation} response missing fields: {json}"),
    }
}

/// The device-code poll loop (upstream `pollOAuthDeviceCodeFlow`): wait
/// before the first poll, honor `slow_down` (server interval or +5 s),
/// stop at the deadline. Ctrl+C cancels ("Login cancelled").
async fn poll_token(host: &str, device: &DeviceCode) -> Result<OauthCredential> {
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval);
    let mut slow_downs = 0u32;
    // waitBeforeFirstPoll: sleep min(interval, remaining) before poll #1.
    let wait = interval.min(deadline.saturating_duration_since(std::time::Instant::now()));
    tokio::select! {
        _ = tokio::signal::ctrl_c() => bail!("Login cancelled"),
        _ = tokio::time::sleep(wait) => {}
    }
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        // A slow_down response is reported as a sentinel by the poll (the
        // server-provided interval rides along in the error-free response).
        tokio::select! {
            _ = tokio::signal::ctrl_c() => bail!("Login cancelled"),
            res = poll_token_once(host, device) => match res {
                Ok(PollOutcome::Complete(cred)) => return Ok(cred),
                Ok(PollOutcome::Pending) => {}
                Ok(PollOutcome::SlowDown(new_interval)) => {
                    slow_downs += 1;
                    interval = new_interval.max(Duration::from_secs(1));
                }
                Err(e) => return Err(e),
            },
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => bail!("Login cancelled"),
            _ = tokio::time::sleep(interval.min(remaining)) => {}
        }
    }
    if slow_downs > 0 {
        bail!("Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.");
    }
    bail!("Device flow timed out");
}

#[derive(Debug)]
enum PollOutcome {
    Complete(OauthCredential),
    Pending,
    SlowDown(Duration),
}

/// One poll with the slow_down interval extracted (upstream reads
/// `json.interval` on slow_down, else +5 s).
async fn poll_token_once(host: &str, device: &DeviceCode) -> Result<PollOutcome> {
    let (status, json, _text) = post_form(
        &format!("{host}/api/oauth/token"),
        &[
            ("client_id", KIMI_CLIENT_ID),
            ("device_code", &device.device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )
    .await?;
    if status >= 500 {
        let detail = json.to_string();
        bail!(
            "Kimi Code device token request failed with status {status}{}",
            if detail == "null" {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    if (200..300).contains(&status) && json.get("access_token").and_then(Value::as_str).is_some() {
        return token_response(&json, "poll").map(PollOutcome::Complete);
    }
    let error = json.get("error").and_then(Value::as_str);
    let description = json
        .get("error_description")
        .and_then(Value::as_str)
        .map(|d| format!(": {d}"));
    match error {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => {
            let server_interval = json
                .get("interval")
                .and_then(Value::as_u64)
                .filter(|v| *v > 0)
                .map(Duration::from_secs);
            Ok(PollOutcome::SlowDown(
                server_interval.unwrap_or(Duration::from_secs(5)),
            ))
        }
        Some("expired_token") => {
            bail!("Kimi Code device authorization expired. Please restart login.")
        }
        Some("access_denied") => bail!("Kimi Code login was denied."),
        _ => bail!(
            "Kimi Code device token request failed (status {status}){}",
            match (error, description) {
                (Some(e), Some(d)) => format!(": {e}{d}"),
                (Some(e), None) => format!(": {e}"),
                _ => String::new(),
            }
        ),
    }
}

// ---------------------------------------------------------------------------
// Refresh + bearer resolution

#[derive(Debug)]
enum RefreshError {
    /// 401/403/invalid_grant — the stored credential is dead.
    Unauthorized(String),
    /// Other non-retryable failure.
    Fail(String),
    /// 429/5xx or network error — retried with backoff.
    Retryable(String),
}

/// One refresh attempt (upstream refreshToken's fetch+parse body).
async fn refresh_once(
    host: String,
    refresh_token: String,
) -> std::result::Result<OauthCredential, RefreshError> {
    let result = post_form(
        &format!("{host}/api/oauth/token"),
        &[
            ("client_id", KIMI_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
        ],
    )
    .await;
    match result {
        Ok((status, json, _text)) => {
            if (200..300).contains(&status) {
                return token_response(&json, "refresh").map_err(|e| {
                    RefreshError::Fail(format!("Kimi Code token refresh failed: {e}"))
                });
            }
            let is_invalid_grant =
                json.get("error").and_then(Value::as_str) == Some("invalid_grant");
            if status == 401 || status == 403 || is_invalid_grant {
                let description = json
                    .get("error_description")
                    .and_then(Value::as_str)
                    .map(|d| format!(": {d}"));
                return Err(RefreshError::Unauthorized(format!(
                    "Kimi Code token refresh unauthorized (status {status}){}",
                    description.unwrap_or_default()
                )));
            }
            if status == 429 || status >= 500 {
                return Err(RefreshError::Retryable(format!(
                    "Kimi Code token refresh failed with status {status}"
                )));
            }
            let detail = json.to_string();
            Err(RefreshError::Fail(format!(
                "Kimi Code token refresh failed with status {status}{}",
                if detail == "null" {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )))
        }
        Err(e) => Err(RefreshError::Retryable(format!(
            "Kimi Code token refresh request failed: {e}"
        ))),
    }
}

/// Token refresh (upstream refreshToken: 429/5xx/network retried up to 3x
/// with 1/2/4 s backoff; 401/403/invalid_grant fails immediately). Owns its
/// strings so the future is `'static` (the sync caller blocks on a worker
/// thread, `block_on`).
async fn refresh_token(host: String, refresh_token: String) -> Result<OauthCredential> {
    let mut last_error: Option<Error> = None;
    for attempt in 0..=REFRESH_MAX_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(
                1000u64 * 2u64.pow(attempt as u32 - 1),
            ))
            .await;
        }
        match refresh_once(host.clone(), refresh_token.clone()).await {
            Ok(cred) => return Ok(cred),
            Err(RefreshError::Unauthorized(msg)) => bail!("{msg}"),
            Err(RefreshError::Fail(msg)) => bail!("{msg}"),
            Err(RefreshError::Retryable(msg)) => {
                last_error = Some(Error::msg(msg));
            }
        }
    }
    Err(last_error.unwrap())
}

/// Runs a future on a dedicated worker thread (the sync `build_provider`
/// entry point may run inside or outside a tokio context).
fn block_on<T>(fut: impl Future<Output = T> + Send + 'static) -> T
where
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("kimi-oauth-refresh".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime for the oauth refresh");
            rt.block_on(fut)
        })
        .expect("spawning the refresh worker")
        .join()
        .expect("the refresh worker panicked")
}

/// Resolves the bearer token for `kimi-coding` (upstream `resolveStoredOAuth`
/// semantics): a stored credential with < 5 min remaining is refreshed first
/// and the rotated credential is written back; otherwise the stored access
/// token is used as-is. Called once at driver build (startup / set_model).
// shortcut: upstream re-resolves (re-refreshes) per request; the core
// resolves at driver build, so a long session that outlives the token
// without re-selecting the model hits the provider with a stale token
// (typed 401 guidance). Re-resolving per request needs an async driver
// seam — upgrade when long-session oauth is a reported pain point.
pub fn resolve_kimi_coding_bearer(agent_dir: &Path) -> Result<String> {
    match stored(agent_dir, KIMI_CODING)? {
        None => bail!(
            "no credential for kimi-coding in auth.json — run `pi-rust auth login kimi-coding`"
        ),
        Some(cred) if !is_expired(&cred) => Ok(cred.access),
        Some(cred) => {
            let host = kimi_oauth_host();
            let new_cred = match block_on(refresh_token(host, cred.refresh)) {
                Ok(c) => c,
                Err(e) => {
                    corelog::line(&format!(
                        "[core] oauth refresh for {KIMI_CODING} failed: {e:#}"
                    ));
                    bail!(
                        "OAuth refresh failed for {KIMI_CODING} ({e:#}) — the stored credential may be dead; run `pi-rust auth login {KIMI_CODING}`"
                    );
                }
            };
            save(agent_dir, KIMI_CODING, &new_cred)?;
            Ok(new_cred.access)
        }
    }
}

// ---------------------------------------------------------------------------
// Login (core-native device flow)

/// The core-native `pi-rust auth login kimi-coding` device flow (upstream
/// loginKimiCoding UX): prints the verification URL + user code, optionally
/// opens the browser, polls until the user approves (Ctrl+C cancels), saves
/// the credential to auth.json.
pub async fn kimi_device_login(agent_dir: &Path, open_browser: bool) -> Result<()> {
    let host = kimi_oauth_host();
    let device = device_authorization(&host).await?;
    println!(
        "Open {} and enter code: {}",
        device.verification_uri_complete, device.user_code
    );
    if open_browser {
        crate::auth_login::open_browser(&device.verification_uri_complete);
    }
    println!("Waiting for authentication...");
    let cred = poll_token(&host, &device).await?;
    save(agent_dir, KIMI_CODING, &cred)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Arc;

    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-oauth-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // The tests below set the process-wide KIMI_CODE_OAUTH_HOST /
    // KIMI_OAUTH_HOST env vars (each pointing at its own random-port mock).
    // Rust runs tests as parallel threads in one process, so without
    // serialization a test's HTTP call can hit a SIBLING test's mock (CI
    // cross-talk: 404/400/401 from a sibling's queued script — the
    // 2026-11-24 macos-latest failure of the three refresh tests). Every
    // env-touching test holds this lock for its whole body.
    static KIMI_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Serialize the KIMI_* env-var tests (see `KIMI_ENV_LOCK`).
    fn lock_kimi_env() -> std::sync::MutexGuard<'static, ()> {
        KIMI_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queued-response HTTP mock: each accepted connection gets the next
    /// (status, json-body) pair; every request line + body is recorded.
    fn mock(responses: Vec<(u16, &str)>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        mock_with(
            listener,
            responses
                .into_iter()
                .map(|(s, b)| (s, b.to_string()))
                .collect(),
        )
    }

    /// Variant over a pre-bound listener (the test needs the port inside the
    /// queued responses — the device_authorization URLs).
    fn mock_with(
        listener: std::net::TcpListener,
        responses: Vec<(u16, String)>,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let base = format!("http://{}", listener.local_addr().unwrap());
        let recorded: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let rec2 = recorded.clone();
        std::thread::spawn(move || {
            let mut queue = responses.into_iter();
            loop {
                let Ok((mut s, _)) = listener.accept() else {
                    break;
                };
                let mut buf = vec![0u8; 65536];
                let mut have = 0usize;
                // Read the full request (headers + content-length body).
                loop {
                    if have == buf.len() {
                        buf.resize(have * 2, 0);
                    }
                    match s.read(&mut buf[have..]) {
                        Ok(k) if k > 0 => have += k,
                        _ => break,
                    }
                    let head_end = have - 4;
                    if buf[..have].windows(4).any(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        let header_end = String::from_utf8_lossy(&buf[..have])
                            .find("\r\n\r\n")
                            .map(|i| i + 4)
                            .unwrap_or(0);
                        if have >= header_end + len {
                            break;
                        }
                    }
                    if have > 1_000_000 {
                        break; // v0 test bound
                    }
                }
                let raw = String::from_utf8_lossy(&buf[..have]).to_string();
                let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                let first = raw.lines().next().unwrap_or_default();
                rec2.lock().unwrap().push(format!("{first} BODY {body}"));
                let (status, body) = queue.next().unwrap_or((404, String::from("{}")));
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                resp += &body;
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (base, recorded)
    }

    const TOKEN: &str =
        r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#;

    #[test]
    fn stored_round_trip_preserves_other_providers() {
        let d = tmpdir();
        std::fs::write(
            d.join("auth.json"),
            r#"{"groq":{"type":"api_key","key":"k"}}"#,
        )
        .unwrap();
        assert!(stored(&d, "kimi-coding").unwrap().is_none());
        assert!(
            stored(&d, "groq").unwrap().is_none(),
            "api_key entries are not oauth"
        );
        let cred = OauthCredential {
            access: "a".into(),
            refresh: "r".into(),
            expires: now_ms() + 3_600_000,
        };
        save(&d, "kimi-coding", &cred).unwrap();
        let got = stored(&d, "kimi-coding").unwrap().expect("oauth entry");
        assert_eq!(got, cred);
        let data: Value =
            serde_json::from_str(&std::fs::read_to_string(d.join("auth.json")).unwrap()).unwrap();
        assert_eq!(data["groq"]["key"], json!("k"), "other providers preserved");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_auth_json_0600() {
        let d = tmpdir();
        save(
            &d,
            KIMI_CODING,
            &OauthCredential {
                access: "a".into(),
                refresh: "r".into(),
                expires: now_ms() + 3_600_000,
            },
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(d.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "auth.json created 0600 (upstream mode-on-creation)"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn expiry_margin_is_five_minutes() {
        let fresh = OauthCredential {
            access: "a".into(),
            refresh: "r".into(),
            expires: now_ms() + 6 * 60_000,
        };
        let stale = OauthCredential {
            access: "a".into(),
            refresh: "r".into(),
            expires: now_ms() + 4 * 60_000,
        };
        assert!(!is_expired(&fresh));
        assert!(is_expired(&stale), "<5min remaining = expired");
    }

    #[test]
    fn resolve_without_credential_is_typed_error() {
        let d = tmpdir();
        let err = resolve_kimi_coding_bearer(&d).unwrap_err().to_string();
        assert!(err.contains("pi-rust auth login kimi-coding"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_fresh_credential_skips_network() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        let cred = OauthCredential {
            access: "valid-access".into(),
            refresh: "r".into(),
            expires: now_ms() + 60 * 60_000,
        };
        save(&d, KIMI_CODING, &cred).unwrap();
        let (base, recorded) = mock(vec![]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let tok = resolve_kimi_coding_bearer(&d).unwrap();
        assert_eq!(tok, "valid-access");
        assert!(
            recorded.lock().unwrap().is_empty(),
            "no refresh for a fresh token"
        );
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_expired_credential_refreshes_and_writes_back() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        save(
            &d,
            KIMI_CODING,
            &OauthCredential {
                access: "old-access".into(),
                refresh: "old-refresh".into(),
                expires: 1,
            },
        )
        .unwrap();
        let (base, recorded) = mock(vec![(200, TOKEN)]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let tok = resolve_kimi_coding_bearer(&d).unwrap();
        assert_eq!(tok, "new-access");
        let log = recorded.lock().unwrap().join("\n");
        assert!(log.contains("grant_type=refresh_token"), "{log}");
        assert!(log.contains("refresh_token=old-refresh"), "{log}");
        assert!(
            log.contains("client_id=17e5f671-d194-4dfb-9706-5516cb48c098"),
            "{log}"
        );
        let got = stored(&d, KIMI_CODING).unwrap().unwrap();
        assert_eq!(got.access, "new-access");
        assert_eq!(got.refresh, "new-refresh");
        assert!(got.expires > now_ms());
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refresh_retries_transient_then_succeeds() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        save(
            &d,
            KIMI_CODING,
            &OauthCredential {
                access: "old-access".into(),
                refresh: "old-refresh".into(),
                expires: 1,
            },
        )
        .unwrap();
        let (base, _recorded) = mock(vec![(429, "{}"), (200, TOKEN)]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let t0 = std::time::Instant::now();
        let tok = resolve_kimi_coding_bearer(&d).unwrap();
        assert_eq!(tok, "new-access");
        assert!(
            t0.elapsed() >= std::time::Duration::from_millis(900),
            "the 1 s backoff before retry 1"
        );
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refresh_unauthorized_fails_without_retry_and_without_material() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        save(
            &d,
            KIMI_CODING,
            &OauthCredential {
                access: "old-access".into(),
                refresh: "old-refresh".into(),
                expires: 1,
            },
        )
        .unwrap();
        let (base, recorded) = mock(vec![(
            401,
            r#"{"error":"invalid_grant","error_description":"nope"}}"#,
        )]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let err = resolve_kimi_coding_bearer(&d).unwrap_err().to_string();
        assert!(err.contains("unauthorized"), "{err}");
        assert!(err.contains("pi-rust auth login kimi-coding"), "{err}");
        assert!(
            !err.contains("old-access") && !err.contains("old-refresh"),
            "{err}"
        );
        assert_eq!(recorded.lock().unwrap().len(), 1, "401 must not be retried");
        // The credential is untouched (no half-rotation).
        let got = stored(&d, KIMI_CODING).unwrap().unwrap();
        assert_eq!(got.refresh, "old-refresh");
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    // The KIMI_ENV_LOCK guard is held across the awaits below on purpose:
    // #[tokio::test] is a current-thread runtime (the guard is never
    // moved), and the env var must stay stable for the whole flow.
    #[allow(clippy::await_holding_lock)]
    async fn device_login_happy_path_saves_credential() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        let device = r#"{"device_code":"dev-1","user_code":"KIMI-TEST","verification_uri":"__CB__","verification_uri_complete":"__CB2__","interval":1,"expires_in":60}"#;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (base2, recorded) = mock_with(
            listener,
            vec![
                (
                    200,
                    device
                        .replace("__CB__", &format!("{base}/activate"))
                        .replace("__CB2__", &format!("{base}/activate?code=KIMI-TEST")),
                ),
                (400, r#"{"error":"authorization_pending"}"#.to_string()),
                (200, TOKEN.to_string()),
            ],
        );
        assert_eq!(base, base2);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        // interval=1 keeps the poll pacing fast; the flow waits one interval
        // before the first poll (upstream waitBeforeFirstPoll), then the
        // pending response, then completes.
        kimi_device_login(&d, false).await.unwrap();
        let got = stored(&d, KIMI_CODING).unwrap().expect("credential saved");
        assert_eq!(got.access, "new-access");
        assert_eq!(got.refresh, "new-refresh");
        assert!(got.expires > now_ms());
        let log = recorded.lock().unwrap().join("\n");
        assert!(log.contains("/api/oauth/device_authorization"), "{log}");
        assert!(
            log.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
                || log.contains("urn:ietf:params:oauth:grant-type:device_code"),
            "{log}"
        );
        assert!(log.contains("device_code=dev-1"), "{log}");
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    // The KIMI_ENV_LOCK guard is held across the awaits below on purpose:
    // #[tokio::test] is a current-thread runtime (the guard is never
    // moved), and the env var must stay stable for the whole flow.
    #[allow(clippy::await_holding_lock)]
    async fn device_login_denied_is_typed() {
        let _env = lock_kimi_env();
        let d = tmpdir();
        let (base, _recorded) = mock(vec![
            (
                200,
                r#"{"device_code":"dev-1","user_code":"U","verification_uri":"http://127.0.0.1/cb","verification_uri_complete":"http://127.0.0.1/cb","interval":1,"expires_in":60}"#,
            ),
            (400, r#"{"error":"access_denied"}"#),
        ]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let device = device_authorization(&base).await.unwrap();
        let err = poll_token(&base, &device).await.unwrap_err().to_string();
        assert!(err.contains("denied"), "{err}");
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    // The KIMI_ENV_LOCK guard is held across the awaits below on purpose:
    // #[tokio::test] is a current-thread runtime (the guard is never
    // moved), and the env var must stay stable for the whole flow.
    #[allow(clippy::await_holding_lock)]
    async fn device_login_expired_token_is_typed() {
        let _env = lock_kimi_env();
        let (base, _recorded) = mock(vec![
            (
                200,
                r#"{"device_code":"dev-1","user_code":"U","verification_uri":"http://127.0.0.1/cb","verification_uri_complete":"http://127.0.0.1/cb","interval":1,"expires_in":60}"#,
            ),
            (400, r#"{"error":"expired_token"}"#),
        ]);
        std::env::set_var("KIMI_CODE_OAUTH_HOST", &base);
        let device = device_authorization(&base).await.unwrap();
        let err = poll_token(&base, &device).await.unwrap_err().to_string();
        assert!(err.contains("expired"), "{err}");
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
    }

    // The KIMI_ENV_LOCK guard is held across the awaits below on purpose:
    // #[tokio::test] is a current-thread runtime (the guard is never
    // moved), and the env var must stay stable for the whole flow.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn untrusted_verification_uri_is_rejected() {
        let _env = lock_kimi_env();
        let (base, _recorded) = mock(vec![(
            200,
            r#"{"device_code":"dev-1","user_code":"U","verification_uri":"file:///etc/passwd","verification_uri_complete":"http://127.0.0.1/cb","interval":1,"expires_in":60}"#,
        )]);
        let err = device_authorization(&base).await.unwrap_err().to_string();
        assert!(
            err.contains("Invalid Kimi Code device authorization"),
            "{err}"
        );
    }

    #[test]
    fn host_override_env_wins_and_strips_trailing_slash() {
        let _env = lock_kimi_env();
        std::env::set_var("KIMI_CODE_OAUTH_HOST", "http://127.0.0.1:1/");
        assert_eq!(kimi_oauth_host(), "http://127.0.0.1:1");
        std::env::remove_var("KIMI_CODE_OAUTH_HOST");
        std::env::set_var("KIMI_OAUTH_HOST", "http://127.0.0.1:2//");
        assert_eq!(kimi_oauth_host(), "http://127.0.0.1:2");
        std::env::remove_var("KIMI_OAUTH_HOST");
        assert_eq!(kimi_oauth_host(), DEFAULT_OAUTH_HOST);
    }
}
