mod event;
mod parse;
mod render;
mod style;

use clap::Parser;
use crossterm::tty::IsTty;
use parse::{Format, create_parser, detect_format};
use render::Renderer;
use std::process;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time;

#[derive(Parser)]
#[command(name = "agentcat", version, about = "Rich terminal renderer for AI coding agent streams")]
struct Cli {
    /// Show extended thinking blocks (hidden by default)
    #[arg(long)]
    show_thinking: bool,

    /// Disable emoji output
    #[arg(long)]
    no_emoji: bool,

    /// Disable ANSI color output
    #[arg(long)]
    no_color: bool,

    /// Force input format: claude, pi, gemini, codex, opencode (default: auto-detect)
    #[arg(long)]
    format: Option<String>,

    /// Print warnings for unknown event types to stderr
    #[arg(long)]
    debug: bool,
}

/// How far into the stream we keep looking for the line that identifies the
/// format. Real headers arrive within the first few lines; these caps bound
/// the buffer so a stream that never identifies itself can't grow unbounded.
const DETECT_MAX_LINES: usize = 32;
const DETECT_MAX_BYTES: usize = 1 << 20; // 1 MiB

/// Parse one line and render whatever events it yields. A line that fails to
/// parse is skipped (reported only with --debug) rather than killing the stream.
fn handle_line(
    parser: &mut Box<dyn parse::EventParser>,
    renderer: &mut Renderer,
    line: &str,
    debug: bool,
    last_success: &mut bool,
) {
    match parser.parse(line) {
        Ok(events) => {
            for event in events {
                if let event::AgentEvent::SessionEnd { success, .. } = &event {
                    *last_success = *success;
                }
                if let Err(e) = renderer.render(event) {
                    eprintln!("Render error: {}", e);
                    process::exit(2);
                }
            }
        }
        Err(e) => {
            if debug {
                eprintln!("Parse error: {}", e);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let is_tty = std::io::stdout().is_tty();
    let use_color = !cli.no_color
        && std::env::var("NO_COLOR").is_err()
        && is_tty;
    let use_emoji = !cli.no_emoji;

    let forced_format = cli.format.as_deref().map(|f| match f {
        "claude" => Format::Claude,
        "pi" => Format::Pi,
        "gemini" => Format::Gemini,
        "codex" => Format::Codex,
        "opencode" => Format::OpenCode,
        other => {
            eprintln!("Error: unknown format '{}'", other);
            process::exit(2);
        }
    });

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    // Buffer the leading lines until one of them identifies the format. Agents
    // emit out-of-band events before the session header (claude, for instance,
    // can lead with rate_limit_event), so the first line alone isn't reliable.
    // The buffer is capped both ways so an unrecognized stream fails fast
    // instead of being held in memory until EOF.
    let mut leading: Vec<String> = Vec::new();
    let mut leading_bytes = 0usize;
    let mut detected: Option<Format> = None;

    if forced_format.is_none() {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    detected = detect_format(&line).ok();
                    leading_bytes += line.len();
                    leading.push(line);
                    if detected.is_some() {
                        break;
                    }
                    if leading.len() >= DETECT_MAX_LINES || leading_bytes >= DETECT_MAX_BYTES {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Error reading stdin: {}", e);
                    process::exit(2);
                }
            }
        }
    }

    let format = match forced_format.or(detected) {
        Some(f) => f,
        None => {
            if leading.is_empty() {
                eprintln!("Error: empty stdin");
            } else {
                eprintln!("Error: unrecognized stream format");
            }
            process::exit(2);
        }
    };

    let mut parser = create_parser(format, cli.debug);
    let mut renderer = Renderer::new(cli.show_thinking, use_emoji, use_color, is_tty);
    let mut last_success = true;

    // Replay the buffered lines now that we know how to read them
    for line in &leading {
        handle_line(&mut parser, &mut renderer, line, cli.debug, &mut last_success);
    }

    // Process remaining lines with spinner support
    let mut tick = time::interval(time::Duration::from_millis(80));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    // Consume the first immediate tick
    tick.tick().await;

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        handle_line(&mut parser, &mut renderer, &line, cli.debug, &mut last_success);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("Error reading stdin: {}", e);
                        process::exit(2);
                    }
                }
            }
            _ = tick.tick(), if renderer.spinner_active() => {
                if let Err(e) = renderer.tick_spinner() {
                    eprintln!("Render error: {}", e);
                    process::exit(2);
                }
            }
        }
    }

    // Finish (emit any remaining events, e.g. Codex SessionEnd)
    let final_events = parser.finish();
    for event in final_events {
        if let event::AgentEvent::SessionEnd { success, .. } = &event {
            last_success = *success;
        }
        if let Err(e) = renderer.render(event) {
            eprintln!("Render error: {}", e);
            process::exit(2);
        }
    }

    if last_success {
        process::exit(0);
    } else {
        process::exit(1);
    }
}
