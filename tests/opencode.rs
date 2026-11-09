//! Synthetic CLI records based on OpenCode v2.2.6 (dc2ffcf8bf26f9e669b6aa22b95f50e12be15e06).
//! Shapes follow packages/cli/src/run/noninteractive.ts and its tests, not live captures.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const MIXED: &str = include_str!("fixtures/opencode-v2.2.6.ndjson");
const START: &str = r#"{"type":"step_start","timestamp":1700000000000,"sessionID":"ses_synthetic","part":{"type":"step-start"}}"#;
const TOOL_ERROR: &str = r#"{"type":"tool_use","timestamp":1700000000100,"sessionID":"ses_synthetic","part":{"partID":"prt_failed","id":"call_failed","tool":"read","state":{"status":"error","input":{"path":"missing.rs"},"error":"Synthetic file not found\nERROR_DETAIL_SENTINEL"}}}"#;
const PROVIDER_ERROR: &str = r#"{"type":"error","timestamp":1700000000100,"sessionID":"ses_synthetic","error":{"type":"provider.transport","message":"Synthetic provider failure"}}"#;

fn run(input: &str, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentcat"))
        .args(["--no-color", "--no-emoji"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentcat");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

#[test]
fn mixed_records_render_in_order_and_keep_successful_bodies_hidden() {
    let out = run(MIXED, &["--show-thinking"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut rest = stdout.as_str();
    for expected in [
        "session ses_synthetic (opencode)",
        "Synthetic reasoning begins.\nCheck the complete record, not just its prefix.\nSynthetic reasoning ends.",
        "Inspecting synthetic files.",
        "[tool] skill: rust-best-practices\n  [ok] (2 lines,",
        "[tool] read: src/main.rs\n  [ok] (2 lines,",
        "[tool] glob: **/*.rs",
        "[tool] grep: \"fn main\" in src",
        "[tool] read: missing.rs\n  [err] Synthetic file not found,",
        "Recover using the available file.",
        "Retrying after the recoverable tool error.",
        "[tool] shell: echo synthetic recovery",
        "[tool] subagent: Inspect synthetic files (test/model)",
        "Synthetic final answer.\nAll supplied text is preserved.",
        "[done] Done",
        "$0.1000 — 5,000 in (2,000 cached) / 300 out tokens",
    ] {
        let position = rest
            .find(expected)
            .unwrap_or_else(|| panic!("missing or out of order: {expected:?}\n{stdout}"));
        rest = &rest[position + expected.len()..];
    }
    assert_eq!(stdout.matches("agentcat — session").count(), 1);
    for hidden in [
        "SKILL_BODY_SENTINEL",
        "ERROR_DETAIL_SENTINEL",
        "Synthetic inspection complete.",
        "1: fn main() {}",
        "Synthetic retry transport error",
        "Synthetic background job",
    ] {
        assert!(
            !stdout.contains(hidden),
            "unexpected body {hidden}: {stdout}"
        );
    }
}

#[test]
fn reasoning_is_hidden_without_show_thinking() {
    let out = run(MIXED, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("[think] Thinking..."), "{stdout}");
    assert!(!stdout.contains("Synthetic reasoning"), "{stdout}");
    assert!(!stdout.contains("Check the complete record"), "{stdout}");
    assert!(
        !stdout.contains("Recover using the available file."),
        "{stdout}"
    );
    assert!(stdout.contains("Synthetic final answer."), "{stdout}");
}

#[test]
fn large_skill_body_is_suppressed() {
    let sentinel = "UNIQUE_LARGE_SKILL_RESULT_SENTINEL";
    let output = format!(
        "{}\n{sentinel}\n{}",
        "synthetic skill line\n".repeat(32768),
        "end"
    );
    let tool = serde_json::json!({
        "type":"tool_use", "timestamp":1700000000100_i64, "sessionID":"ses_synthetic",
        "part":{"partID":"prt_skill","id":"call_skill","tool":"skill","state":{
            "status":"completed","input":{"id":"synthetic-skill"},"output":output,
        }}
    });
    let out = run(&format!("{START}\n{tool}\n"), &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("[tool] skill: synthetic-skill"), "{stdout}");
    assert!(stdout.contains("[ok] (32771 lines,"), "{stdout}");
    assert!(!stdout.contains(sentinel), "{stdout}");
    assert!(!stdout.contains("synthetic skill line"), "{stdout}");
}

#[test]
fn recoverable_tool_error_does_not_fail_the_session() {
    let out = run(&format!("{START}\n{TOOL_ERROR}\n"), &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("[err] Synthetic file not found"),
        "{stdout}"
    );
    assert!(stdout.contains("[done] Done"), "{stdout}");
    assert!(!stdout.contains("ERROR_DETAIL_SENTINEL"), "{stdout}");
}

#[test]
fn forced_format_recovers_from_tool_error_without_a_step_start() {
    let out = run(&format!("{TOOL_ERROR}\n"), &["--format", "opencode"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("[tool] read: missing.rs"), "{stdout}");
    assert!(
        stdout.contains("[err] Synthetic file not found"),
        "{stdout}"
    );
    assert!(stdout.contains("[done] Done"), "{stdout}");
}

#[test]
fn provider_error_still_fails_after_tool_recovery() {
    let out = run(&format!("{MIXED}{PROVIDER_ERROR}\n"), &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("[tool] shell: echo synthetic recovery"),
        "{stdout}"
    );
    assert!(stdout.contains("Synthetic final answer."), "{stdout}");
    assert!(
        stdout.contains("[err] Error: error — Synthetic provider failure"),
        "{stdout}"
    );
    assert!(!stdout.contains("[done] Done"), "{stdout}");
}

#[test]
fn forced_format_reports_error_only_stream_at_eof() {
    let out = run(&format!("{PROVIDER_ERROR}\n"), &["--format", "opencode"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("[err] Error: error — Synthetic provider failure"),
        "{stdout}"
    );
}

#[test]
fn auto_detected_provider_error_fails_the_session() {
    let out = run(&format!("{START}\n{PROVIDER_ERROR}\n"), &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("session ses_synthetic (opencode)"),
        "{stdout}"
    );
    assert!(stdout.contains("Synthetic provider failure"), "{stdout}");
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn tool_header_and_result_are_flushed_before_stdin_closes() {
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_agentcat"))
            .args(["--no-color", "--no-emoji"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn agentcat"),
    );
    let mut stdin = child.0.stdin.take().expect("stdin");
    let stdout = child.0.stdout.take().expect("stdout");
    let (send, receive) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    for line in MIXED.lines().take(4) {
        writeln!(stdin, "{line}").expect("write record");
    }
    stdin.flush().expect("flush stdin");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    loop {
        let line = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|e| {
                panic!("tool output not flushed while stdin is open: {e}\n{output}")
            })
            .expect("read stdout");
        output.push_str(&line);
        output.push('\n');
        if output.contains("[tool] skill: rust-best-practices\n  [ok] (2 lines,") {
            break;
        }
    }
    assert!(child.0.try_wait().expect("check child").is_none());
    assert!(!output.contains("[done]"), "{output}");
    // Terminate even on assertion/timeout failure, without relying on EOF or a startup sleep.
    drop(child);
    drop(stdin);
    reader.join().expect("join stdout reader");
}
