//! Google Generative AI (Gemini) driver: `generateContent` streaming (SSE),
//! text + `functionCall` assembly, wire shapes per upstream
//! `packages/ai/src/api/google-generative-ai.ts` (pinned 17d6ca7).
//! `google-vertex` reuses the same wire with the project/location URL and an
//! API key (ADC/bearer is a typed v0 rejection at build time).
use crate::model::{AssistantTurn, ModelDriver, StreamDelta, ToolCallSpec, Usage};
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug)]
pub struct GoogleDriver {
    /// Google: `https://generativelanguage.googleapis.com/v1beta`;
    /// Vertex: `https://{location}-aiplatform.googleapis.com/v1/projects/{p}/locations/{l}/publishers/google`
    /// (a custom base may already carry the full path).
    base_url: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl GoogleDriver {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url,
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    /// pi-format conversation -> Gemini `contents` (system goes to
    /// `systemInstruction`, consecutive toolResults merge into one user turn,
    /// per upstream `convertMessages`).
    fn map_contents(convo: &[Value]) -> (Option<Value>, Vec<Value>) {
        let mut system = String::new();
        let mut contents: Vec<Value> = Vec::new();
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
                    contents.push(json!({"role":"user","parts":[{"text":text}]}));
                }
                Some("assistant") => {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                        for b in blocks {
                            match b.get("type").and_then(Value::as_str) {
								Some("text") => {
									let t = b.get("text").and_then(Value::as_str).unwrap_or_default().trim();
									if t.is_empty() {
										continue;
									}
									parts.push(json!({"text":t}));
								}
								Some("toolCall") => parts.push(json!({"functionCall":{
									"name":b.get("name").cloned().unwrap_or(Value::Null),
									"args":b.get("input").or_else(|| b.get("arguments")).cloned().unwrap_or(json!({})),
								}})),
								_ => {} // thinking blocks are dropped in v0
							}
                        }
                    }
                    if parts.is_empty() {
                        continue;
                    }
                    contents.push(json!({"role":"model","parts":parts}));
                }
                Some("toolResult") => {
                    let text = Self::text_of(m.get("content"));
                    let is_err = m.get("isError").and_then(Value::as_bool).unwrap_or(false);
                    let response = if is_err {
                        json!({"error":text})
                    } else {
                        json!({"output":text})
                    };
                    let part = json!({"functionResponse":{
                        "name":m.get("toolName").cloned().unwrap_or(Value::Null),
                        "response":response,
                    }});
                    // Upstream: merge into a preceding user turn that already
                    // carries function responses.
                    if let Some(last) = contents.last_mut() {
                        if last.get("role").and_then(Value::as_str) == Some("user")
                            && last
                                .get("parts")
                                .and_then(Value::as_array)
                                .is_some_and(|p| {
                                    p.iter().any(|x| x.get("functionResponse").is_some())
                                })
                        {
                            last["parts"].as_array_mut().unwrap().push(part);
                            continue;
                        }
                    }
                    contents.push(json!({"role":"user","parts":[part]}));
                }
                _ => {}
            }
        }
        let system_instruction = if system.trim().is_empty() {
            None
        } else {
            Some(json!({"parts":[{"text":system}]}))
        };
        (system_instruction, contents)
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
        let decls: Vec<Value> = defs
            .iter()
            .filter_map(|d| {
                d.get("name").and_then(Value::as_str).map(|name| {
					json!({
						"name": name,
						"description": d.get("description").cloned().unwrap_or(Value::Null),
						"parametersJsonSchema": d.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
					})
				})
            })
            .collect();
        if decls.is_empty() {
            None
        } else {
            Some(json!([{"functionDeclarations":decls}]))
        }
    }

    async fn complete_impl<'a>(
        &'a self,
        convo: &'a [Value],
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Result<AssistantTurn> {
        let (system_instruction, contents) = Self::map_contents(convo);
        let mut body = json!({"contents":contents});
        if let Some(si) = system_instruction {
            body["systemInstruction"] = si;
        }
        if let Some(tools) = Self::tools_from(convo) {
            body["tools"] = tools;
        }
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/'),
            self.model
        );
        let resp = super::send_with_retry(|| {
            self.client
                .post(&url)
                .header("x-goog-api-key", &self.api_key)
                .json(&body)
        })
        .await
        .map_err(|f| anyhow::anyhow!("{}", f.message("google")))?;
        let mut text = String::new();
        let mut text_started = false;
        let mut tool_calls: Vec<ToolCallSpec> = Vec::new();
        let mut usage = Usage::default();
        let mut finish_reason: Option<String> = None;
        let mut buf = String::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("google stream error")?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf.drain(..=pos);
                let line = line.trim_end_matches('\r').trim();
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                let candidate = v.pointer("/candidates/0");
                if let Some(parts) = candidate
                    .and_then(|c| c.pointer("/content/parts"))
                    .and_then(Value::as_array)
                {
                    for part in parts {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
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
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let id = fc
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .filter(|i| {
                                    !i.is_empty() && !tool_calls.iter().any(|tc| tc.id == *i)
                                })
                                .unwrap_or_else(|| format!("{name}_{}", tool_calls.len() + 1));
                            tool_calls.push(ToolCallSpec {
                                id,
                                name,
                                input: fc.get("args").cloned().unwrap_or(json!({})),
                            });
                        }
                    }
                }
                if let Some(fr) = candidate
                    .and_then(|c| c.get("finishReason"))
                    .and_then(Value::as_str)
                {
                    finish_reason = Some(fr.to_string());
                }
                if let Some(u) = v.get("usageMetadata") {
                    let prompt = u
                        .get("promptTokenCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cached = u
                        .get("cachedContentTokenCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let candidates = u
                        .get("candidatesTokenCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let thoughts = u
                        .get("thoughtsTokenCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    usage.input = prompt.saturating_sub(cached);
                    usage.output = candidates + thoughts;
                    usage.cache_read = cached;
                }
            }
        }
        match finish_reason.as_deref() {
            Some("STOP") | Some("MAX_TOKENS") | Some("SAFETY") | Some("RECITATION") => {}
            Some("TOOL_USE") => {
                if tool_calls.is_empty() {
                    bail!("google stream: TOOL_USE finish reason without function calls");
                }
            }
            Some(other) => bail!("google stream finished with unexpected reason: {other}"),
            None => bail!("google stream ended without a finish reason"),
        }
        // WP-802: stream-end events (upstream assistantMessageEvent parity;
        // tool-call deltas are batched at end for non-OpenAI drivers).
        if text_started {
            on_delta(StreamDelta::TextEnd {
                index: 0,
                content: text.clone(),
            });
        }
        for (i, tc) in tool_calls.iter().enumerate() {
            let idx = i as u32;
            on_delta(StreamDelta::ToolCallStart {
                index: idx,
                id: tc.id.clone(),
                name: tc.name.clone(),
                initial_args: Value::Null,
            });
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

impl ModelDriver for GoogleDriver {
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

    #[test]
    fn maps_conversation_to_contents() {
        let convo = vec![
            json!({"role":"system","content":"sys prompt","toolsAdded":[
                {"name":"bash","description":"run","parameters":{"type":"object"}}
            ]}),
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[
                {"type":"text","text":"calling"},
                {"type":"toolCall","id":"tc1","name":"bash","input":{"command":"ls"}}
            ]}),
            json!({"role":"toolResult","toolCallId":"tc1","toolName":"bash","content":[{"type":"text","text":"out1"}],"isError":false}),
            json!({"role":"toolResult","toolCallId":"tc2","toolName":"read","content":"err","isError":true}),
            json!({"role":"user","content":"again"}),
        ];
        let (si, contents) = GoogleDriver::map_contents(&convo);
        assert_eq!(si, Some(json!({"parts":[{"text":"sys prompt"}]})));
        assert_eq!(
            contents,
            vec![
                json!({"role":"user","parts":[{"text":"hi"}]}),
                json!({"role":"model","parts":[
                    {"text":"calling"},
                    {"functionCall":{"name":"bash","args":{"command":"ls"}}}
                ]}),
                // consecutive toolResults merge into ONE user turn (upstream)
                json!({"role":"user","parts":[
                    {"functionResponse":{"name":"bash","response":{"output":"out1"}}},
                    {"functionResponse":{"name":"read","response":{"error":"err"}}}
                ]}),
                json!({"role":"user","parts":[{"text":"again"}]}),
            ]
        );
    }

    #[test]
    fn tools_become_function_declarations() {
        let convo = vec![json!({"role":"system","content":"s","toolsAdded":[
            {"name":"bash","description":"run a shell command","parameters":{"type":"object","properties":{}}}
        ]})];
        assert_eq!(
            GoogleDriver::tools_from(&convo),
            Some(json!([{"functionDeclarations":[{
                "name":"bash","description":"run a shell command",
                "parametersJsonSchema":{"type":"object","properties":{}}
            }]}]))
        );
        assert_eq!(
            GoogleDriver::tools_from(&[json!({"role":"user","content":"x"})]),
            None
        );
    }

    #[tokio::test]
    async fn assembles_text_and_function_calls() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let sse = concat!(
			"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n",
			"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"}]}}]}\n\n",
			"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"bash\",\"args\":{\"command\":\"ls\"}}}]},\"finishReason\":\"TOOL_USE\"}],\"usageMetadata\":{\"promptTokenCount\":25,\"cachedContentTokenCount\":5,\"candidatesTokenCount\":9,\"totalTokenCount\":34}}\n\n",
		);
        let base = mock::start_mock(mock::sse_response(sse), tx).await;
        let d = GoogleDriver::new(base, "k".into(), "gemini-2.0-flash".into());
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
                id: "bash_1".into(),
                name: "bash".into(),
                input: json!({"command":"ls"})
            }]
        );
        assert_eq!(turn.usage.input, 20); // 25 - 5 cached
        assert_eq!(turn.usage.output, 9);
        assert_eq!(turn.usage.cache_read, 5);
    }

    #[tokio::test]
    async fn non_2xx_maps_to_error() {
        let body = r#"{"error":{"message":"quota exceeded","code":429}}"#;
        let resp = format!("HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let base = mock::start_mock(resp, tx).await;
        let d = GoogleDriver::new(base, "k".into(), "m".into());
        let err = d
            .complete(
                &mock::test_convo(),
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("429"), "{err}");
        assert!(err.contains("quota exceeded"), "{err}");
    }
}
