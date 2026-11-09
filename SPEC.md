# agentcat — Specification

A CLI tool that reads NDJSON stream output from AI coding agents via stdin and renders it as a rich, human-friendly terminal output with emojis and ANSI colors. Auto-detects the agent format and normalizes events into a common rendering pipeline.

**Supported agents:**
- **Claude Code** — `claude -p ... --output-format stream-json --verbose --include-partial-messages`
- **pi.dev** — `pi --mode json ...`
- **Gemini CLI** — `gemini -o stream-json -p ...`
- **Codex CLI** — `codex exec --json ...`
- **OpenCode** — `opencode run --format json ...`

**Usage:**
```bash
claude -p "prompt" --output-format stream-json --verbose --include-partial-messages | agentcat
pi --mode json "prompt" 2>/dev/null | agentcat
gemini -o stream-json -p "prompt" | agentcat
codex exec --json "prompt" | agentcat
opencode run --format json "prompt" | agentcat
```

**Language:** Async Rust

---

## Table of Contents

1. [CLI Interface](#cli-interface)
2. [Format Detection](#format-detection)
3. [Input Format: Claude Code](#input-format-claude-code)
4. [Input Format: pi.dev](#input-format-pidev)
5. [Input Format: Gemini CLI](#input-format-gemini-cli)
6. [Input Format: Codex CLI](#input-format-codex-cli)
7. [Input Format: OpenCode](#input-format-opencode)
8. [Internal Event Model](#internal-event-model)
9. [Event Mapping Tables](#event-mapping-tables)
10. [Output Rendering](#output-rendering)
11. [Color Scheme](#color-scheme)
12. [Architecture](#architecture)
13. [Project Structure](#project-structure)
14. [Exit Codes](#exit-codes)
15. [Verification](#verification)
16. [Future Extensibility](#future-extensibility)
17. [Sources & References](#sources--references)

---

## CLI Interface

```
agentcat [OPTIONS]

Options:
  --show-thinking        Show extended thinking blocks (hidden by default)
  --no-emoji             Disable emoji output
  --no-color             Disable ANSI color output
  --format <FORMAT>      Force input format: claude, pi, gemini, codex, opencode (default: auto-detect)
  --version              Print version
  --help                 Print help
```

No positional arguments. Input is always stdin. Output is always stdout.

Respect the `NO_COLOR` environment variable (see https://no-color.org/) — when set, behave as if `--no-color` was passed.

---

## Format Detection

Auto-detect based on the first JSON line's `type` field:

| First line `type` value | Detected format |
|--------------------------|-----------------|
| `"system"` | Claude Code |
| `"session"` | pi.dev |
| `"init"` | Gemini CLI |
| `"thread.started"` | Codex CLI |
| `"step_start"` | OpenCode |

If detection fails, exit with code 2 and message to stderr: `"Error: unrecognized stream format"`.

The `--format` flag overrides auto-detection. When set, the first line is still parsed by the specified parser (no detection step).

---

## Input Format: Claude Code

**Transport:** NDJSON (newline-delimited JSON). One JSON object per `\n`-delimited line. Each line has a `type` field.

**Invocation:**
```bash
claude -p "prompt" --output-format stream-json --verbose --include-partial-messages
```

The `--verbose` flag enables full turn-by-turn output. The `--include-partial-messages` flag enables `stream_event` messages for token-by-token streaming. Without it, only complete `assistant`/`user`/`result` messages are emitted.

### Event Types

| `type` | Description |
|--------|-------------|
| `"system"` | Session lifecycle events |
| `"assistant"` | Complete assistant response after each turn |
| `"user"` | Tool results sent back to Claude, or user input |
| `"stream_event"` | Raw API streaming events (only with `--include-partial-messages`) |
| `"result"` | Final message, always last in stream |

### `system` Message

```json
{
  "type": "system",
  "subtype": "init",
  "session_id": "abc-123",
  "data": { "session_id": "abc-123" }
}
```

**Subtypes:**
- `"init"` — First message in the stream. Contains `session_id`.
- `"compact_boundary"` — Emitted when context window was compacted.

### `stream_event` Message

Wraps raw Claude API streaming events. Only emitted when `--include-partial-messages` is passed.

```json
{
  "type": "stream_event",
  "uuid": "evt-1",
  "session_id": "abc-123",
  "event": { ... },
  "parent_tool_use_id": null
}
```

The `event` field contains one of these Claude API event types:

| `event.type` | Description | Key fields |
|-------------|-------------|------------|
| `message_start` | New message begins | `event.message` (with `model`, `role`, `usage`) |
| `content_block_start` | Content block begins | `event.index`, `event.content_block` (`type`: `"text"`, `"tool_use"`, `"thinking"`) |
| `content_block_delta` | Incremental content | `event.index`, `event.delta` (see delta types) |
| `content_block_stop` | Content block complete | `event.index` |
| `message_delta` | Message-level update | `event.delta.stop_reason`, `event.usage` |
| `message_stop` | Message complete | — |
| `ping` | Keep-alive | — |
| `error` | Stream error | `event.error.type`, `event.error.message` |

**Delta types** (within `content_block_delta`):

| `event.delta.type` | Description | Data field |
|--------------------|-------------|------------|
| `text_delta` | Text chunk | `event.delta.text` |
| `input_json_delta` | Tool input JSON chunk | `event.delta.partial_json` |
| `thinking_delta` | Extended thinking chunk | `event.delta.thinking` |
| `signature_delta` | Thinking signature | `event.delta.signature` |

**Example — text streaming:**
```json
{"type":"stream_event","uuid":"evt-1","session_id":"abc-123","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}},"parent_tool_use_id":null}
```

**Example — tool input streaming:**
```json
{"type":"stream_event","uuid":"evt-2","session_id":"abc-123","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"Bash"}},"parent_tool_use_id":null}
{"type":"stream_event","uuid":"evt-3","session_id":"abc-123","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\": \"ls"}},"parent_tool_use_id":null}
{"type":"stream_event","uuid":"evt-4","session_id":"abc-123","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" -la\"}"}},"parent_tool_use_id":null}
{"type":"stream_event","uuid":"evt-5","session_id":"abc-123","event":{"type":"content_block_stop","index":1},"parent_tool_use_id":null}
```

### `assistant` Message

Complete message emitted after each Claude response turn.

```json
{
  "type": "assistant",
  "message": {
    "content": [
      { "type": "text", "text": "Let me check the files." },
      { "type": "tool_use", "id": "toolu_abc", "name": "Bash", "input": { "command": "ls -la" } }
    ],
    "model": "claude-sonnet-4-6"
  },
  "parent_tool_use_id": null,
  "error": null
}
```

**Content block types in `message.content[]`:**

| Block `type` | Fields | Description |
|-------------|--------|-------------|
| `"text"` | `text` | Plain text response |
| `"tool_use"` | `id`, `name`, `input` | Tool call request |
| `"thinking"` | `thinking`, `signature` | Extended thinking block |
| `"tool_result"` | `tool_use_id`, `content`, `is_error` | Tool result (in user messages) |

The `error` field on the assistant message is non-null if the response hit an error: `"rate_limit"`, `"server_error"`, `"authentication_failed"`, `"billing_error"`, `"invalid_request"`, `"unknown"`.

### `user` Message

Tool results sent back to Claude after tool execution.

The `tool_result` blocks may appear at the top level (`content`) or nested under `message.content` — the parser must check both locations.

**Top-level format (with `--include-partial-messages`):**
```json
{
  "type": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_abc",
      "content": "file1.rs\nfile2.rs\nCargo.toml",
      "is_error": false
    }
  ],
  "uuid": "msg-456",
  "parent_tool_use_id": null
}
```

**Nested format (without `--include-partial-messages`, or `-p` mode):**
```json
{
  "type": "user",
  "message": {
    "role": "user",
    "content": [
      {
        "type": "tool_result",
        "tool_use_id": "toolu_abc",
        "content": "hello_world",
        "is_error": false
      }
    ]
  },
  "parent_tool_use_id": null,
  "session_id": "abc-123",
  "uuid": "msg-456"
}
```

The `content` field within each `tool_result` block can be a string or an array of content blocks.

### `result` Message

**Always the last message in the stream.** Always present, even on errors.

```json
{
  "type": "result",
  "subtype": "success",
  "result": "Here is the final text output...",
  "session_id": "abc-123",
  "is_error": false,
  "total_cost_usd": 0.0042,
  "num_turns": 3,
  "duration_ms": 12345,
  "duration_api_ms": 9800,
  "usage": {
    "input_tokens": 1500,
    "output_tokens": 800,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": 500
  },
  "stop_reason": "end_turn"
}
```

**Result subtypes:**

| `subtype` | Meaning | `result` field present? |
|-----------|---------|------------------------|
| `"success"` | Completed normally | Yes |
| `"error_max_turns"` | Hit `--max-turns` limit | No |
| `"error_max_budget_usd"` | Hit `--max-budget-usd` limit | No |
| `"error_during_execution"` | Error interrupted the loop | No |
| `"error_max_structured_output_retries"` | Structured output validation failed | No |

### Typical Event Sequence

**With `--include-partial-messages` (streaming):**
```
system (init)
stream_event (message_start)
stream_event (content_block_start, text)
stream_event (content_block_delta, text_delta) × N
stream_event (content_block_stop)
stream_event (content_block_start, tool_use)
stream_event (content_block_delta, input_json_delta) × N
stream_event (content_block_stop)
stream_event (message_delta)
stream_event (message_stop)
assistant (complete turn)
user (tool results)
... more turns ...
assistant (final text-only turn)
result
```

**Without `--include-partial-messages` (no streaming):**
```
system (init)
assistant (complete turn with tool calls)
user (tool results)
assistant (next turn)
user (tool results)
assistant (final turn)
result
```

---

## Input Format: pi.dev

**Transport:** NDJSON. Strict `\n` (LF) as record delimiter — do NOT split on `U+2028` or `U+2029`.

**Invocation:**
```bash
pi --mode json "prompt" 2>/dev/null
```

pi.dev emits some info to stderr, so redirect stderr if piping.

### Session Header (always first line)

```json
{"type": "session", "version": 3, "id": "550e8400-e29b-41d4-a716-446655440000", "timestamp": "2025-11-30T12:00:00.000Z", "cwd": "/home/user/project"}
```

### Event Types

| `type` | Description |
|--------|-------------|
| `session` | Session header (first line) |
| `agent_start` | Agent session begins |
| `agent_end` | Agent session completes. Fields: `messages` (all new messages) |
| `turn_start` | One LLM turn begins |
| `turn_end` | Turn completes. Fields: `message`, `toolResults` |
| `message_start` | New message begins |
| `message_update` | Streaming content via `assistantMessageEvent` sub-events |
| `message_end` | Message finalized |
| `tool_execution_start` | Tool invoked. Fields: `toolCallId`, `toolName`, `arguments` |
| `tool_execution_update` | Partial results during execution |
| `tool_execution_end` | Tool completes. Fields: `isError`, result data |
| `auto_compaction_start` | Context compaction begins |
| `auto_compaction_end` | Context compaction ends |
| `auto_retry_start` | Error recovery begins |
| `auto_retry_end` | Error recovery ends |
| `extension_error` | Extension failure |

### `message_update` Sub-Events

The `message_update` event contains an `assistantMessageEvent` field with its own `type` discriminator:

| Sub-event `type` | Description | Key fields |
|------------------|-------------|------------|
| `start` | Stream begins | `partial` (AssistantMessage) |
| `text_start` | Text block begins | `contentIndex` |
| `text_delta` | Text chunk | `delta` (string), `contentIndex` |
| `text_end` | Text block complete | `content` (string), `contentIndex` |
| `thinking_start` | Thinking begins | `contentIndex` |
| `thinking_delta` | Thinking chunk | `delta` (string), `contentIndex` |
| `thinking_end` | Thinking complete | `content` (string), `contentIndex` |
| `toolcall_start` | Tool call begins | `contentIndex` |
| `toolcall_delta` | Tool arg JSON streams | `delta` (string), partial `arguments` |
| `toolcall_end` | Tool call complete | `toolCall` (ToolCall object with `name`, `arguments`), `contentIndex` |
| `done` | Stream finished | `reason` (`"stop"` / `"length"` / `"toolUse"`), `message` (AssistantMessage with `usage`) |
| `error` | Stream failed | `reason` (`"error"` / `"aborted"`), `error` (AssistantMessage) |

**Example — text streaming:**
```json
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Hello","contentIndex":0}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":" world","contentIndex":0}}
{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"Hello world","contentIndex":0}}
```

**Example — tool call:**
```json
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":1}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","delta":"{\"command\":\"ls","contentIndex":1}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_end","toolCall":{"name":"bash","arguments":{"command":"ls -la"}},"contentIndex":1}}
```

**Example — tool execution:**
```json
{"type":"tool_execution_start","toolCallId":"call-123","toolName":"bash","arguments":{"command":"ls -la"}}
{"type":"tool_execution_end","toolCallId":"call-123","isError":false}
```

**Example — done with usage:**
```json
{"type":"message_update","assistantMessageEvent":{"type":"done","reason":"stop","message":{"role":"assistant","content":[...],"provider":"anthropic","model":"claude-sonnet-4-6","usage":{"inputTokens":1500,"outputTokens":800},"stopReason":"end_turn"}}}
```

### AssistantMessage Structure (in `done` event)

```json
{
  "role": "assistant",
  "content": [ ... ],
  "provider": "anthropic",
  "model": "claude-sonnet-4-6",
  "usage": {
    "inputTokens": 1500,
    "outputTokens": 800
  },
  "stopReason": "end_turn"
}
```

Note: pi.dev uses camelCase for usage fields (`inputTokens`, `outputTokens`) unlike Claude Code's snake_case (`input_tokens`, `output_tokens`).

### Typical Event Sequence

```
session
agent_start
turn_start
message_start
message_update (start)
message_update (text_start)
message_update (text_delta) × N
message_update (text_end)
message_update (toolcall_start)
message_update (toolcall_delta) × N
message_update (toolcall_end)
message_update (done, reason: "toolUse")
message_end
tool_execution_start
tool_execution_end
turn_end
turn_start
message_start
message_update (text_start)
message_update (text_delta) × N
message_update (text_end)
message_update (done, reason: "stop")
message_end
turn_end
agent_end
```

---

## Input Format: Gemini CLI

**Transport:** NDJSON. All events have `type` and `timestamp` (ISO 8601) fields.

**Invocation:**
```bash
gemini -o stream-json -p "prompt"
# or
gemini --output-format stream-json --prompt "prompt"
```

### Event Types

| `type` | Description |
|--------|-------------|
| `"init"` | Session start |
| `"message"` | User or assistant text |
| `"tool_use"` | Tool call request |
| `"tool_result"` | Tool execution result |
| `"error"` | Non-fatal warning or error |
| `"result"` | Final event with stats |

### `init` Event

```json
{
  "type": "init",
  "timestamp": "2025-10-10T12:00:00.000Z",
  "session_id": "test-session-123",
  "model": "gemini-2.5-pro"
}
```

### `message` Event

```json
{
  "type": "message",
  "timestamp": "2025-10-10T12:00:01.000Z",
  "role": "assistant",
  "content": "The answer",
  "delta": true
}
```

- `role`: `"user"` or `"assistant"`
- `delta`: `true` for streaming assistant chunks, absent/false for complete messages
- User messages (echoing the prompt) have `role: "user"` — these should be ignored by agentcat

### `tool_use` Event

```json
{
  "type": "tool_use",
  "timestamp": "2025-10-10T12:00:02.000Z",
  "tool_name": "Read",
  "tool_id": "read-123",
  "parameters": { "file_path": "/path/to/file.txt" }
}
```

Parameters are available immediately (not streamed incrementally).

### `tool_result` Event

**Success:**
```json
{
  "type": "tool_result",
  "timestamp": "2025-10-10T12:00:03.000Z",
  "tool_id": "read-123",
  "status": "success",
  "output": "file contents here"
}
```

**Error:**
```json
{
  "type": "tool_result",
  "timestamp": "2025-10-10T12:00:03.000Z",
  "tool_id": "read-123",
  "status": "error",
  "error": {
    "type": "FILE_NOT_FOUND",
    "message": "File not found"
  }
}
```

### `error` Event

```json
{
  "type": "error",
  "timestamp": "2025-10-10T12:00:04.000Z",
  "severity": "warning",
  "message": "Loop detected, stopping execution"
}
```

`severity`: `"warning"` or `"error"`.

### `result` Event

**Success:**
```json
{
  "type": "result",
  "timestamp": "2025-10-10T12:00:05.000Z",
  "status": "success",
  "stats": {
    "total_tokens": 250,
    "input_tokens": 150,
    "output_tokens": 100,
    "cached": 50,
    "input": 100,
    "duration_ms": 3200,
    "tool_calls": 2,
    "models": {
      "gemini-2.5-pro": {
        "total_tokens": 250,
        "input_tokens": 150,
        "output_tokens": 100,
        "cached": 50,
        "input": 100
      }
    }
  }
}
```

**Error:**
```json
{
  "type": "result",
  "timestamp": "2025-10-10T12:00:05.000Z",
  "status": "error",
  "error": {
    "type": "MaxSessionTurnsError",
    "message": "Maximum session turns exceeded"
  },
  "stats": { ... }
}
```

**Notes:**
- No thinking events — Gemini drops `Thought` events in headless mode
- No cost data in the stream
- `stats.cached` = cached input tokens, `stats.input` = non-cached input tokens

### Typical Event Sequence

```
init
message (role: user)
message (role: assistant, delta: true) × N
tool_use
tool_result
message (role: assistant, delta: true) × N
result
```

---

## Input Format: Codex CLI

**Transport:** NDJSON via `codex exec --json`.

**Invocation:**
```bash
codex exec --json "prompt"
```

### Event Types

| `type` | Description |
|--------|-------------|
| `"thread.started"` | Session init |
| `"turn.started"` | Turn begins |
| `"turn.completed"` | Turn done with usage stats |
| `"turn.failed"` | Turn encountered an error |
| `"item.started"` | Work item begins |
| `"item.completed"` | Work item done (authoritative final state) |
| `"error"` | Mid-turn error |

### `thread.started` Event

```json
{
  "type": "thread.started",
  "thread_id": "0199a213-81c0-7800-8aa1-bbab2a035a53"
}
```

### `turn.started` / `turn.completed` / `turn.failed`

```json
{"type": "turn.started"}
```

```json
{
  "type": "turn.completed",
  "usage": {
    "input_tokens": 24763,
    "cached_input_tokens": 24448,
    "output_tokens": 122
  }
}
```

```json
{
  "type": "turn.failed",
  "error": {
    "message": "Context window exceeded",
    "codexErrorInfo": "ContextWindowExceeded"
  }
}
```

**Important:** Multiple `turn.completed` events may occur in a single session (multi-turn agent loop). Accumulate token counts across all turns. Emit `SessionEnd` only at EOF.

### `item.started` / `item.completed`

Both carry an `item` object with a `type` field:

**`agent_message` — Text response:**
```json
{
  "type": "item.completed",
  "item": {
    "id": "item_3",
    "type": "agent_message",
    "text": "Repo contains docs, sdk, and examples directories."
  }
}
```

Note: `codex exec --json` does NOT provide streaming text deltas. Text arrives complete in `item.completed`. Only the app-server protocol (not in scope) has `item/agentMessage/delta` events.

**`command_execution` — Shell command:**
```json
{
  "type": "item.started",
  "item": {
    "id": "item_1",
    "type": "command_execution",
    "command": "bash -lc ls",
    "cwd": "/project",
    "status": "in_progress"
  }
}
```

```json
{
  "type": "item.completed",
  "item": {
    "id": "item_1",
    "type": "command_execution",
    "command": "bash -lc ls",
    "status": "completed",
    "exitCode": 0,
    "durationMs": 150,
    "aggregatedOutput": "docs\nsdk\nexamples\n"
  }
}
```

**`file_change` — File edits:**
```json
{
  "type": "item.completed",
  "item": {
    "id": "item_2",
    "type": "file_change",
    "status": "completed",
    "changes": [
      { "path": "src/main.rs", "kind": "edit", "diff": "..." }
    ]
  }
}
```

**`reasoning` — Model reasoning:**
```json
{
  "type": "item.started",
  "item": {
    "id": "item_4",
    "type": "reasoning",
    "summary": [],
    "content": []
  }
}
```

```json
{
  "type": "item.completed",
  "item": {
    "id": "item_4",
    "type": "reasoning",
    "summary": [
      { "type": "summaryText", "text": "Analyzing the repository structure..." }
    ],
    "content": []
  }
}
```

**`mcp_tool_call` — MCP tool invocation:**
```json
{
  "type": "item.completed",
  "item": {
    "id": "item_5",
    "type": "mcp_tool_call",
    "server": "my-server",
    "tool": "search",
    "status": "completed",
    "arguments": { "query": "hello" },
    "result": "Search results..."
  }
}
```

**`web_search` — Web search:**
```json
{
  "type": "item.completed",
  "item": {
    "id": "item_6",
    "type": "web_search",
    "query": "rust async stdin",
    "action": "search"
  }
}
```

**`context_compaction` — Compaction marker:**
```json
{
  "type": "item.completed",
  "item": {
    "id": "item_7",
    "type": "context_compaction"
  }
}
```

**Item status values:** `"inProgress"`, `"completed"`, `"failed"`, `"declined"`.

### `error` Event

```json
{
  "type": "error",
  "error": {
    "message": "Context window exceeded",
    "codexErrorInfo": "ContextWindowExceeded",
    "additionalDetails": "..."
  }
}
```

Known `codexErrorInfo` values: `"ContextWindowExceeded"`, `"UsageLimitExceeded"`, `"HttpConnectionFailed"`, `"BadRequest"`, `"Unauthorized"`, `"SandboxError"`, `"InternalServerError"`, `"Other"`.

### Typical Event Sequence

```
thread.started
turn.started
item.started (command_execution)
item.completed (command_execution)
item.completed (agent_message)
turn.completed (with usage)
turn.started
item.started (file_change)
item.completed (file_change)
item.completed (agent_message)
turn.completed (with usage)
```

---

## Input Format: OpenCode

**Command:** `opencode run --format json "prompt" | agentcat`

**Reasoning:** `opencode run --format json --thinking "prompt" | agentcat --show-thinking`.
OpenCode must emit reasoning (normally with `--thinking`); `--show-thinking` only
controls display of supplied content.

The V2 CLI field contract below covers **v2.2.6**.

All events have the structure: `{"type":"<type>","timestamp":<ms>,"sessionID":"ses_xxx",...}`

### Event Types

| Event type | Key fields | Description |
|---|---|---|
| `step_start` | `part.sessionID`, falling back to envelope `sessionID` | Start of a new step; first occurrence triggers session start |
| `text` | `part.text` | Full supplied text block or reconciliation suffix |
| `reasoning` | `part.text` | Full supplied reasoning block or reconciliation suffix |
| `tool_use` (`part.state.status="completed"`) | `part.tool`, `part.id`, `part.state.input`, `part.state.output` | Finished tool call |
| `tool_use` (`part.state.status="error"`) | `part.tool`, `part.id`, `part.state.input`, `part.state.error` | Recoverable tool failure |
| `step_finish` | `part.tokens`, `part.cost` | Step completed with usage stats |
| `error` | `error.message` (also accepts `error.data.message`, preferred if both exist) | Session failure reported at EOF |
| `retry` | `messageID`, `attempt`, `nextRetryAt`, `error` | Informational; ignored, not a session failure |
| `background_wait` | `jobs[]` (`id`, `kind`, `title`) | Informational; ignored while later records continue normally |
| `message.part.updated` | — | Ignored |
| `session.status` | — | Ignored |
| `session.error` | — | Ignored |

### Nested Tool Records

Synthetic completed-record example:

```json
{
  "type": "tool_use",
  "timestamp": 1700000000100,
  "sessionID": "ses_synthetic",
  "part": {
    "partID": "prt_cli_part",
    "sessionID": "ses_synthetic",
    "messageID": "msg_assistant",
    "type": "tool",
    "id": "call_read",
    "tool": "read",
    "state": {
      "status": "completed",
      "input": { "path": "src/main.rs" },
      "output": "first line\nsecond line",
      "title": "read",
      "metadata": { "metadata": { "truncated": false } },
      "time": { "start": 1700000000000, "end": 1700000000100 }
    }
  }
}
```

Each completed/error record emits `ToolStart`, `ToolReady`, then `ToolResult`.
All three use **`part.id`**, the call ID, not the distinct CLI `part.partID`.
Absent call IDs remain `None`; absent tool names display as `unknown`.
Completed content comes from string `state.output`; error content comes from
string `state.error`, never `state.output`. Missing/non-string content is empty.
Unknown object-state statuses are ignored (with diagnostics under `--debug`);
no nested running/progress lifecycle is synthesized.

OpenCode-local summary rules, without changing the displayed tool name:

| Tool | Summary from `state.input` |
|---|---|
| `skill` | String `id`, otherwise generic JSON fallback |
| `read` | String `path` (including `skill://…` URIs), otherwise generic JSON fallback |
| `shell` | Existing Bash `command` summary (80-character truncation/fallback) |
| `subagent` | Existing Agent `description` summary (60-character truncation) and optional `model` |
| `glob`, `grep`, others | Existing shared `tool_summary` rules |

Successful output is retained for line counts but not printed, including loaded
skill bodies. A failed tool shows only the first error line and does not mark the
session failed.

The existing string-state format remains supported independently: `part.state`
is `"running"`, `"completed"`, or `"error"`; running records use `part.name` and
`part.input`, while completed/error records use `part.output`. These records
retain their existing events and absent call IDs.

### Token/Cost Structure in `step_finish`

```json
{
  "type": "step_finish",
  "part": {
    "tokens": {
      "input": 1000,
      "output": 200,
      "reasoning": 50,
      "cache": { "read": 500, "write": 25 }
    },
    "cost": 0.05
  }
}
```

The parser accumulates `input`, visible `output`, `cache.read`, and cost without
changing existing footer mappings. Reasoning tokens and cache writes are not
fully represented; the footer is not complete token accounting.

### Session Lifecycle

OpenCode has no explicit session-start or session-end event. The parser:
1. Emits `SessionStart` on the first `step_start` event only (subsequent ones are ignored)
2. Accumulates tokens and cost from all `step_finish` events
3. Emits `SessionEnd` from `finish()` at EOF with accumulated stats

Text and reasoning normally arrive as completed blocks. Reconciliation can emit
missed content or a suffix; agentcat preserves supplied content and arrival order
without replacement, history lookup, or additional deduplication.

**Accepted limitations:** The completed-record adapter can briefly show a
synthetic TTY spinner. Tool and completed-reasoning timings may measure local
rendering rather than upstream execution/thinking; upstream durations are not
used. `is_error` reflects only tool state: shell `exit`, `signal`, `timeout`, and
`status` metadata is not interpreted, so a completed tool can display success
despite command failure or still-running background execution. A completed
subagent tool can likewise acknowledge background work that is not finished.
Error-only streams without `step_start` require `--format opencode`; auto-detection
is unchanged. Provider/session `error` records fail the session at EOF, unlike
recoverable tool-state errors.

---

## Internal Event Model

All five formats are parsed into a common internal event stream. The renderer only sees `AgentEvent` values — it is completely format-agnostic.

```rust
/// Events emitted by all parsers, consumed by the renderer.
enum AgentEvent {
    /// Session started. Emitted once, first.
    SessionStart {
        session_id: String,
        agent: AgentKind,
        model: Option<String>,
    },

    /// Incremental text chunk — print immediately, flush.
    TextDelta(String),

    /// Complete text block — print at once (fallback when no deltas).
    TextComplete(String),

    /// Extended thinking / reasoning started.
    ThinkingStart,

    /// Incremental thinking text chunk.
    ThinkingDelta(String),

    /// Extended thinking / reasoning ended.
    ThinkingEnd,

    /// Tool call started (name known, input may still be streaming).
    ToolStart { tool_name: String },

    /// Tool call input fully parsed — display summary line.
    ToolReady { tool_name: String, input_summary: String },

    /// Tool execution result.
    ToolResult { is_error: bool, content: String },

    /// Context was compacted.
    Compaction,

    /// Non-fatal warning.
    Warning(String),

    /// Session finished. All fields optional — show what's available.
    SessionEnd {
        success: bool,
        error_type: Option<String>,
        error_message: Option<String>,
        num_turns: Option<u32>,
        duration_ms: Option<u64>,
        api_duration_ms: Option<u64>,
        cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_tokens: Option<u64>,
    },
}

enum AgentKind {
    Claude,
    Pi,
    Gemini,
    Codex,
    Unknown,
}
```

---

## Event Mapping Tables

### Claude Code → AgentEvent

| Claude event | AgentEvent |
|-------------|------------|
| `system` (subtype=`"init"`) | `SessionStart { session_id, agent: Claude, model: None }` |
| `stream_event` → `content_block_start` (type=`"text"`) | (set internal state to Text block) |
| `stream_event` → `content_block_delta` (delta.type=`"text_delta"`) | `TextDelta(delta.text)` |
| `stream_event` → `content_block_start` (type=`"thinking"`) | `ThinkingStart` |
| `stream_event` → `content_block_delta` (delta.type=`"thinking_delta"`) | `ThinkingDelta(delta.thinking)` |
| `stream_event` → `content_block_stop` (was thinking) | `ThinkingEnd` |
| `stream_event` → `content_block_start` (type=`"tool_use"`) | `ToolStart { name }` + start buffering input |
| `stream_event` → `content_block_delta` (delta.type=`"input_json_delta"`) | (append to buffer) |
| `stream_event` → `content_block_stop` (was tool_use) | Parse buffered JSON → `ToolReady { name, summary }` |
| `assistant` message with text block (if no prior stream_events for this block) | `TextComplete(text)` |
| `assistant` message with tool_use block (if no prior stream_events) | `ToolStart` + `ToolReady` |
| `assistant` message with thinking block (if no prior stream_events) | `ThinkingStart` + `ThinkingDelta` + `ThinkingEnd` |
| `user` message containing `tool_result` blocks | `ToolResult { is_error, content }` for each |
| `system` (subtype=`"compact_boundary"`) | `Compaction` |
| `result` | `SessionEnd { success, error_type, num_turns, duration_ms, api_duration_ms, cost_usd, input_tokens, output_tokens, cached_tokens }` |

**State tracking needed:** The Claude parser needs a `BlockState` to track the current content block type (text, tool_use, thinking) and buffer tool input JSON. Also needs a `saw_stream_events` flag to know whether to emit events from `assistant` messages or suppress them (avoiding duplicates).

### pi.dev → AgentEvent

| pi.dev event | AgentEvent |
|-------------|------------|
| `session` | `SessionStart { id, agent: Pi, model: None }` |
| `message_update` → `text_delta` | `TextDelta(delta)` |
| `message_update` → `text_end` (if no prior text_deltas) | `TextComplete(content)` |
| `message_update` → `thinking_start` | `ThinkingStart` |
| `message_update` → `thinking_delta` | `ThinkingDelta(delta)` |
| `message_update` → `thinking_end` | `ThinkingEnd` |
| `message_update` → `toolcall_start` | `ToolStart { name: "" }` (name not yet known) |
| `message_update` → `toolcall_end` | `ToolReady { name, summary }` (from toolCall object) |
| `tool_execution_end` | `ToolResult { isError, content }` |
| `auto_compaction_start` | `Compaction` |
| `message_update` → `done` | `SessionEnd { success: true, input_tokens, output_tokens }` (from message.usage) |
| `message_update` → `error` | `SessionEnd { success: false, error_message }` |

**Note:** The `done` event in pi.dev is emitted per-message. In a multi-turn session with tool use, multiple `done` events may fire (with `reason: "toolUse"` for intermediate ones). Only the final `done` (with `reason: "stop"`) should emit `SessionEnd`. Intermediate `done` events should be ignored for session-end purposes.

### Gemini CLI → AgentEvent

| Gemini event | AgentEvent |
|-------------|------------|
| `init` | `SessionStart { session_id, agent: Gemini, model }` |
| `message` (role=`"assistant"`, delta=`true`) | `TextDelta(content)` |
| `message` (role=`"assistant"`, delta absent/false) | `TextComplete(content)` |
| `message` (role=`"user"`) | (ignore — echoed prompt) |
| `tool_use` | `ToolStart { tool_name }` + `ToolReady { tool_name, summary from parameters }` |
| `tool_result` (status=`"success"`) | `ToolResult { is_error: false, output }` |
| `tool_result` (status=`"error"`) | `ToolResult { is_error: true, error.message }` |
| `error` | `Warning(message)` |
| `result` (status=`"success"`) | `SessionEnd { success: true, duration_ms, input_tokens, output_tokens, cached_tokens }` |
| `result` (status=`"error"`) | `SessionEnd { success: false, error_type, error_message, ... }` |

**Note:** Gemini has no thinking events.

### Codex CLI → AgentEvent

| Codex event | AgentEvent |
|-------------|------------|
| `thread.started` | `SessionStart { thread_id, agent: Codex, model: None }` |
| `item.started` (type=`"command_execution"`) | `ToolStart { name: "Bash" }` + `ToolReady { name: "Bash", summary: command }` |
| `item.completed` (type=`"command_execution"`) | `ToolResult { is_error: exitCode != 0, content: aggregatedOutput }` |
| `item.completed` (type=`"agent_message"`) | `TextComplete(text)` |
| `item.started` (type=`"reasoning"`) | `ThinkingStart` |
| `item.completed` (type=`"reasoning"`) | `ThinkingDelta(summary[0].text)` + `ThinkingEnd` |
| `item.started` (type=`"file_change"`) | `ToolStart { name: "FileChange" }` |
| `item.completed` (type=`"file_change"`) | `ToolReady { summary: paths }` + `ToolResult { ... }` |
| `item.started` (type=`"mcp_tool_call"`) | `ToolStart { name: "server/tool" }` |
| `item.completed` (type=`"mcp_tool_call"`) | `ToolReady { ... }` + `ToolResult { ... }` |
| `item.completed` (type=`"web_search"`) | `ToolStart` + `ToolReady { summary: query }` + `ToolResult` |
| `item.completed` (type=`"context_compaction"`) | `Compaction` |
| `turn.completed` | (accumulate usage tokens internally) |
| `turn.failed` | (set error flag internally) |
| `error` | (set error flag + message internally) |
| (EOF) | `SessionEnd { success, input_tokens, output_tokens, cached_tokens }` (accumulated) |

**Important:** Codex has no explicit "session end" event. The parser must accumulate token usage from all `turn.completed` events and emit `SessionEnd` from the `finish()` method called at EOF.

### OpenCode → AgentEvent

| OpenCode event | AgentEvent |
|-------------|------------|
| `step_start` (first only) | `SessionStart { session_id: part.sessionID or envelope sessionID, agent: OpenCode, model: None }` |
| `step_start` (subsequent) | (ignored) |
| `text` | `TextComplete(part.text)` |
| `reasoning` | `ThinkingStart` + `ThinkingDelta(part.text)` + `ThinkingEnd` |
| `tool_use` (`state.status="completed"`) | `ToolStart { tool_name: part.tool, id: part.id }` + `ToolReady { tool_name: part.tool, input_summary: summary(state.input), id: part.id }` + `ToolResult { is_error: false, content: state.output, id: part.id }` |
| `tool_use` (`state.status="error"`) | Same start/ready events + `ToolResult { is_error: true, content: state.error, id: part.id }`; no session failure |
| `tool_use` (other object-state status) | (ignored; debug diagnostic if enabled) |
| `tool_use` (string state=`"running"`) | Existing `ToolStart` + `ToolReady` using `part.name`/`part.input`, IDs `None` |
| `tool_use` (string state=`"completed"` / `"error"`) | Existing `ToolResult { is_error: state == "error", content: part.output, id: None }` |
| `retry`, `background_wait` | (ignored; no session failure) |
| `step_finish` | (accumulate tokens and cost internally) |
| `error` | (set error flag + message internally) |
| (EOF) | `SessionEnd { success, cost_usd, input_tokens, output_tokens, cached_tokens }` (accumulated) |

**Important:** Like Codex, OpenCode has no explicit session-end event. The parser accumulates token usage and cost from all `step_finish` events and emits `SessionEnd` from `finish()` at EOF.

---

## Output Rendering

### Session Init

```
🐱 agentcat — session abc-123 (claude, claude-sonnet-4-6)
🐱 agentcat — session 550e8400-... (pi)
🐱 agentcat — session test-123 (gemini, gemini-2.5-pro)
🐱 agentcat — session 0199a213-... (codex)
🐱 agentcat — session ses_abc-... (opencode)
```

Show model name when available. Truncate long session IDs for readability.

### Streaming Text

Token-by-token `TextDelta` events are printed **immediately** to stdout as they arrive. Flush stdout after each delta to achieve the typewriter effect.

For `TextComplete` events (fallback when no deltas): print the full text block at once followed by a newline.

### Tool Use

**On `ToolReady`:**
```
🔧 Bash: ls -la src/
🔧 Read: src/main.rs
🔧 Edit: src/lib.rs
🔧 Grep: "TODO" in src/
🔧 WebSearch: "rust async stdin"
🔧 FileChange: src/main.rs, src/lib.rs
🔧 my-server/search: {"query": "hello"}
```

Format: `🔧 {tool_name}: {compact_summary_of_input}`

**Tool summary extraction rules** (case-insensitive matching, applied across all agents):

| Tool name pattern | Summary |
|-------------------|---------|
| `Bash`, `bash`, `command_execution` | `command` field, truncated to ~80 chars |
| `Read`, `read_file` | `file_path` field |
| `Write`, `write_file` | `file_path` field |
| `Edit`, `edit_file` | `file_path` field |
| `Glob` | `pattern` field |
| `Grep`, `grep` | `pattern` field + `path` if present |
| `WebSearch`, `web_search` | `query` field |
| `WebFetch` | `url` field |
| `FileChange`, `file_change` | comma-joined paths from `changes[]` |
| MCP tool calls | `"{server}/{tool}"` |
| Other | tool name + first ~60 chars of JSON input |

**Per-tool inline spinner (TTY only):**

Each tool gets its own spinner on the line directly below its header. The spinner shows elapsed time and animates at 80ms intervals using braille frames (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`), or ASCII (`|/-\`) when `--no-emoji` is set.

When tools run in parallel (multiple `ToolReady` events before any `ToolResult`), each tool occupies 2 lines (header + status). ANSI cursor movement (`\x1b[nA` / `\x1b[nB`) is used to update each tool's status line independently. The cursor always returns to the "home" position (line after the last status line). The status line offset for slot `i` of `N` tools is `(N - i) * 2 - 1` lines above home.

```
🔧 Read: file1.rs
  ⠹ running... (0.5s)          ← spinner, updated in-place
🔧 Read: file2.rs
  ⠹ running... (0.3s)
🔧 Bash: ls -la
  ⠹ running... (0.2s)
```

**On `ToolResult`:**

Each result replaces the spinner for its corresponding tool (FIFO order). Cursor movement updates only that tool's status line.

```
🔧 Read: file1.rs
  ✅ (10 lines, 0.8s)           ← first result replaces first spinner
🔧 Read: file2.rs
  ⠹ running... (0.6s)          ← still running
🔧 Bash: ls -la
  ✅ (3 lines, 2.0s)            ← third result
```

Result format:
- Success: `  ✅ ({N} line(s), {elapsed}s)` — line count of result content + elapsed time
- Success (empty content): `  ✅ ({elapsed}s)`
- Error: `  ❌ {first_line_of_error}, {elapsed}s`
- For Codex command executions with non-zero exit code, show `exit {code}`

**Non-TTY (piped output):**

No cursor movement or spinners. Tool headers are printed as they arrive; completion marks are appended sequentially below:

```
🔧 Read: file1.rs
🔧 Read: file2.rs
🔧 Bash: ls -la
  ✅ (10 lines, 0.8s)
  ✅ (50 lines, 1.2s)
  ✅ (3 lines, 2.0s)
```

### Thinking / Reasoning Blocks

**Default (hidden):** When `ThinkingStart` arrives, start a timer. When `ThinkingEnd` arrives, print:
```
💭 Thinking... (2.3s)
```

**With `--show-thinking`:** Stream `ThinkingDelta` text in dim/gray as it arrives. When `ThinkingEnd` arrives, print duration:
```
💭 [thinking text rendered in dim gray...]
💭 (2.3s)
```

For Codex `reasoning` items: display the `summary` text (human-readable reasoning summary) as `ThinkingDelta`.

### Warnings

For `Warning` events (Gemini `error` with `severity: "warning"`):
```
⚠️  Loop detected, stopping execution
```

### Result / Summary

Print a separator line, then show available stats. Only include fields that have values.

**Claude Code (all fields available):**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✨ Done — 3 turns, 12.3s (9.8s API)
💰 $0.0042 — 1,500 in / 800 out tokens
```

**Gemini (no cost, has tool_calls and cached):**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✨ Done — 3.2s, 2 tool calls
📊 1,500 in / 800 out tokens (500 cached)
```

**Codex (no cost, no duration):**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✨ Done
📊 24,763 in / 122 out tokens (24,448 cached)
```

**pi.dev (may have limited stats):**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✨ Done
📊 1,500 in / 800 out tokens
```

**On error (any agent):**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
❌ Error: MaxSessionTurnsError — Maximum session turns exceeded
📊 5,000 in / 2,000 out tokens
```

### Context Compaction

```
📦 Context compacted
```

### Unknown Event Types

Silently ignore any unrecognized `type` values. This ensures forward compatibility when agents add new event types.

---

## Color Scheme

ANSI colors applied via `crossterm`.

| Element | Color / Style |
|---------|---------------|
| Session init line | Bold cyan |
| Assistant text (TextDelta/TextComplete) | Default (terminal foreground) |
| Tool name + summary (ToolReady) | Cyan |
| Tool result success marker (✅) | Green |
| Tool result error marker (❌) | Red |
| Thinking text (ThinkingDelta) | Dim / gray |
| Thinking duration | Dim |
| Warning text (⚠️) | Yellow |
| Result summary line (✨ / ❌) | Bold |
| Cost/stats line (💰 / 📊) | Yellow |
| Separator (━━━) | Dim |

**Disable colors when:**
- `--no-color` flag is passed, OR
- `NO_COLOR` environment variable is set (any value), OR
- stdout is not a TTY (detect with `crossterm::tty::IsTty`)

**Disable emojis when:**
- `--no-emoji` flag is passed

When emojis are disabled, use text prefixes instead:
| Emoji | Text replacement |
|-------|-----------------|
| 🐱 | `[agentcat]` |
| 🔧 | `[tool]` |
| ✅ | `[ok]` |
| ❌ | `[err]` |
| 💭 | `[think]` |
| ⚠️ | `[warn]` |
| 📦 | `[compact]` |
| ✨ | `[done]` |
| 💰 | `[cost]` |
| 📊 | `[stats]` |

---

## Architecture

### Crates

| Crate | Purpose |
|-------|---------|
| `tokio` (features: `full`) | Async runtime, async stdin reading |
| `serde` (features: `derive`) | Deserialization derives |
| `serde_json` | JSON parsing |
| `crossterm` | ANSI colors, styling, TTY detection |
| `clap` (features: `derive`) | CLI argument parsing with derive macros |

### Pipeline

```
stdin (NDJSON)
  → tokio::io::BufReader → lines()
  → Format detector (first line's "type" field)
  → Parser (impl EventParser) → Vec<AgentEvent> per line
  → Renderer → styled writes to stdout
  → stdout (with flush after each render)
```

### Core Loop (pseudocode)

```
1. Parse CLI args with clap
2. Read first line from stdin (async)
3. If --format given, use that; else detect format from first line's "type" field
4. Create appropriate parser: ClaudeParser | PiParser | GeminiParser | CodexParser
5. For each line (including the first):
   a. let events = parser.parse(line)?;
   b. for event in events { renderer.render(event)?; }
   c. stdout.flush()?;
6. let final_events = parser.finish();
   for event in final_events { renderer.render(event)?; }
7. Exit with code based on last SessionEnd event
```

### Key Data Structures

```rust
/// Trait implemented by each format-specific parser.
trait EventParser {
    /// Parse one NDJSON line into zero or more AgentEvents.
    fn parse(&mut self, line: &str) -> Result<Vec<AgentEvent>>;

    /// Called at EOF. Emit any remaining events (e.g. Codex accumulated SessionEnd).
    fn finish(&mut self) -> Vec<AgentEvent>;
}

/// Claude Code parser — needs block state tracking.
struct ClaudeParser {
    block_state: BlockState,
    saw_stream_events: bool,
}

/// pi.dev parser — tracks whether text deltas were seen per text block.
struct PiParser {
    saw_text_deltas: bool,
    last_done_reason: Option<String>,
}

/// Gemini parser — minimal state.
struct GeminiParser {}

/// Codex parser — accumulates usage across turns.
struct CodexParser {
    accumulated_input_tokens: u64,
    accumulated_output_tokens: u64,
    accumulated_cached_tokens: u64,
    had_error: bool,
    error_message: Option<String>,
}

/// State machine for Claude's content block tracking.
enum BlockState {
    None,
    Text,
    ToolUse { name: String, json_buf: String },
    Thinking { started: Instant },
}

/// Renderer configuration and state.
struct Renderer {
    show_thinking: bool,
    use_emoji: bool,
    use_color: bool,
    thinking_start: Option<Instant>,
}
```

---

## Project Structure

```
agentcat/
├── Cargo.toml
├── src/
│   ├── main.rs            # Entry point: CLI args (clap), async main loop
│   ├── event.rs           # AgentEvent enum, AgentKind enum
│   ├── parse/
│   │   ├── mod.rs         # EventParser trait, detect_format(), tool_summary() helper
│   │   ├── claude.rs      # ClaudeParser — Claude Code stream-json
│   │   ├── pi.rs          # PiParser — pi.dev JSON mode
│   │   ├── gemini.rs      # GeminiParser — Gemini CLI stream-json
│   │   ├── codex.rs       # CodexParser — Codex CLI --json
│   │   └── opencode.rs    # OpenCodeParser — OpenCode JSON format
│   ├── render.rs          # Renderer: AgentEvent → styled terminal output
│   └── style.rs           # Color/style helpers, emoji mappings, NO_COLOR support
```

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Agent session completed successfully |
| 1 | Agent reported an error (any error result/subtype) |
| 2 | agentcat itself failed (parse error, IO error, unrecognized format, empty stdin) |

---

## Verification

### Smoke Tests

1. **Claude — token-by-token streaming:**
   ```bash
   claude -p "say hello" --output-format stream-json --verbose --include-partial-messages | cargo run
   ```
   Expected: streaming text with session header and result footer.

2. **Claude — tool use:**
   ```bash
   claude -p "list files in /tmp" --output-format stream-json --verbose --include-partial-messages | cargo run
   ```
   Expected: tool call summary, result, then text response.

3. **Claude — without stream_events (fallback):**
   ```bash
   claude -p "say hello" --output-format stream-json | cargo run
   ```
   Expected: complete text blocks rendered (no typewriter effect).

4. **pi.dev — streaming:**
   ```bash
   pi --mode json "say hello" 2>/dev/null | cargo run
   ```

5. **Gemini — streaming:**
   ```bash
   gemini -o stream-json -p "say hello" | cargo run
   ```

6. **Codex — complete messages:**
   ```bash
   codex exec --json "say hello" | cargo run
   ```

### Flag Tests

7. **No emoji:**
   ```bash
   ... | cargo run -- --no-emoji
   ```
   Expected: text prefixes like `[tool]`, `[done]` instead of emojis.

8. **No color:**
   ```bash
   ... | cargo run -- --no-color
   ```
   Expected: no ANSI escape codes in output.

9. **Show thinking:**
   ```bash
   ... | cargo run -- --show-thinking
   ```
   Expected: thinking text visible in dim gray.

10. **Format override:**
    ```bash
    ... | cargo run -- --format claude
    ```

### Synthetic Event Tests

11. **Claude error result:**
    ```bash
    printf '{"type":"system","subtype":"init","session_id":"x"}\n{"type":"result","subtype":"error_max_turns","is_error":true,"total_cost_usd":0.01,"num_turns":5,"duration_ms":30000,"duration_api_ms":25000,"usage":{"input_tokens":5000,"output_tokens":2000},"session_id":"x"}' | cargo run
    ```
    Expected: error summary, exit code 1.

12. **Codex error:**
    ```bash
    printf '{"type":"thread.started","thread_id":"test-123"}\n{"type":"error","error":{"message":"Context window exceeded","codexErrorInfo":"ContextWindowExceeded"}}' | cargo run
    ```
    Expected: error output, exit code 1.

13. **Gemini with tool use:**
    ```bash
    printf '{"type":"init","timestamp":"2025-01-01T00:00:00Z","session_id":"g-1","model":"gemini-2.5-pro"}\n{"type":"tool_use","timestamp":"2025-01-01T00:00:01Z","tool_name":"Read","tool_id":"r-1","parameters":{"file_path":"main.rs"}}\n{"type":"tool_result","timestamp":"2025-01-01T00:00:02Z","tool_id":"r-1","status":"success","output":"fn main() {}"}\n{"type":"result","timestamp":"2025-01-01T00:00:03Z","status":"success","stats":{"total_tokens":100,"input_tokens":50,"output_tokens":50,"cached":0,"input":50,"duration_ms":3000,"tool_calls":1,"models":{}}}' | cargo run
    ```

14. **Unknown event types (forward compat):**
    ```bash
    printf '{"type":"system","subtype":"init","session_id":"x"}\n{"type":"totally_new_event","data":"whatever"}\n{"type":"result","subtype":"success","result":"done","is_error":false,"total_cost_usd":0.001,"num_turns":1,"duration_ms":1000,"duration_api_ms":800,"usage":{"input_tokens":100,"output_tokens":50},"session_id":"x"}' | cargo run
    ```
    Expected: unknown event silently ignored, session completes normally.

---

## Future Extensibility

Adding a new agent format requires:
1. Add a new file in `src/parse/` implementing the `EventParser` trait
2. Add format detection logic in `parse/mod.rs` (match on first line's `type`)
3. Add a variant to `AgentKind` enum and the `--format` CLI flag

No changes needed to `render.rs` or `style.rs` — all agents map to the same `AgentEvent` model.

---

## Sources & References

### Claude Code
- [Claude Code headless/programmatic usage](https://code.claude.com/docs/en/headless) — official docs for `--output-format stream-json`
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference) — all CLI flags
- [Claude API Messages streaming](https://platform.claude.com/docs/en/api/messages-streaming) — raw API streaming events (message_start, content_block_delta, etc.)
- [Claude Agent SDK streaming output](https://platform.claude.com/docs/en/agent-sdk/streaming-output) — streaming in Agent SDK context
- [Claude Agent SDK agent loop](https://platform.claude.com/docs/en/agent-sdk/agent-loop) — how the agent loop works
- [GitHub Issue #24596](https://github.com/anthropics/claude-code/issues/24596) — stream-json lacks event type reference
- [Blog: Extracting text from Claude Code JSON stream](https://www.ytyng.com/en/blog/claude-stream-json-jq) — practical jq recipes

### pi.dev
- [pi.dev official website](https://pi.dev/)
- [pi-mono GitHub repository](https://github.com/badlogic/pi-mono)
- [pi-mono JSON mode docs](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/json.md) — complete JSONL event reference
- [pi-mono RPC mode docs](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/rpc.md) — bidirectional protocol
- [pi-mono SDK docs](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/sdk.md) — programmatic embedding
- [@mariozechner/pi-coding-agent on npm](https://www.npmjs.com/package/@mariozechner/pi-coding-agent)
- [Blog: What I learned building a coding agent](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)

### Gemini CLI
- [Gemini CLI GitHub repository](https://github.com/google-gemini/gemini-cli)
- [Gemini CLI headless mode docs](https://geminicli.com/docs/cli/headless/) — non-interactive/headless usage
- [Gemini CLI reference](https://geminicli.com/docs/reference/configuration/) — configuration and CLI flags
- [GitHub Issue #8203](https://github.com/google-gemini/gemini-cli/issues/8203) — `stream-json` output format feature request/implementation
- Source files:
  - `/packages/core/src/output/types.ts` — TypeScript type definitions for all events
  - `/packages/core/src/output/stream-json-formatter.ts` — StreamJsonFormatter implementation
  - `/packages/core/src/output/stream-json-formatter.test.ts` — comprehensive test cases
  - `/packages/cli/src/nonInteractiveCli.ts` — headless mode event loop

### Codex CLI
- [Codex CLI reference](https://developers.openai.com/codex/cli/reference) — command line options
- [Codex non-interactive mode](https://developers.openai.com/codex/noninteractive/) — `codex exec --json` docs
- [Codex app-server protocol](https://developers.openai.com/codex/app-server/) — full JSON-RPC 2.0 protocol
- [Codex CLI GitHub repository](https://github.com/openai/codex)
- [Codex app-server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [GitHub Issue #4776](https://github.com/openai/codex/issues/4776) — JSON output field naming changes
- [GitHub PR #5546](https://github.com/openai/codex/pull/5546) — item streaming events
- [Codex SDK TypeScript](https://github.com/openai/codex/blob/main/sdk/typescript/README.md) — SDK with `runStreamed()`
- [Codex CLI features](https://developers.openai.com/codex/cli/features/)

### Similar Projects
- [claude-stream-format](https://github.com/jemmyw/claude-stream-format) — similar tool for formatting Claude Code JSON streams

### General
- [NO_COLOR standard](https://no-color.org/) — convention for disabling color output
- [NDJSON specification](http://ndjson.org/) — newline-delimited JSON format
