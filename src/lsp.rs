//! epycell-lsp — a minimal LSP server that bridges editor completions/hover
//! to a running Jupyter kernel via its ZMQ shell channel.
//!
//! Usage: epycell-lsp <connection-file.json>
//!
//! Speaks LSP over stdin/stdout. The kernel must already be running.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use jupyter_protocol::{
    CompleteRequest, ConnectionInfo, InspectRequest, JupyterMessage, JupyterMessageContent,
};
use runtimelib::{
    create_client_shell_connection_with_identity, peer_identity_for_session,
    ClientShellConnection,
};
use serde_json::{json, Value};

struct LspState {
    shell: ClientShellConnection,
    doc_content: String,
    doc_uri: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        anyhow::bail!("usage: epycell-lsp <connection-file.json>");
    }
    let conn_path = PathBuf::from(&args[1]);
    let info: ConnectionInfo =
        serde_json::from_str(&std::fs::read_to_string(&conn_path)?)?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let identity = peer_identity_for_session(&session_id)?;
    let shell =
        create_client_shell_connection_with_identity(&info, &session_id, identity).await?;

    let mut state = LspState {
        shell,
        doc_content: String::new(),
        doc_uri: String::new(),
    };

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        let msg = match read_lsp_message(&mut reader) {
            Ok(Some(msg)) => msg,
            Ok(None) => break,
            Err(_) => break,
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                let resp = json!({
                    "capabilities": {
                        "completionProvider": {
                            "triggerCharacters": [".", "(", "[", ",", " "],
                            "resolveProvider": false
                        },
                        "hoverProvider": true,
                        "textDocumentSync": {
                            "openClose": true,
                            "change": 1
                        }
                    },
                    "serverInfo": {
                        "name": "epycell-lsp",
                        "version": "0.1.0"
                    }
                });
                send_response(id, resp)?;
            }
            "initialized" => {}
            "shutdown" => {
                send_response(id, json!(null))?;
            }
            "exit" => break,

            "textDocument/didOpen" => {
                if let Some(params) = msg.get("params") {
                    if let Some(doc) = params.get("textDocument") {
                        state.doc_content = doc.get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        state.doc_uri = doc.get("uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                }
            }
            "textDocument/didChange" => {
                if let Some(params) = msg.get("params") {
                    if let Some(changes) = params.get("contentChanges") {
                        if let Some(change) = changes.as_array().and_then(|a| a.first()) {
                            if let Some(text) = change.get("text").and_then(|t| t.as_str()) {
                                state.doc_content = text.to_string();
                            }
                        }
                    }
                }
            }
            "textDocument/didClose" => {
                state.doc_content.clear();
                state.doc_uri.clear();
            }

            "textDocument/completion" => {
                let result = handle_completion(&mut state, &msg).await
                    .unwrap_or_else(|_| json!([]));
                send_response(id, result)?;
            }
            "textDocument/hover" => {
                let result = handle_hover(&mut state, &msg).await
                    .unwrap_or_else(|_| json!(null));
                send_response(id, result)?;
            }
            _ => {
                if id.is_some() {
                    send_error(id, -32601, "method not found")?;
                }
            }
        }
    }
    Ok(())
}

async fn handle_completion(state: &mut LspState, msg: &Value) -> Result<Value> {
    let params = msg.get("params").context("no params")?;
    let position = params.get("position").context("no position")?;
    let line = position.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
    let character = position.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as usize;

    let cursor_pos = offset_from_position(&state.doc_content, line, character);

    let req = CompleteRequest {
        code: state.doc_content.clone(),
        cursor_pos,
    };
    let msg = JupyterMessage::new(JupyterMessageContent::CompleteRequest(req), None);
    let msg_id = msg.header.msg_id.clone();
    state.shell.send(msg).await?;

    let reply = tokio::time::timeout(Duration::from_secs(5), state.shell.read())
        .await
        .context("timeout waiting for complete_reply")??;

    if reply.header.msg_id != msg_id {
        if let Some(parent) = &reply.parent_header {
            if parent.msg_id != msg_id {
                return Ok(json!([]));
            }
        }
    }

    match reply.content {
        JupyterMessageContent::CompleteReply(r) => {
            let items: Vec<Value> = r.matches.iter().map(|m| {
                json!({
                    "label": m,
                    "kind": 6
                })
            }).collect();
            Ok(json!(items))
        }
        _ => Ok(json!([])),
    }
}

async fn handle_hover(state: &mut LspState, msg: &Value) -> Result<Value> {
    let params = msg.get("params").context("no params")?;
    let position = params.get("position").context("no position")?;
    let line = position.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
    let character = position.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as usize;

    let cursor_pos = offset_from_position(&state.doc_content, line, character);

    let req = InspectRequest {
        code: state.doc_content.clone(),
        cursor_pos,
        detail_level: Some(0),
    };
    let msg = JupyterMessage::new(JupyterMessageContent::InspectRequest(req), None);
    let msg_id = msg.header.msg_id.clone();
    state.shell.send(msg).await?;

    let reply = tokio::time::timeout(Duration::from_secs(5), state.shell.read())
        .await
        .context("timeout waiting for inspect_reply")??;

    if reply.header.msg_id != msg_id {
        if let Some(parent) = &reply.parent_header {
            if parent.msg_id != msg_id {
                return Ok(json!(null));
            }
        }
    }

    match reply.content {
        JupyterMessageContent::InspectReply(r) => {
            if !r.found {
                return Ok(json!(null));
            }
            let text = r.data.content.iter().find_map(|mt| {
                match mt {
                    jupyter_protocol::MediaType::Plain(t) => Some(t.clone()),
                    _ => None,
                }
            }).unwrap_or_default();

            if text.is_empty() {
                return Ok(json!(null));
            }
            Ok(json!({
                "contents": {
                    "kind": "plaintext",
                    "value": text
                }
            }))
        }
        _ => Ok(json!(null)),
    }
}

fn offset_from_position(text: &str, line: usize, character: usize) -> usize {
    let mut offset = 0;
    for (i, l) in text.lines().enumerate() {
        if i == line {
            return offset + character.min(l.len());
        }
        offset += l.len() + 1; // +1 for \n
    }
    offset
}

fn read_lsp_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length: usize = 0;
    loop {
        let mut header_line = String::new();
        let bytes_read = reader.read_line(&mut header_line)?;
        if bytes_read == 0 {
            return Ok(None);
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
            content_length = len_str.trim().parse().context("bad content-length")?;
        }
    }
    if content_length == 0 {
        return Ok(None);
    }
    let mut body = vec![0u8; content_length];
    Read::read_exact(reader, &mut body)?;
    let msg: Value = serde_json::from_slice(&body)?;
    Ok(Some(msg))
}

fn send_response(id: Option<Value>, result: Value) -> Result<()> {
    let resp = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    });
    write_lsp_message(&resp)
}

fn send_error(id: Option<Value>, code: i64, message: &str) -> Result<()> {
    let resp = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    });
    write_lsp_message(&resp)
}

fn write_lsp_message(msg: &Value) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    out.flush()?;
    Ok(())
}

