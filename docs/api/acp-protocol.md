# ACP (Agent Client Protocol) Reference

The Agent Client Protocol is a JSON-RPC 2.0 based protocol for controlling Hone agents. Messages are exchanged over stdio (standard input/output) in a request-response or notification pattern.

## Protocol Overview

- **Version:** JSON-RPC 2.0
- **Transport:** Text-based messages over stdio, newline-delimited
- **Encoding:** UTF-8 JSON
- **Message Flow:** Client sends requests, agent responds with results or errors; agent sends notifications asynchronously

## Message Format

### Request

A JSON-RPC request from client to agent:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": { ... }
}
```

**Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `jsonrpc` | string | Yes | Protocol version, always `"2.0"` |
| `id` | integer | Yes | Unique request identifier, must be present |
| `method` | string | Yes | RPC method name |
| `params` | object | No | Method parameters |

### Response

A JSON-RPC response from agent to client:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { ... }
}
```

Or on error:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32600,
    "message": "Invalid Request"
  }
}
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `jsonrpc` | string | Protocol version, always `"2.0"` |
| `id` | integer | Matches request ID |
| `result` | any | Result value (present if successful) |
| `error` | object | Error object (present if failed) |

**Error Object:**

| Field | Type | Description |
|-------|------|-------------|
| `code` | integer | Error code (negative integers reserved for protocol errors) |
| `message` | string | Human-readable error message |

### Notification

A JSON-RPC notification from agent to client (no response expected):

```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": { ... }
}
```

**Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `jsonrpc` | string | Yes | Protocol version, always `"2.0"` |
| `method` | string | Yes | RPC method name |
| `params` | object | No | Notification parameters |
| `id` | integer | No | Must NOT be present in notifications |

---

## Core Methods

### initialize

Initialize the protocol connection. Client sends this first to establish protocol version and exchange capability information.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientInfo": {
      "name": "hone-cli",
      "version": "0.1.0"
    }
  }
}
```

**Request Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `protocolVersion` | integer | ACP protocol version (currently `1`) |
| `clientInfo.name` | string | Client name (e.g., `hone-cli`, `hone-ui`) |
| `clientInfo.version` | string | Client version string |

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": 1,
    "agentInfo": {
      "name": "hone-agent",
      "version": "0.1.0"
    }
  }
}
```

**Response Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `protocolVersion` | integer | Negotiated protocol version |
| `agentInfo.name` | string | Agent implementation name |
| `agentInfo.version` | string | Agent implementation version |

---

### session/new

Create a new agent session.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/new",
  "params": {
    "cwd": "/home/user/project"
  }
}
```

**Request Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `cwd` | string | Working directory for the agent session |

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "sessionId": "sess_abc123def456"
  }
}
```

**Response Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionId` | string | Unique session identifier for subsequent requests |

---

### session/prompt

Send a user prompt to the agent for processing.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "session/prompt",
  "params": {
    "sessionId": "sess_abc123def456",
    "prompt": [
      {
        "type": "text",
        "text": "Read /etc/hosts please"
      }
    ],
    "tools": [
      {
        "name": "fs.read",
        "description": "Read a file from the filesystem",
        "parameters": {
          "type": "object",
          "properties": {
            "path": {"type": "string"},
            "depth": {"type": "integer"}
          },
          "required": ["path"]
        }
      }
    ]
  }
}
```

**Request Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionId` | string | Session ID from `session/new` |
| `prompt` | array | Array of content items |
| `tools` | array \| absent | Tool schemas the client makes available for this turn. Empty / absent means no tools — the agent MUST NOT issue `tool_call` notifications in that case. |

**Prompt Content Items:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Content type: `text`, `image`, `file`, etc. |
| `text` | string | Text content (for type `text`) |

**Tool Schema Items:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Tool identifier; agents echo this back as the `title` of the corresponding `tool_call` notification. |
| `description` | string | Human-readable purpose; surfaced to the model. |
| `parameters` | object | JSON Schema describing the tool's arguments. Inlined as a JSON object — never double-encoded as a string. |

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "stopReason": "end_turn"
  }
}
```

**Response Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `stopReason` | string | Why the turn stopped: `end_turn`, `tool_use`, `max_tokens`, etc. |

**Notifications During Processing:**

While processing a prompt, the agent sends `session/update` notifications with event data (see below).

---

### session/update (Notification)

The agent sends these notifications asynchronously during prompt processing. These are notifications (no response required from client).

**Notification:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {
        "type": "text",
        "text": "Hello"
      }
    }
  }
}
```

**Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionId` | string | Session ID |
| `update` | object | Update payload (polymorphic, see variants below) |

#### agent_message_chunk

Agent is streaming text output.

**Update Format:**
```json
{
  "sessionUpdate": "agent_message_chunk",
  "content": {
    "type": "text",
    "text": "I'll help"
  }
}
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionUpdate` | string | Always `"agent_message_chunk"` |
| `content.type` | string | Content type: `text` |
| `content.text` | string | Text chunk |

#### tool_call

Agent initiated a tool call.

**Update Format:**
```json
{
  "sessionUpdate": "tool_call",
  "toolCallId": "call_abc123",
  "title": "fs.read",
  "kind": "filesystem",
  "status": "in_progress",
  "rawInput": {
    "path": "/etc/hosts",
    "depth": 2
  }
}
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionUpdate` | string | Always `"tool_call"` |
| `toolCallId` | string | Unique tool call identifier; correlates with the `session/toolResult` the client sends back and the eventual `tool_call_update` |
| `title` | string | Tool name as advertised in `session/prompt.tools[].name` (used by the client to look up the dispatcher). |
| `kind` | string | Tool category (e.g., `filesystem`, `database`, `network`) |
| `status` | string | Call status: `pending`, `in_progress`, `completed` |
| `rawInput` | object \| absent | Tool arguments as a JSON object. When omitted (legacy agents), clients MUST default to `{}`. Nested objects/arrays are preserved verbatim — never double-encoded as a JSON string. |

After receiving a `tool_call`, the client is expected to dispatch the tool locally and reply with a `session/toolResult` request (see below) carrying the same `toolCallId`. The agent then resumes the prompt and emits a terminal `tool_call_update`.

#### tool_call_update

Tool call status changed (e.g., from pending to completed). Typically sent **after** the agent has consumed the client's `session/toolResult` and decided whether the call succeeded.

**Update Format:**
```json
{
  "sessionUpdate": "tool_call_update",
  "toolCallId": "call_abc123",
  "status": "completed"
}
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionUpdate` | string | Always `"tool_call_update"` |
| `toolCallId` | string | Matches preceding `tool_call` ID |
| `status` | string | New status: `in_progress`, `completed`, `failed`, `error`. The wire value is preserved verbatim by clients; `failed` and `error` (case-insensitive) are classified as terminal failures. |

---

### session/toolResult (Request — Client → Agent)

Sent by the client in response to a `tool_call` notification. The agent blocks the in-flight `session/prompt` on this request, so the client MUST reply for the prompt to resume.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 17,
  "method": "session/toolResult",
  "params": {
    "sessionId": "sess_abc123def456",
    "toolCallId": "call_abc123",
    "content": {"text": "127.0.0.1 localhost"},
    "isError": false
  }
}
```

**Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionId` | string | The session that issued the `tool_call`. |
| `toolCallId` | string | MUST match the `toolCallId` from the originating `tool_call` notification. |
| `content` | any (JSON value) | Tool output. Strings, objects, arrays, or numbers all flow through verbatim — no double-encoding. |
| `isError` | bool | `true` if the tool failed. Drives the agent's downstream `tool_call_update.status`. |

**Response:** the agent acknowledges with a standard JSON-RPC result object; the response payload itself is unused by the client and may be discarded.

---

### session/cancel (Notification)

Client can request to cancel an ongoing prompt processing.

**Notification:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/cancel",
  "params": {
    "sessionId": "sess_abc123def456"
  }
}
```

**Parameters:**

| Field | Type | Description |
|-------|------|-------------|
| `sessionId` | string | Session to cancel |

The agent will stop processing and may send a final error notification. This is a fire-and-forget notification (no response).

---

## Error Codes

Standard JSON-RPC 2.0 error codes:

| Code | Message | Description |
|------|---------|-------------|
| `-32700` | Parse error | Invalid JSON |
| `-32600` | Invalid Request | Malformed request object |
| `-32601` | Method not found | Unknown method |
| `-32602` | Invalid params | Invalid method parameters |
| `-32603` | Internal error | Agent internal error |
| `-32000` | Session not found | Referenced session does not exist |
| `-32001` | Invalid session state | Session is in wrong state for operation |

---

## Example: Complete Conversation Flow

```
Client -> Server (Request):
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientInfo": {"name": "cli", "version": "1.0"}
  }
}

Server -> Client (Response):
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": 1,
    "agentInfo": {"name": "hone", "version": "1.0"}
  }
}

Client -> Server (Request):
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/new",
  "params": {"cwd": "/tmp"}
}

Server -> Client (Response):
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {"sessionId": "sess_123"}
}

Client -> Server (Request):
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "session/prompt",
  "params": {
    "sessionId": "sess_123",
    "prompt": [{"type": "text", "text": "List the files in /tmp"}],
    "tools": [
      {
        "name": "fs.list",
        "description": "List filesystem entries",
        "parameters": {
          "type": "object",
          "properties": {"path": {"type": "string"}},
          "required": ["path"]
        }
      }
    ]
  }
}

Server -> Client (Notification):
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_123",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {"type": "text", "text": "Let me check…"}
    }
  }
}

Server -> Client (Notification — bidirectional handoff):
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_123",
    "update": {
      "sessionUpdate": "tool_call",
      "toolCallId": "call_1",
      "title": "fs.list",
      "kind": "filesystem",
      "status": "in_progress",
      "rawInput": {"path": "/tmp"}
    }
  }
}

Client -> Server (Request — fulfilling the tool call):
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "session/toolResult",
  "params": {
    "sessionId": "sess_123",
    "toolCallId": "call_1",
    "content": {"entries": ["file1.txt", "file2.txt"]},
    "isError": false
  }
}

Server -> Client (Response — toolResult ack, payload unused):
{"jsonrpc": "2.0", "id": 4, "result": null}

Server -> Client (Notification):
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_123",
    "update": {
      "sessionUpdate": "tool_call_update",
      "toolCallId": "call_1",
      "status": "completed"
    }
  }
}

Server -> Client (Notification):
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_123",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {"type": "text", "text": "I found file1.txt and file2.txt."}
    }
  }
}

Server -> Client (Response):
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {"stopReason": "end_turn"}
}
```

---

## Implementation Notes

1. **Message Framing:** Each JSON message must be followed by a newline character (`\n`)
2. **Concurrent Requests:** Clients may send multiple requests before receiving responses; use `id` to correlate
3. **Notifications Don't Block:** Agents send notifications independently; clients should not wait for their receipt
4. **Session Lifetime:** Sessions persist on the agent until explicitly closed or the agent terminates
5. **Prompt Parameters:** The `prompt` array can contain multiple content items for multi-modal prompts
6. **Tool Dispatch (bidirectional):** Tools advertised on `session/prompt.tools[]` enable the agent to issue `tool_call` notifications. The client MUST reply with a `session/toolResult` request bearing the same `toolCallId`; the agent then emits a terminal `tool_call_update` and resumes the prompt. Hone's `AcpProvider` surfaces this lifecycle to the dispatcher as `ProviderEvent::ToolCall` (with `arguments` parsed from `rawInput`, defaulting to `{}` when absent) → caller-supplied `ToolResult` → `ProviderEvent::ObservedToolEnd` (status verbatim; `failed`/`error` flagged as `is_error: true`, case-insensitive) → `ProviderEvent::Done`. See `crates/hone-providers/tests/acp_full_lifecycle.rs` for the executable contract.
