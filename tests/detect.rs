//! Format detection over the head of the stream.

use std::io::Write;
use std::process::{Command, Stdio};

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentcat"))
        .arg("--no-color")
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
    let out = child.wait_with_output().expect("wait");
    Output {
        code: out.status.code().expect("exit code"),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-1","model":"claude-opus-5"}"#;
const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"session_id":"sess-1"}"#;
const ASSISTANT: &str = r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"hello"}]},"session_id":"sess-1"}"#;

#[test]
fn detects_claude_from_first_line() {
    let out = run(&format!("{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("sess-1"), "stdout: {}", out.stdout);
    assert!(out.stdout.contains("hello"), "stdout: {}", out.stdout);
}

#[test]
fn detects_claude_behind_leading_rate_limit_event() {
    let out = run(&format!("{RATE_LIMIT}\n{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("sess-1"), "stdout: {}", out.stdout);
    assert!(out.stdout.contains("hello"), "stdout: {}", out.stdout);
}

#[test]
fn detects_claude_behind_blank_and_unknown_lines() {
    let unknown = r#"{"type":"task_notification","session_id":"sess-1"}"#;
    let out = run(&format!("\n{RATE_LIMIT}\n{unknown}\n{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("hello"), "stdout: {}", out.stdout);
}

#[test]
fn rejects_stream_that_never_identifies_itself() {
    let out = run(&format!("{RATE_LIMIT}\n{RATE_LIMIT}\n"));
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("unrecognized stream format"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn rejects_empty_stdin() {
    let out = run("");
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("empty stdin"), "stderr: {}", out.stderr);
}

#[test]
fn forced_format_skips_detection() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentcat"))
        .args(["--no-color", "--format", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentcat");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{RATE_LIMIT}\n{ASSISTANT}\n").as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
}

// --- detection buffer bounds ---

#[test]
fn detects_header_at_the_line_cap() {
    // 31 noise lines + header on line 32 — the last position still inspected.
    let noise = format!("{RATE_LIMIT}\n").repeat(31);
    let out = run(&format!("{noise}{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("hello"), "stdout: {}", out.stdout);
}

#[test]
fn gives_up_past_the_line_cap() {
    // Header on line 33 — one past the cap, so detection must have stopped.
    let noise = format!("{RATE_LIMIT}\n").repeat(32);
    let out = run(&format!("{noise}{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("unrecognized stream format"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn gives_up_past_the_byte_cap() {
    // Two lines, together over 1 MiB, then a valid header. The line cap is not
    // reached; the byte cap must stop us.
    let fat = format!(
        r#"{{"type":"noise","pad":"{}"}}"#,
        "x".repeat(600 * 1024)
    );
    let out = run(&format!("{fat}\n{fat}\n{INIT}\n{ASSISTANT}\n"));
    assert_eq!(out.code, 2);
    assert!(
        out.stderr.contains("unrecognized stream format"),
        "stderr: {}",
        out.stderr
    );
}
