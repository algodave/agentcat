use crate::event::AgentEvent;
use crate::style::Style;
use std::io::{self, Write};
use std::time::Instant;

struct ToolSlot {
    id: Option<String>,
    start_time: Instant,
    spinner_frame: usize,
    completed: bool,
}

pub struct Renderer {
    style: Style,
    show_thinking: bool,
    thinking_start: Option<Instant>,
    needs_newline_before_tool: bool,
    tool_slots: Vec<ToolSlot>,
    is_tty: bool,
    /// True when the cursor is on the last spinner line (no trailing \n).
    spinner_line_active: bool,
    /// Extra lines inserted (e.g. warnings) while tool_slots are active,
    /// which offset the cursor from the expected position.
    extra_lines: usize,
}

impl Renderer {
    pub fn new(show_thinking: bool, use_emoji: bool, use_color: bool, is_tty: bool) -> Self {
        Self {
            style: Style::new(use_emoji, use_color),
            show_thinking,
            thinking_start: None,
            needs_newline_before_tool: false,
            tool_slots: Vec::new(),
            is_tty,
            spinner_line_active: false,
            extra_lines: 0,
        }
    }

    pub fn spinner_active(&self) -> bool {
        self.is_tty && self.tool_slots.iter().any(|s| !s.completed)
    }

    pub fn tick_spinner(&mut self) -> io::Result<()> {
        if !self.is_tty {
            return Ok(());
        }
        let n = self.tool_slots.len();
        if n == 0 {
            return Ok(());
        }
        let stdout = io::stdout();
        let mut w = stdout.lock();
        for (i, slot) in self.tool_slots.iter_mut().enumerate() {
            if slot.completed {
                continue;
            }
            let elapsed = slot.start_time.elapsed().as_secs_f64();
            let frame = self.style.spinner_frame(slot.spinner_frame);
            slot.spinner_frame += 1;
            let line = format!("  {} running... ({:.1}s)", frame, elapsed);

            if self.spinner_line_active && i == n - 1 {
                // Last spinner is on cursor line (no \n) — use \r to overwrite
                write!(w, "\r\x1b[K")?;
                self.style.write_dim(&mut w, &line)?;
            } else {
                // Earlier spinner — use cursor-up
                let offset = if self.spinner_line_active {
                    (n - 1 - i) * 2 + self.extra_lines
                } else {
                    (n - i) * 2 - 1 + self.extra_lines
                };
                write!(w, "\x1b[{}A\x1b[2K\r", offset)?;
                self.style.write_dim(&mut w, &line)?;
                write!(w, "\x1b[{}B\r", offset)?;
            }
        }
        w.flush()?;
        Ok(())
    }

    pub fn render(&mut self, event: AgentEvent) -> io::Result<()> {
        let stdout = io::stdout();
        let mut w = stdout.lock();

        // Finalize active spinner line (add \n) before any non-result output
        if self.spinner_line_active && !matches!(event, AgentEvent::ToolResult { .. }) {
            write!(w, "\n")?;
            self.spinner_line_active = false;
        }

        // Defensive: unexpected events while tools are active → clear tool state
        // Allow tool events and lightweight status events through
        if !self.tool_slots.is_empty()
            && !matches!(
                event,
                AgentEvent::ToolReady { .. }
                    | AgentEvent::ToolStart { .. }
                    | AgentEvent::ToolResult { .. }
                    | AgentEvent::Warning(_)
                    | AgentEvent::Compaction
            )
        {
            self.tool_slots.clear();
            self.extra_lines = 0;
        }

        match event {
            AgentEvent::SessionStart {
                session_id,
                agent,
                model,
            } => {
                let id_display = if session_id.len() > 16 {
                    format!("{}...", &session_id[..16])
                } else {
                    session_id
                };
                let model_str = model
                    .map(|m| format!(", {}", m))
                    .unwrap_or_default();
                let line = format!(
                    "{} agentcat \u{2014} session {} ({}{})\n",
                    self.style.icon_session(),
                    id_display,
                    agent,
                    model_str,
                );
                self.style.write_bold_cyan(&mut w, &line)?;
            }
            AgentEvent::ModelDetected(model) => {
                let line = format!(
                    "{} Model: {}\n",
                    self.style.icon_session(),
                    model,
                );
                self.style.write_dim(&mut w, &line)?;
            }
            AgentEvent::TextDelta(text) => {
                self.needs_newline_before_tool = true;
                write!(w, "{}", text)?;
            }
            AgentEvent::TextComplete(text) => {
                self.needs_newline_before_tool = false;
                writeln!(w, "{}", text)?;
            }
            AgentEvent::ThinkingStart => {
                self.ensure_newline(&mut w)?;
                self.thinking_start = Some(Instant::now());
                if self.show_thinking {
                    let prefix = format!("{} ", self.style.icon_think());
                    self.style.write_dim(&mut w, &prefix)?;
                }
            }
            AgentEvent::ThinkingDelta(text) => {
                if self.show_thinking {
                    self.style.write_dim(&mut w, &text)?;
                }
            }
            AgentEvent::ThinkingEnd => {
                let duration = self
                    .thinking_start
                    .take()
                    .map(|start| start.elapsed().as_secs_f64());
                if self.show_thinking {
                    let dur_str = duration
                        .map(|d| format!("\n{} ({:.1}s)\n", self.style.icon_think(), d))
                        .unwrap_or_else(|| format!("\n{}\n", self.style.icon_think()));
                    self.style.write_dim(&mut w, &dur_str)?;
                } else {
                    let dur_str = duration
                        .map(|d| format!("{} Thinking... ({:.1}s)\n", self.style.icon_think(), d))
                        .unwrap_or_else(|| {
                            format!("{} Thinking...\n", self.style.icon_think())
                        });
                    self.style.write_dim(&mut w, &dur_str)?;
                }
            }
            AgentEvent::ToolStart { .. } => {
                self.ensure_newline(&mut w)?;
            }
            AgentEvent::ToolReady {
                tool_name,
                input_summary,
                id,
            } => {
                // Write tool header
                let line = format!(
                    "{} {}: {}\n",
                    self.style.icon_tool(),
                    tool_name,
                    input_summary
                );
                self.style.write_cyan(&mut w, &line)?;

                if self.is_tty {
                    // Write initial spinner (no \n — will be overwritten by result via \r)
                    let frame = self.style.spinner_frame(0);
                    let spinner_line = format!("  {} running... (0.0s)", frame);
                    self.style.write_dim(&mut w, &spinner_line)?;
                    self.spinner_line_active = true;
                }

                self.tool_slots.push(ToolSlot {
                    id,
                    start_time: Instant::now(),
                    spinner_frame: 1,
                    completed: false,
                });
            }
            AgentEvent::ToolResult { is_error, content, id } => {
                // ID-based matching with FIFO fallback
                let slot_idx = match &id {
                    Some(tid) => self.tool_slots.iter().position(|s| s.id.as_deref() == Some(tid))
                        .or_else(|| self.tool_slots.iter().position(|s| !s.completed)),
                    None => self.tool_slots.iter().position(|s| !s.completed),
                };

                let elapsed = slot_idx.map(|i| self.tool_slots[i].start_time.elapsed().as_secs_f64());
                let elapsed_str = elapsed
                    .map(|d| format!(", {:.1}s", d))
                    .unwrap_or_default();

                let result_line = if is_error {
                    let first_line = content.lines().next().unwrap_or(&content);
                    format!("  {} {}{}", self.style.icon_err(), first_line, elapsed_str)
                } else {
                    let line_count = if content.is_empty() {
                        0
                    } else {
                        content.lines().count()
                    };
                    if line_count > 0 {
                        format!(
                            "  {} ({} line{}{})",
                            self.style.icon_ok(),
                            line_count,
                            if line_count == 1 { "" } else { "s" },
                            elapsed_str,
                        )
                    } else {
                        format!("  {}{}", self.style.icon_ok(), elapsed_str)
                    }
                };

                if self.is_tty {
                    if let Some(i) = slot_idx {
                        let n = self.tool_slots.len();
                        if self.spinner_line_active && i == n - 1 {
                            // Last spinner is on cursor line — overwrite with \r
                            write!(w, "\r\x1b[K")?;
                            if is_error {
                                self.style.write_red(&mut w, &result_line)?;
                            } else {
                                self.style.write_green(&mut w, &result_line)?;
                            }
                            write!(w, "\n")?;
                            self.spinner_line_active = false;
                        } else {
                            // Earlier spinner — use cursor-up
                            let offset = if self.spinner_line_active {
                                (n - 1 - i) * 2 + self.extra_lines
                            } else {
                                (n - i) * 2 - 1 + self.extra_lines
                            };
                            write!(w, "\x1b[{}A\x1b[2K\r", offset)?;
                            if is_error {
                                self.style.write_red(&mut w, &result_line)?;
                            } else {
                                self.style.write_green(&mut w, &result_line)?;
                            }
                            write!(w, "\x1b[{}B\r", offset)?;
                        }
                        self.tool_slots[i].completed = true;
                    }
                } else {
                    // Non-TTY: write result at current position
                    if let Some(i) = slot_idx {
                        self.tool_slots[i].completed = true;
                    }
                    if is_error {
                        self.style.write_red(&mut w, &format!("{}\n", result_line))?;
                    } else {
                        self.style.write_green(&mut w, &format!("{}\n", result_line))?;
                    }
                }

                // If all completed, clear slots
                if self.tool_slots.iter().all(|s| s.completed) {
                    self.tool_slots.clear();
                    self.spinner_line_active = false;
                    self.extra_lines = 0;
                }
            }
            AgentEvent::Compaction => {
                self.ensure_newline(&mut w)?;
                let msg = format!("{} Context compacted\n", self.style.icon_compact());
                self.style.write_dim(&mut w, &msg)?;
                if !self.tool_slots.is_empty() {
                    self.extra_lines += 1;
                }
            }
            AgentEvent::UserMessage(message) => {
                self.ensure_newline(&mut w)?;
                let msg = format!("{} {}\n", self.style.icon_user(), message);
                self.style.write_magenta(&mut w, &msg)?;
            }
            AgentEvent::SubAgentMessage(message) => {
                self.ensure_newline(&mut w)?;
                let msg = format!("{} {}\n", self.style.icon_subagent(), message);
                self.style.write_magenta(&mut w, &msg)?;
            }
            AgentEvent::Warning(message) => {
                self.ensure_newline(&mut w)?;
                let msg = format!("{} {}\n", self.style.icon_warn(), message);
                self.style.write_yellow(&mut w, &msg)?;
                if !self.tool_slots.is_empty() {
                    self.extra_lines += 1;
                }
            }
            AgentEvent::SessionEnd {
                success,
                error_type,
                error_message,
                num_turns,
                duration_ms,
                api_duration_ms,
                cost_usd,
                input_tokens,
                output_tokens,
                cached_tokens,
            } => {
                self.ensure_newline(&mut w)?;
                // Separator
                let sep = "\u{2501}".repeat(34);
                self.style.write_dim(&mut w, &format!("{}\n", sep))?;

                // Done/Error line
                if success {
                    let mut parts = vec![];
                    if let Some(turns) = num_turns {
                        parts.push(format!(
                            "{} turn{}",
                            turns,
                            if turns == 1 { "" } else { "s" }
                        ));
                    }
                    if let Some(dur) = duration_ms {
                        let secs = dur as f64 / 1000.0;
                        if let Some(api_dur) = api_duration_ms {
                            let api_secs = api_dur as f64 / 1000.0;
                            parts.push(format!("{:.1}s ({:.1}s API)", secs, api_secs));
                        } else {
                            parts.push(format!("{:.1}s", secs));
                        }
                    }
                    let detail = if parts.is_empty() {
                        String::new()
                    } else {
                        format!(" \u{2014} {}", parts.join(", "))
                    };
                    let line = format!("{} Done{}\n", self.style.icon_done(), detail);
                    self.style.write_bold(&mut w, &line)?;
                } else {
                    let err_type = error_type.as_deref().unwrap_or("error");
                    let err_msg = error_message.as_deref().unwrap_or("");
                    let line = if err_msg.is_empty() {
                        format!("{} Error: {}\n", self.style.icon_err(), err_type)
                    } else {
                        format!(
                            "{} Error: {} \u{2014} {}\n",
                            self.style.icon_err(),
                            err_type,
                            err_msg
                        )
                    };
                    self.style.write_bold(&mut w, &line)?;
                }

                // Cost line (if available)
                if let Some(cost) = cost_usd {
                    if let (Some(inp), Some(out)) = (input_tokens, output_tokens) {
                        let total_in = inp + cached_tokens.unwrap_or(0);
                        let cached_str = cached_tokens
                            .filter(|&c| c > 0)
                            .map(|c| format!(" ({} cached)", format_number(c)))
                            .unwrap_or_default();
                        let line = format!(
                            "{} ${:.4} \u{2014} {} in{} / {} out tokens\n",
                            self.style.icon_cost(),
                            cost,
                            format_number(total_in),
                            cached_str,
                            format_number(out),
                        );
                        self.style.write_yellow(&mut w, &line)?;
                    } else {
                        let line = format!(
                            "{} ${:.4}\n",
                            self.style.icon_cost(),
                            cost,
                        );
                        self.style.write_yellow(&mut w, &line)?;
                    }
                } else if input_tokens.is_some() || output_tokens.is_some() {
                    // Stats line without cost
                    let inp = input_tokens.unwrap_or(0);
                    let out = output_tokens.unwrap_or(0);
                    let cached_str = cached_tokens
                        .filter(|&c| c > 0)
                        .map(|c| format!(" ({} cached)", format_number(c)))
                        .unwrap_or_default();
                    let line = format!(
                        "{} {} in / {} out tokens{}\n",
                        self.style.icon_stats(),
                        format_number(inp),
                        format_number(out),
                        cached_str,
                    );
                    self.style.write_yellow(&mut w, &line)?;
                }
            }
        }
        w.flush()?;
        Ok(())
    }

    fn ensure_newline(&mut self, w: &mut impl Write) -> io::Result<()> {
        if self.needs_newline_before_tool {
            writeln!(w)?;
            self.needs_newline_before_tool = false;
        }
        Ok(())
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{},{:03},{:03}", n / 1_000_000, (n % 1_000_000) / 1_000, n % 1_000)
    } else if n >= 1_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        n.to_string()
    }
}
