use crate::event::{AgentEvent, AgentKind};
use crate::parse::{EventParser, extract_field, tool_summary};
use serde_json::Value;

pub struct OpenCodeParser {
    session_started: bool,
    accumulated_input_tokens: u64,
    accumulated_output_tokens: u64,
    accumulated_cached_tokens: u64,
    accumulated_cost: f64,
    had_error: bool,
    error_message: Option<String>,
    debug: bool,
}

impl OpenCodeParser {
    pub fn new(debug: bool) -> Self {
        Self {
            session_started: false,
            accumulated_input_tokens: 0,
            accumulated_output_tokens: 0,
            accumulated_cached_tokens: 0,
            accumulated_cost: 0.0,
            had_error: false,
            error_message: None,
            debug,
        }
    }
}

fn opencode_tool_summary(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "skill" => extract_field(input, "id", tool_name),
        "read" => extract_field(input, "path", tool_name),
        "shell" => tool_summary("Bash", input),
        "subagent" => tool_summary("Agent", input),
        _ => tool_summary(tool_name, input),
    }
}

impl EventParser for OpenCodeParser {
    fn parse(&mut self, line: &str) -> Result<Vec<AgentEvent>, String> {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
        let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match msg_type {
            "step_start" => {
                if self.session_started {
                    return Ok(vec![]);
                }
                self.session_started = true;
                let session_id = v
                    .get("part")
                    .and_then(|p| p.get("sessionID"))
                    .and_then(|s| s.as_str())
                    .or_else(|| v.get("sessionID").and_then(|s| s.as_str()))
                    .unwrap_or("")
                    .to_string();
                Ok(vec![AgentEvent::SessionStart {
                    session_id,
                    agent: AgentKind::OpenCode,
                    model: None,
                }])
            }
            "text" => {
                let text = v
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(vec![AgentEvent::TextComplete(text)])
            }
            "reasoning" => {
                let text = v
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(vec![
                    AgentEvent::ThinkingStart,
                    AgentEvent::ThinkingDelta(text),
                    AgentEvent::ThinkingEnd,
                ])
            }
            "tool_use" => {
                let part = v.get("part").unwrap_or(&Value::Null);
                if let Some(state) = part.get("state").filter(|s| s.is_object()) {
                    let status = state.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    let is_error = match status {
                        "completed" => false,
                        "error" => true,
                        other => {
                            if self.debug {
                                eprintln!("debug: unknown opencode tool_use state: {}", other);
                            }
                            return Ok(vec![]);
                        }
                    };
                    let tool_name = part
                        .get("tool")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let id = part
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string);
                    let input = state.get("input").unwrap_or(&Value::Null);
                    let summary = opencode_tool_summary(&tool_name, input);
                    let content = state
                        .get(if is_error { "error" } else { "output" })
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    // The CLI emits only the finished tool, so adapt it to the renderer's lifecycle.
                    return Ok(vec![
                        AgentEvent::ToolStart {
                            tool_name: tool_name.clone(),
                            id: id.clone(),
                        },
                        AgentEvent::ToolReady {
                            tool_name,
                            input_summary: summary,
                            id: id.clone(),
                        },
                        AgentEvent::ToolResult {
                            is_error,
                            content,
                            id,
                        },
                    ]);
                }
                let state = part.get("state").and_then(|s| s.as_str()).unwrap_or("");
                match state {
                    "running" => {
                        let tool_name = part
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = part.get("input").unwrap_or(&Value::Null);
                        let summary = tool_summary(&tool_name, input);
                        Ok(vec![
                            AgentEvent::ToolStart {
                                tool_name: tool_name.clone(),
                                id: None,
                            },
                            AgentEvent::ToolReady {
                                tool_name,
                                input_summary: summary,
                                id: None,
                            },
                        ])
                    }
                    "completed" => {
                        let output = part
                            .get("output")
                            .and_then(|o| o.as_str())
                            .unwrap_or("")
                            .to_string();
                        Ok(vec![AgentEvent::ToolResult {
                            is_error: false,
                            content: output,
                            id: None,
                        }])
                    }
                    "error" => {
                        let output = part
                            .get("output")
                            .and_then(|o| o.as_str())
                            .unwrap_or("")
                            .to_string();
                        Ok(vec![AgentEvent::ToolResult {
                            is_error: true,
                            content: output,
                            id: None,
                        }])
                    }
                    other => {
                        if self.debug {
                            eprintln!("debug: unknown opencode tool_use state: {}", other);
                        }
                        Ok(vec![])
                    }
                }
            }
            "step_finish" => {
                if let Some(part) = v.get("part") {
                    if let Some(tokens) = part.get("tokens") {
                        self.accumulated_input_tokens +=
                            tokens.get("input").and_then(|t| t.as_u64()).unwrap_or(0);
                        self.accumulated_output_tokens +=
                            tokens.get("output").and_then(|t| t.as_u64()).unwrap_or(0);
                        self.accumulated_cached_tokens += tokens
                            .get("cache")
                            .and_then(|c| c.get("read"))
                            .and_then(|t| t.as_u64())
                            .unwrap_or(0);
                    }
                    if let Some(cost) = part.get("cost").and_then(|c| c.as_f64()) {
                        self.accumulated_cost += cost;
                    }
                }
                Ok(vec![])
            }
            "error" => {
                self.had_error = true;
                self.error_message = v
                    .get("error")
                    .and_then(|e| e.get("data"))
                    .and_then(|d| d.get("message"))
                    .and_then(|m| m.as_str())
                    .or_else(|| {
                        v.get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                    })
                    .map(|s| s.to_string());
                Ok(vec![])
            }
            "message.part.updated" | "session.status" | "session.error" => Ok(vec![]),
            other => {
                if self.debug {
                    eprintln!("debug: unknown opencode event type: {}", other);
                }
                Ok(vec![])
            }
        }
    }

    fn finish(&mut self) -> Vec<AgentEvent> {
        let input = if self.accumulated_input_tokens > 0 {
            Some(self.accumulated_input_tokens)
        } else {
            None
        };
        let output = if self.accumulated_output_tokens > 0 {
            Some(self.accumulated_output_tokens)
        } else {
            None
        };
        let cached = if self.accumulated_cached_tokens > 0 {
            Some(self.accumulated_cached_tokens)
        } else {
            None
        };
        let cost = if self.accumulated_cost > 0.0 {
            Some(self.accumulated_cost)
        } else {
            None
        };

        vec![AgentEvent::SessionEnd {
            success: !self.had_error,
            error_type: if self.had_error {
                Some("error".to_string())
            } else {
                None
            },
            error_message: self.error_message.take(),
            num_turns: None,
            duration_ms: None,
            api_duration_ms: None,
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::EventParser;

    fn parse(parser: &mut OpenCodeParser, line: &str) -> Vec<AgentEvent> {
        parser.parse(line).unwrap()
    }

    #[test]
    fn step_start_first_emits_session_start() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"step_start","timestamp":1700000000,"sessionID":"ses_abc","part":{"sessionID":"ses_abc"}}"#,
        );
        assert_eq!(
            events,
            vec![AgentEvent::SessionStart {
                session_id: "ses_abc".into(),
                agent: AgentKind::OpenCode,
                model: None,
            }]
        );
    }

    #[test]
    fn step_start_second_is_ignored() {
        let mut p = OpenCodeParser::new(false);
        parse(
            &mut p,
            r#"{"type":"step_start","timestamp":1700000000,"sessionID":"ses_abc","part":{"sessionID":"ses_abc"}}"#,
        );
        let events = parse(
            &mut p,
            r#"{"type":"step_start","timestamp":1700000001,"sessionID":"ses_abc","part":{"sessionID":"ses_abc"}}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn text_event() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"text","timestamp":1700000000,"part":{"text":"Hello world"}}"#,
        );
        assert_eq!(events, vec![AgentEvent::TextComplete("Hello world".into())]);
    }

    #[test]
    fn reasoning_event() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"reasoning","timestamp":1700000000,"part":{"text":"Let me think..."}}"#,
        );
        assert_eq!(
            events,
            vec![
                AgentEvent::ThinkingStart,
                AgentEvent::ThinkingDelta("Let me think...".into()),
                AgentEvent::ThinkingEnd,
            ]
        );
    }

    #[test]
    fn tool_use_running() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"tool_use","timestamp":1700000000,"part":{"name":"Bash","state":"running","input":{"command":"ls -la"}}}"#,
        );
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            AgentEvent::ToolStart {
                tool_name: "Bash".into(),
                id: None,
            }
        );
        match &events[1] {
            AgentEvent::ToolReady {
                tool_name,
                input_summary,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert!(input_summary.contains("ls -la"));
            }
            other => panic!("expected ToolReady, got {:?}", other),
        }
    }

    #[test]
    fn tool_use_completed() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"tool_use","timestamp":1700000000,"part":{"name":"Bash","state":"completed","output":"file1.txt\nfile2.txt"}}"#,
        );
        assert_eq!(
            events,
            vec![AgentEvent::ToolResult {
                is_error: false,
                content: "file1.txt\nfile2.txt".into(),
                id: None,
            }]
        );
    }

    #[test]
    fn tool_use_error() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"tool_use","timestamp":1700000000,"part":{"name":"Bash","state":"error","output":"command not found"}}"#,
        );
        assert_eq!(
            events,
            vec![AgentEvent::ToolResult {
                is_error: true,
                content: "command not found".into(),
                id: None,
            }]
        );
    }

    #[test]
    fn step_finish_accumulates_tokens() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"step_finish","timestamp":1700000000,"part":{"tokens":{"input":1000,"output":200,"cache":{"read":500}},"cost":0.05}}"#,
        );
        assert!(events.is_empty());
        assert_eq!(p.accumulated_input_tokens, 1000);
        assert_eq!(p.accumulated_output_tokens, 200);
        assert_eq!(p.accumulated_cached_tokens, 500);
        assert!((p.accumulated_cost - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn step_finish_multi_accumulation() {
        let mut p = OpenCodeParser::new(false);
        parse(
            &mut p,
            r#"{"type":"step_finish","timestamp":1700000000,"part":{"tokens":{"input":1000,"output":100,"cache":{"read":500}},"cost":0.03}}"#,
        );
        parse(
            &mut p,
            r#"{"type":"step_finish","timestamp":1700000001,"part":{"tokens":{"input":2000,"output":200,"cache":{"read":1500}},"cost":0.07}}"#,
        );
        let events = p.finish();
        assert_eq!(
            events,
            vec![AgentEvent::SessionEnd {
                success: true,
                error_type: None,
                error_message: None,
                num_turns: None,
                duration_ms: None,
                api_duration_ms: None,
                cost_usd: Some(0.1),
                input_tokens: Some(3000),
                output_tokens: Some(300),
                cached_tokens: Some(2000),
            }]
        );
    }

    #[test]
    fn error_event() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"error","timestamp":1700000000,"error":{"data":{"message":"Rate limit exceeded"}}}"#,
        );
        assert!(events.is_empty());
        assert!(p.had_error);
        assert_eq!(p.error_message.as_deref(), Some("Rate limit exceeded"));
    }

    #[test]
    fn finish_success() {
        let mut p = OpenCodeParser::new(false);
        parse(
            &mut p,
            r#"{"type":"step_finish","timestamp":1700000000,"part":{"tokens":{"input":500,"output":100,"cache":{"read":200}},"cost":0.02}}"#,
        );
        let events = p.finish();
        assert_eq!(
            events,
            vec![AgentEvent::SessionEnd {
                success: true,
                error_type: None,
                error_message: None,
                num_turns: None,
                duration_ms: None,
                api_duration_ms: None,
                cost_usd: Some(0.02),
                input_tokens: Some(500),
                output_tokens: Some(100),
                cached_tokens: Some(200),
            }]
        );
    }

    #[test]
    fn finish_after_error() {
        let mut p = OpenCodeParser::new(false);
        parse(
            &mut p,
            r#"{"type":"error","timestamp":1700000000,"error":{"data":{"message":"Something broke"}}}"#,
        );
        let events = p.finish();
        match &events[0] {
            AgentEvent::SessionEnd {
                success,
                error_type,
                error_message,
                ..
            } => {
                assert!(!success);
                assert_eq!(error_type.as_deref(), Some("error"));
                assert_eq!(error_message.as_deref(), Some("Something broke"));
            }
            other => panic!("expected SessionEnd, got {:?}", other),
        }
    }

    #[test]
    fn unknown_type_returns_empty() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"future_event","timestamp":1700000000,"data":{}}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn invalid_json_returns_error() {
        let mut p = OpenCodeParser::new(false);
        assert!(p.parse("not json").is_err());
    }

    #[test]
    fn nested_completed_tool_preserves_call_id_and_output() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"tool_use","timestamp":1700000000100,"sessionID":"ses_test","part":{"partID":"prt_not_the_call","id":"call_read","tool":"read","state":{"status":"completed","input":{"path":"src/main.rs"},"output":"first\nsecond\n","metadata":{"truncated":false},"time":{"start":1700000000000,"end":1700000000100}}}}"#,
        );
        assert_eq!(
            events,
            vec![
                AgentEvent::ToolStart {
                    tool_name: "read".into(),
                    id: Some("call_read".into()),
                },
                AgentEvent::ToolReady {
                    tool_name: "read".into(),
                    input_summary: "src/main.rs".into(),
                    id: Some("call_read".into()),
                },
                AgentEvent::ToolResult {
                    is_error: false,
                    content: "first\nsecond\n".into(),
                    id: Some("call_read".into()),
                },
            ]
        );
    }

    #[test]
    fn nested_failed_tool_uses_error_not_output() {
        let mut p = OpenCodeParser::new(false);
        let events = parse(
            &mut p,
            r#"{"type":"tool_use","part":{"partID":"prt_other","id":"call_shell","tool":"shell","state":{"status":"error","input":{"command":"missing-command"},"error":"Command unavailable\nDetails","output":"not the error"}}}"#,
        );
        assert_eq!(
            events,
            vec![
                AgentEvent::ToolStart {
                    tool_name: "shell".into(),
                    id: Some("call_shell".into()),
                },
                AgentEvent::ToolReady {
                    tool_name: "shell".into(),
                    input_summary: "missing-command".into(),
                    id: Some("call_shell".into()),
                },
                AgentEvent::ToolResult {
                    is_error: true,
                    content: "Command unavailable\nDetails".into(),
                    id: Some("call_shell".into()),
                },
            ]
        );
    }

    #[test]
    fn nested_tool_missing_fields_keep_existing_defaults() {
        for status in ["completed", "error"] {
            let mut p = OpenCodeParser::new(false);
            let events = parse(
                &mut p,
                &serde_json::json!({
                    "type": "tool_use",
                    "part": {"partID": "prt_not_a_call", "state": {"status": status}}
                })
                .to_string(),
            );
            assert_eq!(
                events,
                vec![
                    AgentEvent::ToolStart {
                        tool_name: "unknown".into(),
                        id: None,
                    },
                    AgentEvent::ToolReady {
                        tool_name: "unknown".into(),
                        input_summary: "null".into(),
                        id: None,
                    },
                    AgentEvent::ToolResult {
                        is_error: status == "error",
                        content: String::new(),
                        id: None,
                    },
                ]
            );
        }
    }

    #[test]
    fn unknown_nested_status_is_ignored() {
        let mut p = OpenCodeParser::new(true);
        for status in [
            serde_json::json!("running"),
            serde_json::json!("future"),
            Value::Null,
        ] {
            assert!(parse(
                &mut p,
                &serde_json::json!({"type":"tool_use","part":{"tool":"shell","state":{"status":status}}}).to_string(),
            ).is_empty());
        }
    }

    #[test]
    fn nested_tool_summaries_use_input_without_renaming_tools() {
        use serde_json::json;
        let cases = [
            (
                "skill",
                json!({"id":"rust-best-practices"}),
                "rust-best-practices".to_string(),
            ),
            ("read", json!({"path":"src/main.rs"}), "src/main.rs".into()),
            (
                "read",
                json!({"path":"skill://rust-best-practices/SKILL.md"}),
                "skill://rust-best-practices/SKILL.md".into(),
            ),
            (
                "shell",
                json!({"command":"echo synthetic"}),
                "echo synthetic".into(),
            ),
            (
                "shell",
                json!({"command":"x".repeat(100)}),
                format!("{}...", "x".repeat(80)),
            ),
            (
                "subagent",
                json!({"description":"Inspect synthetic files","model":"test/model"}),
                "Inspect synthetic files (test/model)".into(),
            ),
            (
                "subagent",
                json!({"description":"Inspect synthetic files"}),
                "Inspect synthetic files".into(),
            ),
            (
                "subagent",
                json!({"description":"x".repeat(80)}),
                format!("{}...", "x".repeat(60)),
            ),
            ("glob", json!({"pattern":"**/*.rs"}), "**/*.rs".into()),
            (
                "grep",
                json!({"pattern":"needle","path":"src"}),
                "\"needle\" in src".into(),
            ),
            (
                "custom",
                json!({"key":"value"}),
                "{\"key\":\"value\"}".into(),
            ),
            ("skill", json!({"id":42}), "{\"id\":42}".into()),
            ("read", json!({"path":42}), "{\"path\":42}".into()),
            ("skill", json!({}), "{}".into()),
            ("read", json!({}), "{}".into()),
            ("shell", json!({}), "{}".into()),
            ("subagent", json!({}), String::new()),
            ("glob", json!({}), "{}".into()),
            ("grep", json!({}), "\"\"".into()),
        ];
        let mut p = OpenCodeParser::new(false);
        for (tool, input, summary) in cases {
            let events = parse(&mut p, &json!({
                "type":"tool_use",
                "part":{"tool":tool,"state":{"status":"completed","input":input,"output":"loaded body must not be the summary"}}
            }).to_string());
            assert_eq!(events.len(), 3, "tool: {tool}");
            assert_eq!(
                events[1],
                AgentEvent::ToolReady {
                    tool_name: tool.into(),
                    input_summary: summary,
                    id: None,
                },
                "tool: {tool}"
            );
        }
    }

    #[test]
    fn full_text_reasoning_and_reconciliation_suffixes_keep_arrival_order() {
        let mut p = OpenCodeParser::new(false);
        let text = "Full assistant text.\n".repeat(512);
        let reasoning = "Full reasoning.\n".repeat(512);
        let mut events = Vec::new();
        for (kind, content) in [
            ("text", text.as_str()),
            ("reasoning", reasoning.as_str()),
            ("text", "text suffix"),
            ("reasoning", "reasoning suffix"),
        ] {
            events.extend(parse(
                &mut p,
                &serde_json::json!({"type":kind,"part":{"text":content}}).to_string(),
            ));
        }
        assert_eq!(
            events,
            vec![
                AgentEvent::TextComplete(text),
                AgentEvent::ThinkingStart,
                AgentEvent::ThinkingDelta(reasoning),
                AgentEvent::ThinkingEnd,
                AgentEvent::TextComplete("text suffix".into()),
                AgentEvent::ThinkingStart,
                AgentEvent::ThinkingDelta("reasoning suffix".into()),
                AgentEvent::ThinkingEnd,
            ]
        );
    }

    #[test]
    fn mixed_fixture_recovers_from_tool_error_and_preserves_statistics() {
        let mut p = OpenCodeParser::new(false);
        let mut events = Vec::new();
        for line in include_str!("../../tests/fixtures/opencode-v2.2.6.ndjson").lines() {
            events.extend(parse(&mut p, line));
        }
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::SessionStart { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. }))
                .count(),
            1
        );
        assert_eq!(
            p.finish(),
            vec![AgentEvent::SessionEnd {
                success: true,
                error_type: None,
                error_message: None,
                num_turns: None,
                duration_ms: None,
                api_duration_ms: None,
                cost_usd: Some(0.1),
                input_tokens: Some(3000),
                output_tokens: Some(300),
                cached_tokens: Some(2000),
            }]
        );
    }

    #[test]
    fn step_start_falls_back_to_envelope_session_id() {
        let mut p = OpenCodeParser::new(false);
        assert_eq!(
            parse(
                &mut p,
                r#"{"type":"step_start","sessionID":"ses_envelope","part":{}}"#
            ),
            vec![AgentEvent::SessionStart {
                session_id: "ses_envelope".into(),
                agent: AgentKind::OpenCode,
                model: None,
            }]
        );
    }

    #[test]
    fn step_start_prefers_part_session_id() {
        let mut p = OpenCodeParser::new(false);
        assert_eq!(
            parse(
                &mut p,
                r#"{"type":"step_start","sessionID":"ses_envelope","part":{"sessionID":"ses_part"}}"#
            ),
            vec![AgentEvent::SessionStart {
                session_id: "ses_part".into(),
                agent: AgentKind::OpenCode,
                model: None,
            }]
        );
    }

    #[test]
    fn provider_error_message_survives_until_eof() {
        let mut p = OpenCodeParser::new(false);
        assert!(parse(&mut p, r#"{"type":"error","error":{"type":"provider.transport","message":"Synthetic provider failure"}}"#).is_empty());
        let events = p.finish();
        assert!(
            matches!(&events[..], [AgentEvent::SessionEnd { success: false, error_message: Some(message), .. }] if message == "Synthetic provider failure")
        );
    }

    #[test]
    fn retry_and_background_wait_are_informational_not_session_errors() {
        let mut p = OpenCodeParser::new(false);
        let records: Vec<_> = include_str!("../../tests/fixtures/opencode-v2.2.6.ndjson")
            .lines()
            .filter(|line| {
                let record: Value = serde_json::from_str(line).unwrap();
                matches!(record["type"].as_str(), Some("retry" | "background_wait"))
            })
            .collect();
        assert_eq!(records.len(), 2);
        for line in records {
            assert!(parse(&mut p, line).is_empty());
        }
        assert!(matches!(
            &p.finish()[..],
            [AgentEvent::SessionEnd {
                success: true,
                error_message: None,
                ..
            }]
        ));
    }
}
