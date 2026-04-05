# HTTP API Reference

The `honed` daemon exposes a REST API for session management, team governance, collaboration, and tunneling. All endpoints require Bearer token authentication (if configured).

## Authentication

Requests must include a Bearer token in the Authorization header:

```
Authorization: Bearer <token>
```

If no auth token is configured on the server, this header may be omitted.

### Error Response

All endpoints return error responses with the following format:

```json
{
  "error": "error description"
}
```

## Core Endpoints

### GET /health

Health check endpoint.

**Request:**
```bash
curl http://localhost:8080/health
```

**Response:** `200 OK`
```json
{
  "status": "ok"
}
```

---

### POST /sessions

Create a new session.

**Request:**
```bash
curl -X POST http://localhost:8080/sessions \
  -H "Authorization: Bearer <token>"
```

**Response:** `201 Created`
```json
{
  "session_id": "sess_abc123def456"
}
```

**Return Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | Unique session identifier |

---

### GET /sessions/{id}

Retrieve session information by ID.

**Request:**
```bash
curl http://localhost:8080/sessions/sess_abc123def456 \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK`
```json
{
  "id": "sess_abc123def456",
  "created_at": 1704067200
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Session identifier |
| `created_at` | u64 | Unix timestamp of session creation |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `404 Not Found` | Session not found | Session with given ID does not exist |
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### DELETE /sessions/{id}

Delete a session.

**Request:**
```bash
curl -X DELETE http://localhost:8080/sessions/sess_abc123def456 \
  -H "Authorization: Bearer <token>"
```

**Response:** `204 No Content`

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `404 Not Found` | Session not found | Session with given ID does not exist |
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

## Collaboration Endpoints

### POST /sessions/{id}/join

Add a participant to a session.

**Request:**
```bash
curl -X POST http://localhost:8080/sessions/sess_abc123def456/join \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": "user_123",
    "name": "Alice"
  }'
```

**Request Body:**

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | Unique user identifier |
| `name` | string | Display name for the participant |

**Response:** `200 OK`
```json
{
  "user_id": "user_123",
  "name": "Alice",
  "role": "owner",
  "joined_at": 1704067200
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | User identifier |
| `name` | string | Display name |
| `role` | string | Participant role: `owner` (first joiner) or `editor` |
| `joined_at` | u64 | Unix timestamp of when user joined |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `409 Conflict` | Already joined | User already a participant in this session |
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### POST /sessions/{id}/leave

Remove a participant from a session.

**Request:**
```bash
curl -X POST http://localhost:8080/sessions/sess_abc123def456/leave \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": "user_123"
  }'
```

**Request Body:**

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | User to remove |

**Response:** `204 No Content`

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `404 Not Found` | Not a participant | User is not in this session |
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### GET /sessions/{id}/participants

List all participants in a session.

**Request:**
```bash
curl http://localhost:8080/sessions/sess_abc123def456/participants \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK`
```json
[
  {
    "user_id": "user_123",
    "name": "Alice",
    "role": "owner",
    "joined_at": 1704067200
  },
  {
    "user_id": "user_456",
    "name": "Bob",
    "role": "editor",
    "joined_at": 1704067201
  }
]
```

**Response:** Array of participant objects with fields:

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | User identifier |
| `name` | string | Display name |
| `role` | string | Participant role: `owner` or `editor` |
| `joined_at` | u64 | Unix timestamp of join time |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### POST /sessions/{id}/kick

Remove a participant from a session (owner only).

**Request:**
```bash
curl -X POST http://localhost:8080/sessions/sess_abc123def456/kick \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "owner_user_id": "user_123",
    "target_user_id": "user_456"
  }'
```

**Request Body:**

| Field | Type | Description |
|-------|------|-------------|
| `owner_user_id` | string | User performing the kick (must be owner) |
| `target_user_id` | string | User to remove |

**Response:** `204 No Content`

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `403 Forbidden` | Forbidden | Caller is not the session owner |
| `404 Not Found` | Not a participant | Target user is not in this session |
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

## Team Governance Endpoints

### GET /team/usage

Retrieve team token usage and cost metrics.

**Request:**
```bash
curl http://localhost:8080/team/usage \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK`
```json
{
  "total_tokens": 50000,
  "total_cost_usd": 12.50,
  "by_model": {
    "claude-opus-4-5": {
      "tokens": 30000,
      "cost_usd": 10.00
    },
    "claude-haiku-3-5": {
      "tokens": 20000,
      "cost_usd": 2.50
    }
  }
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `total_tokens` | u64 | Total tokens consumed by team |
| `total_cost_usd` | f64 | Total cost in USD |
| `by_model` | object | Usage breakdown by model ID |

**Model Usage Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `tokens` | u64 | Tokens for this model |
| `cost_usd` | f64 | Cost for this model in USD |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### GET /team/members

List all team members.

**Request:**
```bash
curl http://localhost:8080/team/members \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK`
```json
[
  {
    "id": "member_123",
    "name": "Alice",
    "role": "admin",
    "joined_at": 1704067200
  },
  {
    "id": "member_456",
    "name": "Bob",
    "role": "user",
    "joined_at": 1704067201
  }
]
```

**Response:** Array of team member objects with fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Member identifier |
| `name` | string | Display name |
| `role` | string | Team role (e.g., `admin`, `user`) |
| `joined_at` | u64 | Unix timestamp of when member joined team |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### GET /team/policy

Retrieve team governance policy.

**Request:**
```bash
curl http://localhost:8080/team/policy \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK`
```json
{
  "allowed_models": [
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-haiku-3-5"
  ],
  "spend_cap_usd": 1000.00,
  "approved_extensions": [
    "ext_mcp_filesystem",
    "ext_mcp_database"
  ]
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `allowed_models` | array | List of model IDs allowed for team |
| `spend_cap_usd` | number \| null | Monthly spend limit in USD, or null for unlimited |
| `approved_extensions` | array | List of approved extension identifiers |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

### POST /team/policy

Update team governance policy.

**Request:**
```bash
curl -X POST http://localhost:8080/team/policy \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "allowed_models": ["claude-opus-4-5", "claude-haiku-3-5"],
    "spend_cap_usd": 500.00,
    "approved_extensions": ["ext_mcp_filesystem"]
  }'
```

**Request Body:**

| Field | Type | Description |
|-------|------|-------------|
| `allowed_models` | array | New list of allowed model IDs |
| `spend_cap_usd` | number \| null | New spend cap, or null for unlimited |
| `approved_extensions` | array | New list of approved extension IDs |

**Response:** `200 OK` (returns updated policy)
```json
{
  "allowed_models": ["claude-opus-4-5", "claude-haiku-3-5"],
  "spend_cap_usd": 500.00,
  "approved_extensions": ["ext_mcp_filesystem"]
}
```

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |

---

## Tunnel Endpoints

### POST /tunnel

Start a reverse tunnel to expose a local port.

**Request:**
```bash
curl -X POST http://localhost:8080/tunnel \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "local_port": 3000
  }'
```

**Request Body:**

| Field | Type | Description |
|-------|------|-------------|
| `local_port` | u16 | Local port to expose via tunnel |

**Response:** `201 Created`
```json
{
  "url": "https://tunnel-abc123.hone.sh",
  "local_port": 3000,
  "is_active": true
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | Public tunnel URL |
| `local_port` | u16 | Exposed local port |
| `is_active` | bool | Whether tunnel is currently active |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |
| `500 Internal Server Error` | Internal error | Failed to create tunnel |

---

### GET /tunnel

Get current tunnel status.

**Request:**
```bash
curl http://localhost:8080/tunnel \
  -H "Authorization: Bearer <token>"
```

**Response:** `200 OK` (if tunnel active)
```json
{
  "url": "https://tunnel-abc123.hone.sh",
  "local_port": 3000,
  "is_active": true
}
```

**Response:** `200 OK` (if no tunnel)
```json
{
  "is_active": false
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | Public tunnel URL (if active) |
| `local_port` | u16 | Exposed port (if active) |
| `is_active` | bool | Whether tunnel is currently active |

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |
| `500 Internal Server Error` | Internal error | Failed to check tunnel status |

---

### DELETE /tunnel

Stop the active tunnel.

**Request:**
```bash
curl -X DELETE http://localhost:8080/tunnel \
  -H "Authorization: Bearer <token>"
```

**Response:** `204 No Content`

**Errors:**

| Status | Error | Description |
|--------|-------|-------------|
| `401 Unauthorized` | Unauthorized | Invalid or missing authentication token |
| `500 Internal Server Error` | Internal error | Failed to stop tunnel |

---

## HTTP Status Codes

Standard HTTP status codes are used throughout:

| Code | Meaning |
|------|---------|
| `200 OK` | Request succeeded |
| `201 Created` | Resource created successfully |
| `204 No Content` | Request succeeded, no response body |
| `400 Bad Request` | Invalid request format or parameters |
| `401 Unauthorized` | Missing or invalid authentication token |
| `403 Forbidden` | Authenticated but lacking required permissions |
| `404 Not Found` | Resource does not exist |
| `409 Conflict` | Request conflicts with current state (e.g., duplicate join) |
| `500 Internal Server Error` | Server error during processing |

---

## Rate Limiting

Rate limiting is not currently enforced. Future versions may implement per-token or per-team rate limits.
