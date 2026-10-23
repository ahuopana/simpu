//! Plugin protocol. A plugin is an executable that reads one JSON-RPC 2.0
//! request per line on stdin and writes one response per line on stdout.
//! stderr is free for logs (never log secrets). See ADR 0002.
//!
//! Methods:
//! - `initialize` -> [`Info`]
//! - `credential.get` with [`GetParams`] -> [`GetResult`]
//! - `vault.unlock` with [`UnlockParams`] -> `{}` (optional; capability `unlock`)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_CREDENTIAL_GET: &str = "credential.get";
pub const METHOD_VAULT_UNLOCK: &str = "vault.unlock";

pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_LOCKED: i64 = -32001;
pub const ERR_BAD_PASSWORD: i64 = -32003;
pub const ERR_NOT_FOUND: i64 = -32002;
pub const ERR_OTHER: i64 = -32000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Info {
    /// Provider id referenced from connection secrets, e.g. `bitwarden`.
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetParams {
    pub item: String,
    /// `password` or `keyfile`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetResult {
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnlockParams {
    pub password: String,
}

/// Implemented by each plugin.
pub trait Plugin {
    fn info(&self) -> Info;
    fn credential_get(&self, params: GetParams) -> Result<GetResult, RpcError>;
    /// Unlock the vault with the master password. Plugins without a vault keep the default.
    fn unlock(&self, _password: &str) -> Result<(), RpcError> {
        Err(RpcError::new(ERR_METHOD_NOT_FOUND, "unlock not supported"))
    }
}

fn handle(plugin: &dyn Plugin, req: Request) -> Response {
    let outcome: Result<Value, RpcError> = match req.method.as_str() {
        METHOD_INITIALIZE => {
            serde_json::to_value(plugin.info()).map_err(|e| RpcError::new(ERR_OTHER, e.to_string()))
        }
        METHOD_CREDENTIAL_GET => serde_json::from_value::<GetParams>(req.params)
            .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, e.to_string()))
            .and_then(|p| plugin.credential_get(p))
            .and_then(|r| {
                serde_json::to_value(r).map_err(|e| RpcError::new(ERR_OTHER, e.to_string()))
            }),
        METHOD_VAULT_UNLOCK => serde_json::from_value::<UnlockParams>(req.params)
            .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, e.to_string()))
            .and_then(|p| plugin.unlock(&p.password))
            .map(|()| serde_json::json!({})),
        other => Err(RpcError::new(
            ERR_METHOD_NOT_FOUND,
            format!("unknown method {other}"),
        )),
    };
    match outcome {
        Ok(v) => Response {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(v),
            error: None,
        },
        Err(e) => Response {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: None,
            error: Some(e),
        },
    }
}

/// Serve requests until `input` reaches EOF.
pub fn serve<R: BufRead, W: Write>(
    plugin: &dyn Plugin,
    input: R,
    mut output: W,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(plugin, req),
            Err(e) => Response {
                jsonrpc: "2.0".into(),
                id: 0,
                result: None,
                error: Some(RpcError::new(
                    ERR_INVALID_PARAMS,
                    format!("bad request: {e}"),
                )),
            },
        };
        writeln!(
            output,
            "{}",
            serde_json::to_string(&resp).map_err(std::io::Error::other)?
        )?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Plugin for Echo {
        fn info(&self) -> Info {
            Info {
                id: "echo".into(),
                name: "Echo".into(),
                version: "0".into(),
                protocol: PROTOCOL_VERSION,
                capabilities: vec!["credentials".into()],
            }
        }
        fn credential_get(&self, p: GetParams) -> Result<GetResult, RpcError> {
            if p.item == "locked" {
                Err(RpcError::new(ERR_LOCKED, "locked"))
            } else {
                Ok(GetResult {
                    secret: format!("secret-of-{}", p.item),
                })
            }
        }
    }

    fn run(input: &str) -> Vec<Response> {
        let mut out = Vec::new();
        serve(&Echo, input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_and_get() {
        let r = run("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"credential.get\",\"params\":{\"item\":\"a\",\"kind\":\"password\"}}\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].result.as_ref().unwrap()["id"], "echo");
        assert_eq!(r[1].result.as_ref().unwrap()["secret"], "secret-of-a");
    }

    #[test]
    fn errors_are_reported() {
        let r = run("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"credential.get\",\"params\":{\"item\":\"locked\",\"kind\":\"password\"}}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"nope\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"credential.get\",\"params\":{}}\n\
                     garbage\n");
        assert_eq!(r[0].error.as_ref().unwrap().code, ERR_LOCKED);
        assert_eq!(r[1].error.as_ref().unwrap().code, ERR_METHOD_NOT_FOUND);
        assert_eq!(r[2].error.as_ref().unwrap().code, ERR_INVALID_PARAMS);
        assert_eq!(r[3].error.as_ref().unwrap().code, ERR_INVALID_PARAMS);
    }
}
