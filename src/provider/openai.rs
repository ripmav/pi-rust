//! OpenAI-compatible `chat/completions` driver (streaming, tool calls).
//! Covers OpenAI itself, local servers, and compatible endpoints (solheim/qwen-style)
//! via `base_url`. The conversation arrives in pi format; tools come from the
//! system message's `toolsAdded` (built by `build_conversation`).
use crate::model::{AssistantTurn, ModelDriver, StreamDelta, ToolCallSpec};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug)]
pub struct OpenAiDriver {
    base_url: String,
    api_key: String,
    model: String,
    /// Azure variant: deployment path + `api-version` query param and the
    /// `api-key` header instead of bearer (upstream azure-openai-config.ts).
    azure: Option<(String, String)>,
    client: reqwest::Client,
}

impl OpenAiDriver {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url,
            api_key,
            model,
            azure: None,
            client: reqwest::Client::new(),
        }
    }

    /// `deployment` replaces the model in the URL path (the body keeps the
    /// catalog model id, per upstream `withDeploymentName`).
    pub fn new_azure(
        base_url: String,
        api_key: String,
        model: String,
        deployment: String,
        api_version: String,
    ) -> Self {
        Self {
            base_url,
            api_key,
            model,
            azure: Some((deployment, api_version)),
            client: reqwest::Client::new(),
        }
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

    fn map_messages(convo: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        for m in convo {
            match m.get("role").and_then(Value::as_str) {
                Some("system") => {
                    if let Some(c) = m.get("content").and_then(Value::as_str) {
                        out.push(json!({"role":"system","content":c}));
                    }
                }
                Some("user") => {
                    // Images: pi-ai image blocks -> OpenAI `image_url` data-URL
                    // parts (S8); text stays a string when there are no images.
                    let images: Vec<(String, String)> = m
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                                .filter_map(|b| {
                                    Some((
                                        b.get("mimeType")
                                            .and_then(Value::as_str)
                                            .unwrap_or("image/png")
                                            .to_string(),
                                        b.get("data").and_then(Value::as_str)?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if images.is_empty() {
                        out.push(json!({"role":"user","content":Self::text_of(m.get("content"))}));
                    } else {
                        let mut parts: Vec<Value> = Vec::new();
                        let text = Self::text_of(m.get("content"));
                        if !text.is_empty() {
                            parts.push(json!({"type":"text","text":text}));
                        }
                        for (mime, data) in &images {
                            parts.push(json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{data}")}}));
                        }
                        out.push(json!({"role":"user","content":parts}));
                    }
                }
                Some("assistant") => {
                    let mut text = String::new();
                    let mut tool_calls = Vec::new();
                    if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                        for b in blocks {
                            match b.get("type").and_then(Value::as_str) {
								Some("text") => {
									let t = b.get("text").and_then(Value::as_str).unwrap_or_default();
									if !text.is_empty() {
										text.push(' ');
									}
									text.push_str(t);
								}
								Some("toolCall") => tool_calls.push(json!({
									"id": b.get("id").cloned().unwrap_or(Value::Null),
									"type": "function",
									"function": {
										"name": b.get("name").cloned().unwrap_or(Value::Null),
										"arguments": b.get("input").or_else(|| b.get("arguments")).map(|v| v.to_string()).unwrap_or_default(),
									},
								})),
								_ => {}
							}
                        }
                    }
                    let mut msg = json!({"role":"assistant","content": if text.is_empty() { Value::Null } else { Value::String(text) }});
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = json!(tool_calls);
                    }
                    out.push(msg);
                }
                Some("toolResult") => out.push(json!({
                    "role": "tool",
                    "tool_call_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
                    "content": Self::text_of(m.get("content")),
                })),
                _ => {} // custom/unknown roles are skipped in v0
            }
        }
        out
    }

    fn tools_from(convo: &[Value]) -> Vec<Value> {
        let defs = convo
            .iter()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
            .and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        defs.iter()
            .filter_map(|d| {
                d.get("name").and_then(Value::as_str).map(|name| {
					json!({
						"type": "function",
						"function": {
							"name": name,
							"description": d.get("description").cloned().unwrap_or(Value::Null),
							"parameters": d.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
						},
					})
				})
            })
            .collect()
    }

    async fn complete_impl<'a>(
        &'a self,
        convo: &'a [Value],
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Result<AssistantTurn> {
        let tools = Self::tools_from(convo);
        let mut body = json!({"model":self.model,"stream":true,"stream_options":{"include_usage":true},"messages":Self::map_messages(convo)});
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        let (url, use_azure_key) = match &self.azure {
            Some((dep, ver)) => (
                format!(
                    "{}/deployments/{}/chat/completions?api-version={}",
                    self.base_url.trim_end_matches('/'),
                    dep,
                    ver
                ),
                true,
            ),
            None => (
                format!("{}/chat/completions", self.base_url.trim_end_matches('/')),
                false,
            ),
        };
        let mut req = self.client.post(&url).json(&body);
        if use_azure_key {
            req = req.header("api-key", &self.api_key);
        } else {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("openai request failed: {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("openai api error {status}: {text}");
        }
        let mut text = String::new();
        let mut text_started = false;
        let mut usage = crate::model::Usage::default();
        // index -> (id, name, accumulated argument JSON string)
        let mut calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        let mut buf = String::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("openai stream error")?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf.drain(..=pos);
                let line = line.trim_end_matches('\r').trim();
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                if let Some(err) = v.get("error") {
                    bail!("openai stream error: {err}");
                }
                if let Some(u) = v.get("usage") {
                    usage.input = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
                    usage.output = u
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    usage.cache_read = u
                        .pointer("/prompt_tokens_details/cached_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                }
                let Some(delta) = v.pointer("/choices/0/delta") else {
                    continue;
                };
                if let Some(t) = delta.get("content").and_then(Value::as_str) {
                    if !text_started {
                        text_started = true;
                        on_delta(StreamDelta::TextStart { index: 0 });
                    }
                    text.push_str(t);
                    on_delta(StreamDelta::TextDelta {
                        index: 0,
                        delta: t.to_string(),
                    }); // WP-603: TUI renders tokens as they arrive
                }
                if let Some(items) = delta.get("tool_calls").and_then(Value::as_array) {
                    for c in items {
                        let idx = c.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                        let e = calls.entry(idx).or_default();
                        let (had_id, had_name) = (!e.0.is_empty(), !e.1.is_empty());
                        if let Some(id) = c.get("id").and_then(Value::as_str) {
                            e.0 = id.to_string();
                        }
                        if let Some(n) = c.pointer("/function/name").and_then(Value::as_str) {
                            e.1 = n.to_string();
                        }
                        let fragment = c
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let first_seen =
                            (!had_id || !had_name) && !e.0.is_empty() && !e.1.is_empty();
                        if first_seen {
                            // WP-802: toolcall_start once id + name are known
                            // (the content index offsets past a text block);
                            // the same chunk's arguments fragment is the
                            // initial partial the upstream `start` event sees.
                            if let Some(f) = &fragment {
                                e.2.push_str(f);
                            }
                            let ci = if text.is_empty() { idx } else { idx + 1 };
                            let initial = if e.2.trim().is_empty() {
                                None
                            } else {
                                serde_json::from_str::<Value>(e.2.as_str()).ok()
                            };
                            on_delta(StreamDelta::ToolCallStart {
                                index: ci,
                                id: e.0.clone(),
                                name: e.1.clone(),
                                initial_args: initial.unwrap_or(Value::Null),
                            });
                        }
                        if let Some(f) = &fragment {
                            if !first_seen {
                                e.2.push_str(f);
                            }
                            let ci = if text.is_empty() { idx } else { idx + 1 };
                            on_delta(StreamDelta::ToolCallDelta {
                                index: ci,
                                delta: f.clone(),
                            });
                        }
                    }
                }
            }
        }
        // WP-802: stream-end events (upstream `assistantMessageEvent` parity).
        if text_started {
            on_delta(StreamDelta::TextEnd {
                index: 0,
                content: text.clone(),
            });
        }
        for (idx, (id, name, args)) in &calls {
            let parsed = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(args).unwrap_or(json!({}))
            };
            let ci = if text.is_empty() { *idx } else { *idx + 1 };
            on_delta(StreamDelta::ToolCallEnd {
                index: ci,
                tool_call: json!({"type":"toolCall","id":id,"name":name,"arguments":parsed}),
            });
        }
        let tool_calls = calls
            .into_iter()
            .map(|(_, (id, name, args))| ToolCallSpec {
                id,
                name,
                input: if args.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&args).unwrap_or(json!({}))
                },
            })
            .collect();
        Ok(AssistantTurn {
            text: if text.is_empty() { None } else { Some(text) },
            tool_calls,
            usage,
        })
    }
}

impl ModelDriver for OpenAiDriver {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        Box::pin(async move { self.complete_impl(convo, on_delta).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock;

    #[tokio::test]
    async fn maps_conversation_tools_and_auth() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let sse = concat!(
			"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n",
			"\n",
			"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n",
			"\n",
			"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"tc_1\",\"type\":\"function\",\"function\":{\"name\":\"hello\",\"arguments\":\"{\\\"na\"}}]}}]}\n",
			"\n",
			"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"me\\\":\\\"X\\\"}\"}}]}}]}\n",
			"\n",
			"data: [DONE]\n",
			"\n",
		);
        let base = mock::start_mock(mock::sse_response(sse), tx).await;
        let d = OpenAiDriver::new(base, "k".into(), "m".into());
        let turn = d
            .complete(
                &mock::test_convo(),
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .unwrap();
        assert_eq!(turn.text.as_deref(), Some("Hello"));
        assert_eq!(
            turn.tool_calls,
            vec![ToolCallSpec {
                id: "tc_1".into(),
                name: "hello".into(),
                input: json!({"name":"X"})
            }]
        );
    }

    #[tokio::test]
    async fn emits_one_delta_per_sse_text_chunk() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}
",
            "\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}
",
            "\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}
",
            "\n",
            "data: [DONE]
",
            "\n",
        );
        let base = mock::start_mock(mock::sse_response(sse), tx).await;
        let d = OpenAiDriver::new(base, "k".into(), "m".into());
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let got2 = got.clone();
        let turn = d
            .complete(
                &mock::test_convo(),
                Box::new(move |ev: crate::model::StreamDelta| {
                    if let crate::model::StreamDelta::TextDelta { delta, .. } = &ev {
                        got2.lock().unwrap().push(delta.clone());
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(turn.text.as_deref(), Some("Hello world"));
        // WP-603: one delta per streamed chunk, in order.
        assert_eq!(
            *got.lock().unwrap(),
            vec!["Hel".to_string(), "lo".to_string(), " world".to_string()]
        );
    }

    #[tokio::test]
    async fn captures_usage_and_requests_include_usage() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sse = concat!(
			"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hi\"}}]}\n",
			"\n",
			"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n",
			"\n",
			"data: [DONE]\n",
			"\n",
		);
        let base = mock::start_mock(mock::sse_response(sse), tx).await;
        let d = OpenAiDriver::new(base, "k".into(), "m".into());
        let turn = d
            .complete(
                &mock::test_convo(),
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .unwrap();
        assert_eq!(turn.usage.input, 11);
        assert_eq!(turn.usage.output, 7);
        assert_eq!(turn.usage.cache_read, 3);
        let recorded = rx.recv().await.unwrap();
        assert!(recorded.contains("include_usage"), "{recorded}");
    }

    #[tokio::test]
    async fn non_2xx_maps_to_error() {
        let body = r#"{"error":{"message":"boom"}}"#;
        let resp = format!("HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let base = mock::start_mock(resp, tx).await;
        let d = OpenAiDriver::new(base, "k".into(), "m".into());
        let err = d
            .complete(
                &mock::test_convo(),
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "{err}");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn user_image_becomes_data_url_part() {
        let convo = vec![json!({"role":"user","content":[
            {"type":"text","text":"describe"},
            {"type":"image","mimeType":"image/png","data":"aW1hZ2U="}
        ]})];
        let msgs = OpenAiDriver::map_messages(&convo);
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"text","text":"describe"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,aW1hZ2U="}}
            ])
        );
        // plain text users keep a string content
        let plain = OpenAiDriver::map_messages(&[json!({"role":"user","content":"hi"})]);
        assert_eq!(plain[0]["content"], json!("hi"));
    }

    #[tokio::test]
    async fn azure_uses_deployment_path_api_version_and_api_key_header() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n",
            "\n",
            "data: [DONE]\n",
            "\n",
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let base = mock::start_mock(mock::sse_response(sse), tx).await;
        let d = OpenAiDriver::new_azure(
            format!("{base}/openai/v1"),
            "az-key".into(),
            "gpt-4o".into(),
            "dep-gpt".into(),
            "2024-10-21".into(),
        );
        let turn = d
            .complete(
                &mock::test_convo(),
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .unwrap();
        assert_eq!(turn.text.as_deref(), Some("ok"));
        let recorded = rx.recv().await.unwrap();
        let first_line = recorded.lines().next().unwrap_or_default().to_string();
        assert!(
            first_line
                .contains("/openai/v1/deployments/dep-gpt/chat/completions?api-version=2024-10-21"),
            "{}",
            first_line
        );
        let lower = recorded.to_lowercase();
        assert!(lower.contains("api-key: az-key"), "{recorded}");
        assert!(
            !lower.contains("authorization"),
            "azure must not send bearer: {recorded}"
        );
    }
}
