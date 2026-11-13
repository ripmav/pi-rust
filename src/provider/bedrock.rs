//! AWS Bedrock driver: `converse-stream` with manual SigV4 signing
//! (no AWS SDK, per WP-405). The AWS event-stream framing (prelude,
//! headers, CRCs) is parsed manually; wire shapes follow upstream
//! `packages/ai/src/api/bedrock-converse-stream.ts` (pinned 085c84e).
use crate::model::{AssistantTurn, ModelDriver, StreamDelta, ToolCallSpec, Usage};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
pub struct BedrockDriver {
	/// `https://bedrock-runtime.{region}.amazonaws.com` (tests override).
	base_url: String,
	region: String,
	access_key: String,
	secret_key: String,
	session_token: Option<String>,
	model: String,
	client: reqwest::Client,
}

impl BedrockDriver {
	pub fn new(region: String, access_key: String, secret_key: String, session_token: Option<String>, model: String) -> Self {
		Self {
			base_url: format!("https://bedrock-runtime.{region}.amazonaws.com"),
			region,
			access_key,
			secret_key,
			session_token,
			model,
			client: reqwest::Client::new(),
		}
	}

	/// Test hook: point the driver at a mock endpoint (signing still runs
	/// against the declared region/host).
	#[cfg(test)]
	pub fn with_base(base_url: String, region: String, access_key: String, secret_key: String, session_token: Option<String>, model: String) -> Self {
		Self { base_url, ..Self::new(region, access_key, secret_key, session_token, model) }
	}

	/// Sign a request (SigV4, service `bedrock`). `headers` are the
	/// (lowercase name, value) pairs included in the canonical request, any
	/// order. Returns (authorization header, headers to send).
	fn sign(&self, method: &str, path: &str, headers: &[(String, String)], payload: &str, amz_date: &str) -> (String, Vec<(String, String)>) {
		// Canonical headers are sorted by (lowercase) name.
		let mut sorted: Vec<(&String, &String)> = headers.iter().map(|(k, v)| (k, v)).collect();
		sorted.sort_by(|a, b| a.0.cmp(b.0));
		// Canonical headers each end with a newline (spec); the canonical
		// request is the six lines joined by newlines.
		let canonical_header_str: String = sorted.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
		let signed_headers: String = sorted.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
		let canonical_request = [method, path, "", &canonical_header_str, &signed_headers, &hex_sha256(payload.as_bytes())].join("\n");
		let scope = format!("{}/{}/bedrock/aws4_request", &amz_date[..8], self.region);
		let string_to_sign = ["AWS4-HMAC-SHA256", amz_date, &scope, &hex_sha256(canonical_request.as_bytes())].join("\n");
		let k_date = hmac_sha256(format!("AWS4{}", self.secret_key).as_bytes(), amz_date[..8].as_bytes());
		let k_region = hmac_sha256(&k_date, self.region.as_bytes());
		let k_service = hmac_sha256(&k_region, b"bedrock");
		let k_signing = hmac_sha256(&k_service, b"aws4_request");
		let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
		let authorization =
			format!("AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}", self.access_key, scope, signed_headers, signature);
		let mut out: Vec<(String, String)> =
			vec![("x-amz-date".into(), amz_date.to_string()), ("authorization".into(), authorization.clone())];
		if let Some(tok) = &self.session_token {
			out.push(("x-amz-security-token".into(), tok.clone()));
		}
		(authorization, out)
	}

	/// pi-format conversation -> Converse `messages` (tool results become
	/// `toolResult` content in a user message, per the Converse API).
	fn map_messages(convo: &[Value]) -> (Option<Value>, Vec<Value>) {
		let mut system = String::new();
		let mut messages: Vec<Value> = Vec::new();
		for m in convo {
			match m.get("role").and_then(Value::as_str) {
				Some("system") => {
					if let Some(c) = m.get("content").and_then(Value::as_str) {
						system = c.to_string();
					}
				}
				Some("user") => {
					let text = Self::text_of(m.get("content"));
					if text.is_empty() {
						continue;
					}
					messages.push(json!({"role":"user","content":[{"text":text}]}));
				}
				Some("assistant") => {
					let mut content: Vec<Value> = Vec::new();
					if let Some(blocks) = m.get("content").and_then(Value::as_array) {
						for b in blocks {
							match b.get("type").and_then(Value::as_str) {
								Some("text") => {
									let t = b.get("text").and_then(Value::as_str).unwrap_or_default().trim();
									if !t.is_empty() {
										content.push(json!({"text":t}));
									}
								}
								Some("toolCall") => content.push(json!({"toolUse":{
									"toolUseId":b.get("id").cloned().unwrap_or(Value::Null),
									"name":b.get("name").cloned().unwrap_or(Value::Null),
									"input":b.get("input").or_else(|| b.get("arguments")).cloned().unwrap_or(json!({})),
								}})),
								_ => {}
							}
						}
					}
					if content.is_empty() {
						continue;
					}
					messages.push(json!({"role":"assistant","content":content}));
				}
				Some("toolResult") => {
					let text = Self::text_of(m.get("content"));
					let is_err = m.get("isError").and_then(Value::as_bool).unwrap_or(false);
					messages.push(json!({"role":"user","content":[{"toolResult":{
						"toolUseId":m.get("toolCallId").cloned().unwrap_or(Value::Null),
						"content":[{"text":text}],
						"status":if is_err {"error"} else {"success"},
					}}]}));
				}
				_ => {}
			}
		}
		let system_field = if system.trim().is_empty() {
			None
		} else {
			Some(json!([{"text":system}]))
		};
		(system_field, messages)
	}

	fn text_of(v: Option<&Value>) -> String {
		match v {
			Some(Value::String(s)) => s.clone(),
			Some(Value::Array(blocks)) => blocks
				.iter()
				.filter_map(|b| b.get("text").and_then(Value::as_str))
				.collect::<Vec<_>>()
				.join(" "),
			_ => String::new(),
		}
	}

	fn tools_from(convo: &[Value]) -> Option<Value> {
		let defs = convo
			.iter()
			.find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
			.and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
			.unwrap_or_default();
		let tools: Vec<Value> = defs
			.iter()
			.filter_map(|d| {
				d.get("name").and_then(Value::as_str).map(|name| {
					json!({"toolSpec":{
						"name":name,
						"description":d.get("description").cloned().unwrap_or(Value::Null),
						"inputSchema":{"json":d.get("parameters").cloned().unwrap_or(json!({"type":"object"}))},
					}})
				})
			})
			.collect();
		if tools.is_empty() {
			None
		} else {
			Some(json!(tools))
		}
	}

	async fn complete_impl<'a>(
		&'a self,
		convo: &'a [Value],
		mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
	) -> Result<AssistantTurn> {
		let (system_field, messages) = Self::map_messages(convo);
		let mut body = json!({"modelId":self.model,"messages":messages});
		if let Some(s) = system_field {
			body["system"] = s;
		}
		if let Some(tools) = Self::tools_from(convo) {
			body["tools"] = tools;
		}
		let payload = body.to_string();
		let path = format!("/model/{}/converse-stream", self.model);
		let host = self.base_url.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or_default().to_string();
		let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
		// Sign host, x-amz-date, content-type (+ session token when present).
		let mut sign_headers = vec![
			("host".to_string(), host.clone()),
			("x-amz-date".to_string(), amz_date.clone()),
			("content-type".to_string(), "application/json".to_string()),
		];
		if let Some(tok) = &self.session_token {
			sign_headers.push(("x-amz-security-token".to_string(), tok.clone()));
		}
		let (_, auth_headers) = self.sign("POST", &path, &sign_headers, &payload, &amz_date);
		let url = format!("{}{path}", self.base_url.trim_end_matches('/'));
		let mut req = self.client.post(&url).header("content-type", "application/json");
		for (k, v) in &auth_headers {
			req = req.header(k.as_str(), v.as_str());
		}
		let resp = req
			.body(payload)
			.send()
			.await
			.with_context(|| format!("bedrock request failed: {url}"))?;
		if !resp.status().is_success() {
			let status = resp.status();
			let text = resp.text().await.unwrap_or_default();
			bail!("bedrock api error {status}: {text}");
		}
		let mut text = String::new();
		let mut text_started = false;
		let mut usage = Usage::default();
		let mut stop_reason: Option<String> = None;
		// toolUseId -> (name, accumulated input JSON string)
		let mut calls: BTreeMap<String, (String, String)> = BTreeMap::new();
		let mut buf: Vec<u8> = Vec::new();
		let mut stream = resp.bytes_stream();
		while let Some(chunk) = stream.next().await {
			let chunk = chunk.context("bedrock stream error")?;
			buf.extend_from_slice(&chunk);
			loop {
				// AWS event stream record: 4-byte total length (whole record),
				// 4-byte headers length, 16-byte prelude CRC, headers,
				// 8-byte message CRC, JSON payload.
				if buf.len() < 16 {
					break;
				}
				let total = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
				if total < 24 || buf.len() < total {
					break;
				}
				let rec = buf[..total].to_vec();
				buf.drain(..total);
				let headers_len = u32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as usize;
				if rec.len() < 16 + headers_len + 8 {
					continue;
				}
				let payload_start = 16 + headers_len + 8;
				let event_type = event_type_of(&rec[16..16 + headers_len]);
				// Layout: prelude(16) + headers + message CRC(8) + payload —
				// the payload runs to the end of the record.
				let Ok(v) = serde_json::from_slice::<Value>(&rec[payload_start..]) else {
					continue;
				};
				match event_type.as_str() {
					"messageStart" => {
						let role = v.pointer("/messageStart/role").and_then(Value::as_str).unwrap_or_default();
						if role != "assistant" {
							bail!("bedrock stream: expected assistant messageStart, got {role}");
						}
					}
					"contentBlockStart" => {
						if let Some(tu) = v.pointer("/contentBlockStart/start/toolUse") {
							let id = tu.get("toolUseId").and_then(Value::as_str).unwrap_or_default().to_string();
							let name = tu.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
							calls.insert(id, (name, String::new()));
						}
					}
					"contentBlockDelta" => {
						let delta = v.pointer("/contentBlockDelta/delta");
						if let Some(t) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
							if !text_started {
							text_started = true;
							on_delta(StreamDelta::TextStart { index: 0 });
							}
							text.push_str(t);
							on_delta(StreamDelta::TextDelta { index: 0, delta: t.to_string() }); // WP-603: TUI renders tokens as they arrive
						}
						if let Some(tu) = delta.and_then(|d| d.get("toolUse")) {
							let id = tu.get("toolUseId").and_then(Value::as_str).unwrap_or_default().to_string();
							let entry = calls.entry(id).or_default();
							if let Some(name) = tu.get("name").and_then(Value::as_str) {
								entry.0 = name.to_string();
							}
							if let Some(frag) = tu.get("input").and_then(Value::as_str) {
								entry.1.push_str(frag);
							}
						}
					}
					"messageStop" => {
						stop_reason = v.pointer("/messageStop/stopReason").and_then(Value::as_str).map(str::to_string);
					}
					"metadata" => {
						if let Some(u) = v.pointer("/metadata/usage") {
							usage.input = u.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
							usage.output = u.get("outputTokens").and_then(Value::as_u64).unwrap_or(0);
							usage.cache_read = u.get("cacheReadInputTokens").and_then(Value::as_u64).unwrap_or(0);
						}
					}
					"internalServerException" | "modelStreamErrorException" | "validationException" | "throttlingException" | "serviceUnavailableException" => {
						let msg = v.get(event_type.as_str()).and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("unknown bedrock error");
						bail!("bedrock stream error: {msg}");
					}
					_ => {}
				}
			}
		}
		match stop_reason.as_deref() {
			Some("end_turn") | Some("max_tokens") | Some("tool_use") => {}
			Some(other) => bail!("bedrock stream finished with unexpected stop reason: {other}"),
			None => bail!("bedrock stream ended without a stop reason"),
		}
		let tool_calls: Vec<ToolCallSpec> = calls
			.into_iter()
			.map(|(id, (name, input_json))| {
				let input: Value = if input_json.trim().is_empty() {
					json!({})
				} else {
					serde_json::from_str(&input_json).unwrap_or(json!({}))
				};
				ToolCallSpec { id, name, input }
			})
			.collect();
		// WP-802: stream-end events (upstream assistantMessageEvent parity;
		// tool-call deltas are batched at end for non-OpenAI drivers).
		if text_started {
			on_delta(StreamDelta::TextEnd { index: 0, content: text.clone() });
		}
		for (i, tc) in tool_calls.iter().enumerate() {
			let idx = i as u32;
			on_delta(StreamDelta::ToolCallStart { index: idx, id: tc.id.clone(), name: tc.name.clone(), initial_args: Value::Null });
			on_delta(StreamDelta::ToolCallEnd {
				index: idx,
				tool_call: json!({"id":tc.id,"name":tc.name,"arguments":tc.input.clone()}),
			});
		}
		Ok(AssistantTurn {
			text: if text.is_empty() { None } else { Some(text) },
			tool_calls,
			usage,
		})
	}
}

/// Read the `:event-type` string header from an AWS event header section.
fn event_type_of(headers: &[u8]) -> String {
	// AWS event header layout: name_len | name | type | value_len | value.
	let mut pos = 0;
	let mut event_type = String::new();
	while pos < headers.len() {
		let name_len = headers[pos] as usize;
		pos += 1;
		if pos + name_len > headers.len() {
			break;
		}
		let name = String::from_utf8_lossy(&headers[pos..pos + name_len]).to_string();
		pos += name_len;
		if pos + 2 > headers.len() {
			break;
		}
		pos += 1; // type (0 = string; only string headers are used here)
		let value_len = headers[pos] as usize;
		pos += 1;
		if pos + value_len > headers.len() {
			break;
		}
		if name == ":event-type" {
			event_type = String::from_utf8_lossy(&headers[pos..pos + value_len]).to_string();
		}
		pos += value_len;
	}
	event_type
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
	let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key size");
	mac.update(data);
	let out = mac.finalize().into_bytes();
	let mut arr = [0u8; 32];
	arr.copy_from_slice(&out);
	arr
}

fn hex_sha256(data: &[u8]) -> String {
	let mut h = Sha256::new();
	h.update(data);
	hex(&h.finalize().to_vec())
}

fn hex(b: &[u8]) -> String {
	b.iter().map(|x| format!("{x:02x}")).collect()
}

impl ModelDriver for BedrockDriver {
	fn complete<'a>(&'a self, convo: &'a [Value], on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
		Box::pin(async move { self.complete_impl(convo, on_delta).await })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::provider::mock;

	fn driver() -> BedrockDriver {
		BedrockDriver::new(
			"us-east-1".into(),
			"AKIAIOSFODNN7EXAMPLE".into(),
			"wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
			None,
			"anthropic.claude-3-5-sonnet-v2:0".into(),
		)
	}

	/// Reference vectors computed independently (Python hmac/sha256, same
	/// canonical construction).
	#[test]
	fn sigv4_matches_reference_vectors() {
		let d = driver();
		let headers = vec![
			("host".to_string(), "bedrock-runtime.us-east-1.amazonaws.com".to_string()),
			("x-amz-date".to_string(), "20150830T123600Z".to_string()),
			("content-type".to_string(), "application/json".to_string()),
		];
		let payload = "{\"modelId\":\"anthropic.claude-3-5-sonnet\",\"messages\":[{\"role\":\"user\",\"content\":[{\"text\":\"hi\"}]}]}";
		let (auth, hdrs) = d.sign("POST", "/model/anthropic.claude-3-5-sonnet-v2:0/converse-stream", &headers, payload, "20150830T123600Z");
		assert_eq!(
			auth,
			"AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20150830/us-east-1/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=f58e1f19a231ce6b8e61cc11bb3ef6a1fbc812065e1df1fe45b3a7c1114220d3",
			"{auth}"
		);
		assert_eq!(hdrs.len(), 2); // no session token
	}

	#[test]
	fn sigv4_session_token_signed_and_emitted() {
		let d = BedrockDriver::new("eu-west-1".into(), "AK".into(), "secret2".into(), Some("tok123".into()), "m".into());
		let headers = vec![
			("host".to_string(), "bedrock-runtime.eu-west-1.amazonaws.com".to_string()),
			("x-amz-date".to_string(), "20260101T000000Z".to_string()),
			("content-type".to_string(), "application/json".to_string()),
			("x-amz-security-token".to_string(), "tok123".to_string()),
		];
		let (auth, hdrs) = d.sign("POST", "/model/m/converse-stream", &headers, "{}", "20260101T000000Z");
		assert_eq!(
			auth,
			"AWS4-HMAC-SHA256 Credential=AK/20260101/eu-west-1/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, Signature=3e85f46ce18c7c4a55d92664780b4bc7fbab59f66733f29ef0226e83baadf25a",
			"{auth}"
		);
		assert!(hdrs.iter().any(|(k, v)| k == "x-amz-security-token" && v == "tok123"));
	}

	/// Build a raw AWS event stream from (event type, JSON payload) pairs.
	fn event_stream(events: &[(&str, &str)]) -> Vec<u8> {
		let mut out = Vec::new();
		for (etype, payload) in events {
			let mut headers = Vec::new();
			let name = b":event-type";
			headers.push(name.len() as u8);
			headers.extend_from_slice(name);
			headers.push(0u8); // header type: string
			headers.push(etype.len() as u8);
			headers.extend_from_slice(etype.as_bytes());
			let total = 16 + headers.len() + 8 + payload.len();
			out.extend_from_slice(&(total as u32).to_be_bytes());
			out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
			out.extend_from_slice(&[0u8; 8]); // prelude CRC (not verified in v0)
			out.extend_from_slice(&headers);
			out.extend_from_slice(&[0u8; 8]); // message CRC
			out.extend_from_slice(payload.as_bytes());
		}
		out
	}

	/// Build a full HTTP response with the binary event-stream body. The
	/// test payloads are valid UTF-8 (NULs are legal), so a String transport
	/// is lossless.
	fn raw_response(body: &[u8]) -> Vec<u8> {
		// Byte-exact: totals >= 128 contain raw high bytes, so no String
		// transport (from_utf8_lossy would corrupt the framing).
		let mut out = format!(
			"HTTP/1.1 200 OK\r\ncontent-type: application/vnd.amazon.eventstream\r\ncontent-length: {}\r\n\r\n",
			body.len()
		)
		.into_bytes();
		out.extend_from_slice(body);
		out
	}

	#[tokio::test]
	async fn assembles_text_tool_use_and_usage() {
		let events = event_stream(&[
			("messageStart", r#"{"messageStart":{"role":"assistant"}}"#),
			("contentBlockDelta", r#"{"contentBlockDelta":{"delta":{"text":"Hel"}}}"#),
			("contentBlockDelta", r#"{"contentBlockDelta":{"delta":{"text":"lo"}}}"#),
			("contentBlockStart", r#"{"contentBlockStart":{"start":{"toolUse":{"toolUseId":"tu1","name":"bash"}}}}"#),
			("contentBlockDelta", r#"{"contentBlockDelta":{"delta":{"toolUse":{"toolUseId":"tu1","name":"bash","input":"{\"command\":"}}}}"#),
			("contentBlockDelta", r#"{"contentBlockDelta":{"delta":{"toolUse":{"toolUseId":"tu1","input":"\"ls\"}"}}}}"#),
			("messageStop", r#"{"messageStop":{"stopReason":"tool_use"}}"#),
			("metadata", r#"{"metadata":{"usage":{"inputTokens":12,"outputTokens":7,"cacheReadInputTokens":3}}}"#),
		]);
		let (tx, mut rx) = tokio::sync::mpsc::channel(1);
		let base = mock::start_mock_bytes(raw_response(&events), tx).await;
		let d = BedrockDriver::with_base(base, "us-east-1".into(), "AK".into(), "SK".into(), None, "anthropic.claude-3-5-sonnet-v2:0".into());
		let turn = d.complete(&mock::test_convo(), Box::new(|_: crate::model::StreamDelta| {})).await.unwrap();
		assert_eq!(turn.text.as_deref(), Some("Hello"));
		assert_eq!(turn.tool_calls, vec![ToolCallSpec { id: "tu1".into(), name: "bash".into(), input: json!({"command":"ls"}) }]);
		assert_eq!(turn.usage.input, 12);
		assert_eq!(turn.usage.output, 7);
		assert_eq!(turn.usage.cache_read, 3);
		// The signed request must carry the converse path and SigV4 headers.
		let recorded = rx.recv().await.unwrap();
		let first = recorded.lines().next().unwrap_or_default().to_string();
		assert!(first.contains("/model/anthropic.claude-3-5-sonnet-v2:0/converse-stream"), "{first}");
		assert!(recorded.contains("x-amz-date:"), "{recorded}");
		assert!(recorded.contains("authorization: aws4-hmac-sha256"), "lowercased headers expected: {recorded}");
	}

	#[tokio::test]
	async fn non_2xx_maps_to_error() {
		let body = r#"{"message":"Unauthorized access"}"#;
		let resp = format!("HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
		let (tx, _rx) = tokio::sync::mpsc::channel(1);
		let base = mock::start_mock(resp, tx).await;
		let d = BedrockDriver::with_base(base, "us-east-1".into(), "AK".into(), "SK".into(), None, "m".into());
		let err = d.complete(&mock::test_convo(), Box::new(|_: crate::model::StreamDelta| {})).await.unwrap_err().to_string();
		assert!(err.contains("403"), "{err}");
		assert!(err.contains("Unauthorized access"), "{err}");
	}

	#[tokio::test]
	async fn stream_exception_event_maps_to_error() {
		let events = event_stream(&[
			("messageStart", r#"{"messageStart":{"role":"assistant"}}"#),
			("validationException", r#"{"validationException":{"message":"messages.0.content: field required"}}"#),
		]);
		let (tx, _rx) = tokio::sync::mpsc::channel(1);
		let base = mock::start_mock_bytes(raw_response(&events), tx).await;
		let d = BedrockDriver::with_base(base, "us-east-1".into(), "AK".into(), "SK".into(), None, "m".into());
		let err = d.complete(&mock::test_convo(), Box::new(|_: crate::model::StreamDelta| {})).await.unwrap_err().to_string();
		assert!(err.contains("field required"), "{err}");
	}
}
