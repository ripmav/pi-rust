//! Anthropic native `messages` driver (streaming, tool use).
//! `x-api-key` + `anthropic-version` headers; `system` as top-level field;
//! `toolResult` maps to user-role `tool_result` blocks; consecutive same-role
//! messages are merged (the API requires strict alternation).
use crate::model::{AssistantTurn, ModelDriver, ToolCallSpec};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

#[derive(Debug)]
pub struct AnthropicDriver {
	base_url: String,
	api_key: String,
	model: String,
	client: reqwest::Client,
}

impl AnthropicDriver {
	pub fn new(base_url: String, api_key: String, model: String) -> Self {
		Self { base_url, api_key, model, client: reqwest::Client::new() }
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

	/// Maps the pi-format conversation to Anthropic messages with strict
	/// user/assistant alternation (consecutive same-role entries are merged).
	fn map_messages(convo: &[Value]) -> Vec<Value> {
		let mut out: Vec<Value> = Vec::new();
		for m in convo {
			let (role, content) = match m.get("role").and_then(Value::as_str) {
				Some("user") => ("user", json!([json!({"type":"text","text":Self::text_of(m.get("content"))})])),
				Some("assistant") => {
					let mut blocks = Vec::new();
					if let Some(src) = m.get("content").and_then(Value::as_array) {
						for b in src {
							match b.get("type").and_then(Value::as_str) {
								Some("text") => blocks.push(json!({"type":"text","text":b.get("text").cloned().unwrap_or(Value::Null)})),
								Some("toolCall") => blocks.push(json!({
									"type": "tool_use",
									"id": b.get("id").cloned().unwrap_or(Value::Null),
									"name": b.get("name").cloned().unwrap_or(Value::Null),
									"input": b.get("input").or_else(|| b.get("arguments")).cloned().unwrap_or(json!({})),
								})),
								_ => {}
							}
						}
					}
					("assistant", json!(blocks))
				}
				Some("toolResult") => (
					"user",
					json!([{
						"type": "tool_result",
						"tool_use_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
						"content": Self::text_of(m.get("content")),
						"is_error": m.get("isError").and_then(Value::as_bool).unwrap_or(false),
					}]),
				),
				_ => continue,
			};
			let msg = json!({"role": role, "content": content});
			if out.last().is_some_and(|p| p["role"] == msg["role"]) {
				// merge: concatenate block arrays
				let merged = out
					.last_mut()
					.unwrap()["content"]
					.as_array_mut()
					.unwrap()
					.drain(..)
					.chain(msg["content"].as_array().unwrap().iter().cloned())
					.collect::<Vec<_>>();
				out.last_mut().unwrap()["content"] = json!(merged);
			} else {
				out.push(msg);
			}
		}
		out
	}

	fn system_from(convo: &[Value]) -> Option<String> {
		convo
			.iter()
			.find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
			.and_then(|m| m.get("content").and_then(Value::as_str))
			.map(str::to_string)
	}

	fn tools_from(convo: &[Value]) -> Vec<Value> {
		let defs = convo
			.iter()
			.find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
			.and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
			.unwrap_or_default();
		defs
			.iter()
			.filter_map(|d| {
				d.get("name").and_then(Value::as_str).map(|name| {
					json!({
						"name": name,
						"description": d.get("description").cloned().unwrap_or(Value::Null),
						"input_schema": d.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
					})
				})
			})
			.collect()
	}

	async fn complete_impl(&self, convo: &[Value]) -> Result<AssistantTurn> {
		let mut body = json!({
			"model": self.model,
			"max_tokens": DEFAULT_MAX_TOKENS,
			"stream": true,
			"messages": Self::map_messages(convo),
		});
		if let Some(sys) = Self::system_from(convo) {
			body["system"] = json!(sys);
		}
		let tools = Self::tools_from(convo);
		if !tools.is_empty() {
			body["tools"] = json!(tools);
		}
		let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
		let resp = self
			.client
			.post(&url)
			.header("x-api-key", &self.api_key)
			.header("anthropic-version", ANTHROPIC_VERSION)
			.json(&body)
			.send()
			.await
			.with_context(|| format!("anthropic request failed: {url}"))?;
		if !resp.status().is_success() {
			let status = resp.status();
			let text = resp.text().await.unwrap_or_default();
			bail!("anthropic api error {status}: {text}");
		}
		// per content-block index: ("text", text) | ("tool", id, name, partial_json)
		#[derive(Default)]
		struct Block {
			kind: u8, // 0=unknown, 1=text, 2=tool
			text: String,
			id: String,
			name: String,
			partial: String,
		}
		let mut blocks: BTreeMap<u32, Block> = BTreeMap::new();
		let mut usage = crate::model::Usage::default();
		let mut buf = String::new();
		let mut event = String::new();
		let mut stream = resp.bytes_stream();
		while let Some(chunk) = stream.next().await {
			let chunk = chunk.context("anthropic stream error")?;
			buf.push_str(&String::from_utf8_lossy(&chunk));
			while let Some(pos) = buf.find('\n') {
				let line = buf[..pos].to_string();
				buf.drain(..=pos);
				let line = line.trim_end_matches('\r').trim();
				if line.is_empty() {
					event.clear();
					continue;
				}
				if let Some(e) = line.strip_prefix("event:") {
					event = e.trim().to_string();
					continue;
				}
				let Some(payload) = line.strip_prefix("data:") else { continue };
				let payload = payload.trim();
				let Ok(v) = serde_json::from_str::<Value>(payload) else { continue };
				match event.as_str() {
					"error" => bail!("anthropic stream error: {}", v.get("error").cloned().unwrap_or(v)),
					"content_block_start" => {
						let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
						let cb = v.get("content_block").cloned().unwrap_or(Value::Null);
						if cb.get("type").and_then(Value::as_str) == Some("tool_use") {
							let b = blocks.entry(idx).or_default();
							b.kind = 2;
							b.id = cb.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
							b.name = cb.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
						} else {
							let b = blocks.entry(idx).or_default();
							b.kind = 1;
						}
					}
					"content_block_delta" => {
						let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
						let delta = v.get("delta").cloned().unwrap_or(Value::Null);
						let b = blocks.entry(idx).or_default();
						match delta.get("type").and_then(Value::as_str) {
							Some("text_delta") => b.text.push_str(delta.get("text").and_then(Value::as_str).unwrap_or_default()),
							Some("input_json_delta") => b.partial.push_str(delta.get("partial_json").and_then(Value::as_str).unwrap_or_default()),
							_ => {}
						}
					}
					"message_start" => {
						if let Some(u) = v.pointer("/message/usage") {
							usage.input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
							usage.cache_read = u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0);
							usage.cache_write = u.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0);
						}
						}
						"message_delta" => {
							usage.output = v.pointer("/usage/output_tokens").and_then(Value::as_u64).unwrap_or(0);
						}
						_ => {} // message_stop, content_block_stop
				}
			}
		}
		let mut text = String::new();
		let mut tool_calls = Vec::new();
		for (_, b) in blocks {
			match b.kind {
				1 if !b.text.is_empty() => {
					if !text.is_empty() {
						text.push(' ');
					}
					text.push_str(&b.text);
				}
				2 => {
					let input = if b.partial.trim().is_empty() {
						json!({})
					} else {
						serde_json::from_str(&b.partial).unwrap_or(json!({}))
					};
					tool_calls.push(ToolCallSpec { id: b.id, name: b.name, input });
				}
				_ => {}
			}
		}
		Ok(AssistantTurn {
		text: if text.is_empty() { None } else { Some(text) },
		tool_calls,
		usage,
	})
	}
}

impl ModelDriver for AnthropicDriver {
	fn complete<'a>(&'a self, convo: &'a [Value]) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
		Box::pin(async move { self.complete_impl(convo).await })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::provider::mock;

	#[tokio::test]
	async fn captures_usage_from_start_and_delta() {
		let (tx, _rx) = tokio::sync::mpsc::channel(1);
		let sse = concat!(
			"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":42,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":2}}}\n\n",
			"event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
			"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"yo\"}}\n\n",
			"event: content_block_stop\ndata: {\"index\":0}\n\n",
			"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n",
		);
		let base = mock::start_mock(mock::sse_response(sse), tx).await;
		let d = AnthropicDriver::new(base, "k".into(), "m".into());
		let turn = d.complete(&mock::test_convo()).await.unwrap();
		assert_eq!(turn.usage.input, 42);
		assert_eq!(turn.usage.output, 9);
		assert_eq!(turn.usage.cache_read, 5);
		assert_eq!(turn.usage.cache_write, 2);
	}

	#[tokio::test]
	async fn maps_request_headers_and_shape() {
		let (tx, mut rx) = tokio::sync::mpsc::channel(1);
		let sse = "event: message_stop\ndata: {}\n\n";
		let base = mock::start_mock(mock::sse_response(sse), tx).await;
		let d: Box<dyn ModelDriver> = Box::new(AnthropicDriver::new(base, "key-1".into(), "claude-test".into()));
		let turn = d.complete(&mock::test_convo()).await.unwrap();
		assert_eq!(turn.text, None);
		let req = rx.recv().await.unwrap();
		let lines: Vec<&str> = req.lines().collect();
		let sep = lines.iter().position(|l| *l == "---").unwrap();
		let headers = &lines[1..sep];
		let body: Value = serde_json::from_str(&lines[sep + 1..].join("\n")).unwrap();
		assert_eq!(lines[0], "POST /v1/messages");
		assert!(headers.contains(&"x-api-key: key-1"), "{headers:?}");
		assert!(headers.contains(&"anthropic-version: 2023-06-01"), "{headers:?}");
		assert_eq!(body["model"], "claude-test");
		assert_eq!(body["stream"], true);
		assert_eq!(body["system"], "SYS PROMPT");
		let msgs = body["messages"].as_array().unwrap();
		// user, assistant(tool_use), toolResult->user(tool_result) — alternation kept
		assert_eq!(msgs[0]["role"], "user");
		assert_eq!(msgs[0]["content"][0]["text"], "do it");
		assert_eq!(msgs[1]["role"], "assistant");
		assert_eq!(msgs[1]["content"][0]["type"], "text");
		assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
		assert_eq!(msgs[1]["content"][1]["name"], "bash");
		assert_eq!(msgs[2]["role"], "user");
		assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
		assert_eq!(msgs[2]["content"][0]["tool_use_id"], "tc_9");
		assert_eq!(body["tools"][0]["name"], "bash");
		assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
	}

	#[tokio::test]
	async fn assembles_text_blocks_and_tool_use() {
		let (tx, _rx) = tokio::sync::mpsc::channel(1);
		let sse = concat!(
			"event: message_start\ndata: {\"message\":{\"id\":\"m1\"}}\n\n",
			"event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
			"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Pi \"}}\n\n",
			"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"rocks\"}}\n\n",
			"event: content_block_stop\ndata: {\"index\":0}\n\n",
			"event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"hello\"}}\n\n",
			"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"na\"}}\n\n",
			"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"me\\\":\\\"X\\\"}\"}}\n\n",
			"event: content_block_stop\ndata: {\"index\":1}\n\n",
			"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
			"event: message_stop\ndata: {}\n\n",
		);
		let base = mock::start_mock(mock::sse_response(sse), tx).await;
		let d = AnthropicDriver::new(base, "k".into(), "m".into());
		let turn = d.complete(&mock::test_convo()).await.unwrap();
		assert_eq!(turn.text.as_deref(), Some("Pi rocks"));
		assert_eq!(turn.tool_calls, vec![ToolCallSpec { id: "tu_1".into(), name: "hello".into(), input: json!({"name":"X"}) }]);
	}

	#[tokio::test]
	async fn non_2xx_maps_to_error() {
		let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#;
		let resp = format!("HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
		let (tx, _rx) = tokio::sync::mpsc::channel(1);
		let base = mock::start_mock(resp, tx).await;
		let d = AnthropicDriver::new(base, "k".into(), "m".into());
		let err = d.complete(&mock::test_convo()).await.unwrap_err().to_string();
		assert!(err.contains("400"), "{err}");
		assert!(err.contains("bad model"), "{err}");
	}
}
