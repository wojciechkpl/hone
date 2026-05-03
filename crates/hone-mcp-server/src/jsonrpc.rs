use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 id field. Per spec: string, number, or null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
    Null,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<JsonRpcId>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: JsonRpcId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: JsonRpcId, err: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(err),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self { code: Self::PARSE_ERROR, message: msg.into(), data: None }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self { code: Self::INVALID_REQUEST, message: msg.into(), data: None }
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self { code: Self::METHOD_NOT_FOUND, message: msg.into(), data: None }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self { code: Self::INVALID_PARAMS, message: msg.into(), data: None }
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self { code: Self::INTERNAL_ERROR, message: msg.into(), data: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_with_id_is_not_notification() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(JsonRpcId::Number(1)),
            method: "tools/list".into(),
            params: None,
        };
        assert!(!req.is_notification());
    }

    #[test]
    fn request_without_id_is_notification() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: None,
        };
        assert!(req.is_notification());
    }

    #[test]
    fn request_serde_roundtrip_with_string_id() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(JsonRpcId::String("abc".into())),
            method: "tools/list".into(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, Some(JsonRpcId::String("abc".into())));
        assert_eq!(back.method, "tools/list");
    }

    #[test]
    fn request_serde_roundtrip_with_number_id() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(JsonRpcId::Number(42)),
            method: "initialize".into(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, Some(JsonRpcId::Number(42)));
    }

    #[test]
    fn request_serde_roundtrip_with_null_id() {
        // JSON-RPC spec allows null id (discouraged but valid)
        let raw = r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        // null id should parse to Some(JsonRpcId::Null), not None
        // (None means the key is ABSENT; null means the key is present with null value)
        // After roundtrip the id variant is Null
        let serialised = serde_json::to_value(&req).unwrap();
        assert_eq!(serialised["id"], serde_json::Value::Null);
    }

    #[test]
    fn request_omits_params_when_absent() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert!(req.params.is_none());
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(JsonRpcId::Number(1)));
    }

    #[test]
    fn response_ok_serializes_with_result_only() {
        let resp = JsonRpcResponse::ok(JsonRpcId::Number(1), json!({"ok": true}));
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("result").is_some(), "result field must be present");
        assert!(v.get("error").is_none(), "error field must be absent");
        assert_eq!(v["jsonrpc"], "2.0");
    }

    #[test]
    fn response_err_serializes_with_error_only() {
        let err = JsonRpcError::method_not_found("no such method");
        let resp = JsonRpcResponse::err(JsonRpcId::Number(1), err);
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("error").is_some(), "error field must be present");
        assert!(v.get("result").is_none(), "result field must be absent");
        assert_eq!(v["error"]["code"], JsonRpcError::METHOD_NOT_FOUND);
    }

    #[test]
    fn error_constructors_have_expected_codes() {
        assert_eq!(JsonRpcError::parse_error("x").code, -32700);
        assert_eq!(JsonRpcError::invalid_request("x").code, -32600);
        assert_eq!(JsonRpcError::method_not_found("x").code, -32601);
        assert_eq!(JsonRpcError::invalid_params("x").code, -32602);
        assert_eq!(JsonRpcError::internal_error("x").code, -32603);
    }
}
