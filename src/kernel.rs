//! Kernel session: launch an IPython kernel and run cells over ZMQ.
//!
//! This is the plumbing proven by the milestone-0 spike, lifted into a module
//! so the notebook UI can drive it.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use jupyter_protocol::{
    ConnectionInfo, ExecuteRequest, ExecutionState, JupyterMessage, JupyterMessageContent,
    Media, MediaType, Transport,
};
use runtimelib::{
    create_client_iopub_connection, create_client_shell_connection_with_identity,
    peer_identity_for_session, peek_ports_with_listeners, wait_for_iopub_welcome,
    ClientIoPubConnection, ClientShellConnection,
};
use tokio::process::{Child, Command};

/// One piece of output produced by running a cell.
#[derive(Debug, Clone)]
pub enum Output {
    /// stdout/stderr stream text.
    Stream { name: String, text: String },
    /// text/plain result or display data.
    Text(String),
    /// Decoded PNG bytes (e.g. a matplotlib figure).
    Png(Vec<u8>),
    /// An exception.
    Error {
        ename: String,
        evalue: String,
        traceback: Vec<String>,
    },
}

/// Event from a running cell: either an output chunk or an idle signal.
#[derive(Debug)]
pub enum CellEvent {
    Output(Output),
    Idle,
}

/// A launched kernel plus its client connections.
pub struct KernelSession {
    child: Child,
    shell: ClientShellConnection,
    iopub: ClientIoPubConnection,
    _conn_file: tempfile::NamedTempFile,
}

impl KernelSession {
    /// Path to the connection file for this kernel (needed by epycell-lsp).
    pub fn connection_file(&self) -> &std::path::Path {
        self._conn_file.path()
    }

    /// Launch an ipykernel using the given python interpreter and connect to it.
    pub async fn launch(python: &std::path::Path) -> Result<Self> {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        // Reserve 5 free ports, keeping the listeners alive until *after* the
        // connection file is written, to dodge the documented TOCTOU race.
        let (ports, listeners) = peek_ports_with_listeners(ip, 5)
            .await
            .context("finding free ports")?;
        let key = uuid::Uuid::new_v4().to_string();
        let session_id = uuid::Uuid::new_v4().to_string();

        let info = ConnectionInfo {
            ip: ip.to_string(),
            transport: Transport::TCP,
            shell_port: ports[0],
            iopub_port: ports[1],
            stdin_port: ports[2],
            control_port: ports[3],
            hb_port: ports[4],
            key: key.clone(),
            signature_scheme: "hmac-sha256".to_string(),
            kernel_name: Some("python3".to_string()),
        };

        let conn_file = tempfile::Builder::new()
            .prefix("epycell-conn-")
            .suffix(".json")
            .tempfile()
            .context("creating connection file")?;
        std::fs::write(conn_file.path(), serde_json::to_vec_pretty(&info)?)
            .context("writing connection file")?;

        drop(listeners);

        let child = Command::new(python)
            .arg("-m")
            .arg("ipykernel_launcher")
            .arg("-f")
            .arg(conn_file.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawning ipykernel (is the uv venv set up?)")?;

        let identity = peer_identity_for_session(&session_id)?;
        let mut iopub = create_client_iopub_connection(&info, "", &session_id).await?;
        let shell =
            create_client_shell_connection_with_identity(&info, &session_id, identity).await?;

        let _ = wait_for_iopub_welcome(&mut iopub, Duration::from_secs(5)).await;

        let mut session = Self {
            child,
            shell,
            iopub,
            _conn_file: conn_file,
        };

        // Enable inline plotting so figures auto-display at the end of every
        // cell (via the inline backend's flush_figures post-execute hook),
        // not only when the figure happens to be the cell's last expression.
        let _ = session
            .run_cell(
                "%matplotlib inline\n\
                 from matplotlib_inline.backend_inline import set_matplotlib_formats\n\
                 set_matplotlib_formats('png')",
            )
            .await;

        Ok(session)
    }

    /// Run one cell; collect its outputs until the kernel goes idle for it.
    /// (Blocking — prefer start_cell + poll_output for async use.)
    pub async fn run_cell(&mut self, code: &str) -> Result<Vec<Output>> {
        let msg_id = self.start_cell(code).await?;
        let mut outputs = Vec::new();
        loop {
            match self.poll_output(&msg_id).await? {
                Some(CellEvent::Output(o)) => outputs.push(o),
                Some(CellEvent::Idle) => break,
                None => {}
            }
        }
        Ok(outputs)
    }

    /// Send an execute_request without waiting for outputs. Returns the msg_id
    /// to pass to poll_output.
    pub async fn start_cell(&mut self, code: &str) -> Result<String> {
        let msg = JupyterMessage::new(ExecuteRequest::new(code.to_string()), None);
        let msg_id = msg.header.msg_id.clone();
        self.shell
            .send(msg)
            .await
            .context("sending execute_request")?;
        Ok(msg_id)
    }

    /// Non-blocking poll for the next output belonging to `msg_id`.
    /// Returns None if nothing available yet, Some(Idle) when execution finishes.
    pub async fn poll_output(&mut self, msg_id: &str) -> Result<Option<CellEvent>> {
        let reply = match tokio::time::timeout(Duration::from_millis(10), self.iopub.read()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Ok(None), // timeout = nothing yet
        };

        let is_ours = reply
            .parent_header
            .as_ref()
            .map(|h| h.msg_id == msg_id)
            .unwrap_or(false);
        if !is_ours {
            return Ok(None);
        }

        match reply.content {
            JupyterMessageContent::StreamContent(s) => Ok(Some(CellEvent::Output(Output::Stream {
                name: format!("{:?}", s.name),
                text: s.text,
            }))),
            JupyterMessageContent::ExecuteResult(r) => {
                let mut outputs = Vec::new();
                collect_media(&r.data, &mut outputs);
                Ok(outputs.into_iter().next().map(CellEvent::Output))
            }
            JupyterMessageContent::DisplayData(d) => {
                let mut outputs = Vec::new();
                collect_media(&d.data, &mut outputs);
                Ok(outputs.into_iter().next().map(CellEvent::Output))
            }
            JupyterMessageContent::ErrorOutput(e) => Ok(Some(CellEvent::Output(Output::Error {
                ename: e.ename,
                evalue: e.evalue,
                traceback: e.traceback,
            }))),
            JupyterMessageContent::Status(st) => {
                if matches!(st.execution_state, ExecutionState::Idle) {
                    Ok(Some(CellEvent::Idle))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Send SIGINT to the kernel process to interrupt the current execution.
    pub fn interrupt(&self) {
        if let Some(pid) = self.child.id() {
            unsafe { libc::kill(pid as i32, libc::SIGINT); }
        }
    }

    pub async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

/// Pull the richest representation out of a Media bundle. A figure arrives as
/// both `image/png` and a `text/plain` "<Figure ...>" placeholder — prefer the
/// image and drop the redundant text.
fn collect_media(media: &Media, outputs: &mut Vec<Output>) {
    let has_png = media
        .content
        .iter()
        .any(|m| matches!(m, MediaType::Png(_)));
    for item in &media.content {
        match item {
            MediaType::Png(b64) => {
                match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                    Ok(bytes) => outputs.push(Output::Png(bytes)),
                    Err(e) => outputs.push(Output::Text(format!("<png decode error: {e}>"))),
                }
            }
            MediaType::Plain(t) if !has_png => outputs.push(Output::Text(t.clone())),
            _ => {}
        }
    }
}

/// Kernel python: EPYCELL_PYTHON env → VIRTUAL_ENV/bin/python → ~/.epycell/.venv fallback.
pub fn default_kernel_python() -> PathBuf {
    if let Ok(p) = std::env::var("EPYCELL_PYTHON") {
        return PathBuf::from(p);
    }
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        return PathBuf::from(venv).join("bin/python");
    }
    PathBuf::from(std::env::var("HOME").expect("HOME unset")).join("epycell/.venv/bin/python")
}
