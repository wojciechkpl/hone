# Server-Sent Events (SSE) Reference

The Hone agent streams events to connected clients using Server-Sent Events (SSE). This document describes the event format and all event types.

## Connection Setup

SSE endpoints are established via HTTP GET requests with the `Accept: text/event-stream` header:

```bash
curl http://localhost:8080/sessions/sess_abc123/stream \
  -H "Authorization: Bearer <token>" \
  -H "Accept: text/event-stream"
```

The server will keep the connection open and stream events line-by-line.

## Event Format

Each event is formatted according to the SSE specification:

```
id: {event_id}
event: {event_type}
data: {json_payload}

```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique event identifier (monotonically increasing) |
| `event` | string | Event type name |
| `data` | JSON | Event payload (serialized as JSON) |

Empty line separates events.

## Event Types

### token

A text token from the model's response.

**Event Type:** `token`

**Data Format:**
```json
{
  "type": "token",
  "text": "Hello"
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"token"` |
| `text` | string | The text token (may be partial word, sentence, or complete token) |

**Example:**
```
id: 1
event: token
data: {"type":"token","text":"Hello"}

id: 2
event: token
data: {"type":"token","text":" world"}

```

---

### tool_call_start

The model requested execution of a tool.

**Event Type:** `tool_call_start`

**Data Format:**
```json
{
  "type": "tool_call_start",
  "id": "call_abc123",
  "name": "list_files"
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"tool_call_start"` |
| `id` | string | Unique tool call identifier |
| `name` | string | Name of the tool being called |

**Note:** The arguments passed to the tool are sent in a `tool_call_update` event (see below).

**Example:**
```
id: 10
event: tool_call_start
data: {"type":"tool_call_start","id":"call_abc123","name":"list_files"}

```

---

### tool_call_end

A tool call completed with a result.

**Event Type:** `tool_call_end`

**Data Format:**
```json
{
  "type": "tool_call_end",
  "id": "call_abc123",
  "result": {
    "files": ["file1.txt", "file2.txt"]
  },
  "is_error": false
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"tool_call_end"` |
| `id` | string | Tool call identifier (matches preceding `tool_call_start`) |
| `result` | JSON | Result returned by the tool |
| `is_error` | boolean | True if result is an error message |

**Error Example:**
```json
{
  "type": "tool_call_end",
  "id": "call_abc123",
  "result": {
    "error": "Permission denied"
  },
  "is_error": true
}
```

**Example:**
```
id: 11
event: tool_call_end
data: {"type":"tool_call_end","id":"call_abc123","result":{"files":["file1.txt","file2.txt"]},"is_error":false}

```

---

### error

An error occurred during the agent turn.

**Event Type:** `error`

**Data Format:**
```json
{
  "type": "error",
  "message": "Model API request failed: rate limit exceeded"
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"error"` |
| `message` | string | Human-readable error description |

**Example:**
```
id: 20
event: error
data: {"type":"error","message":"Model API request failed: rate limit exceeded"}

```

The connection may close after an error event, depending on error severity.

---

### done

The agent turn completed successfully.

**Event Type:** `done`

**Data Format:**
```json
{
  "type": "done",
  "usage": {
    "input": 150,
    "output": 45,
    "cache_read": 10
  }
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"done"` |
| `usage` | object | Token usage for this turn |

**Usage Object:**

| Field | Type | Description |
|-------|------|-------------|
| `input` | u32 | Input tokens consumed |
| `output` | u32 | Output tokens generated |
| `cache_read` | u32 | Tokens read from cache |

**Example:**
```
id: 21
event: done
data: {"type":"done","usage":{"input":150,"output":45,"cache_read":10}}

```

This is typically the final event in a turn. After this event, the connection may close or remain open for subsequent turns.

---

### approval_required

The agent is waiting for user approval before taking an action.

**Event Type:** `approval_required`

**Data Format:**
```json
{
  "type": "approval_required",
  "action": "delete_file",
  "description": "User is requesting to delete /important/data.txt. Approve?"
}
```

**Payload Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"approval_required"` |
| `action` | string | Action requiring approval |
| `description` | string | Human-readable description of what needs approval |

**Example:**
```
id: 15
event: approval_required
data: {"type":"approval_required","action":"delete_file","description":"User is requesting to delete /important/data.txt. Approve?"}

```

The client should pause execution and prompt the user. The agent will wait for an approval confirmation before proceeding (via a separate HTTP endpoint).

---

## Event Stream Example

A typical agent turn might produce the following event stream:

```
id: 1
event: token
data: {"type":"token","text":"I'll"}

id: 2
event: token
data: {"type":"token","text":" help"}

id: 3
event: token
data: {"type":"token","text":" you"}

id: 4
event: tool_call_start
data: {"type":"tool_call_start","id":"call_1","name":"list_files"}

id: 5
event: tool_call_end
data: {"type":"tool_call_end","id":"call_1","result":{"files":["a.txt","b.txt"]},"is_error":false}

id: 6
event: token
data: {"type":"token","text":" found"}

id: 7
event: token
data: {"type":"token","text":" 2 files"}

id: 8
event: done
data: {"type":"done","usage":{"input":200,"output":50,"cache_read":0}}

```

## Client Implementation Notes

- Process each event independently as it arrives
- Event IDs can be used to resume a stream from a specific point if the connection drops
- The `data` field is always a complete JSON object (never multi-line)
- Connections may close unexpectedly; clients should implement reconnection logic
- Token events may contain partial words; buffer tokens until whitespace or tool calls for complete text assembly
