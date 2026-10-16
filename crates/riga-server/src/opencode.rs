//! Supervises the RIGA OpenCode Responses adapter.
//!
//! RIGA's `opencode_open_ai_compatible` provider speaks the OpenAI **Responses
//! API**. That API is served by the standalone `@rigai/opencode` adapter, which
//! in turn launches `opencode serve` from `PATH`. This module owns the adapter's
//! lifecycle: it starts it on demand (when such a provider is configured, or its
//! models are listed), waits for `/health`, and returns an actionable error the
//! UI shows as a toast when it cannot start.
//!
//! ## Ports
//!
//! The adapter defaults to [`DEFAULT_PORT`] (8791), deliberately clear of the
//! other RIGA processes: `riga-server` binds 8787 and `riga-mcp` binds 8788.
//! `RIGA_OPENCODE_PORT` overrides it. If the port is taken, the adapter exits
//! non-zero and its message is surfaced here.
//!
//! ## Resolution
//!
//! No `npm` is required: the adapter is looked up as an executable —
//! `RIGA_OPENCODE_BIN` → `riga-opencode` on `PATH`; or, for a source checkout,
//! `RIGA_OPENCODE_ADAPTER` points at a `.js` entrypoint run with `node`.

use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Adapter port: distinct from riga-server (8787) and riga-mcp (8788).
pub const DEFAULT_PORT: u16 = 8791;
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// One process-wide adapter supervisor.
pub struct OpenCodeService {
    running: Mutex<Option<Running>>,
}

struct Running {
    child: Child,
}

static SERVICE: OnceLock<OpenCodeService> = OnceLock::new();

/// The process-wide adapter supervisor.
pub fn service() -> &'static OpenCodeService {
    SERVICE.get_or_init(|| OpenCodeService {
        running: Mutex::new(None),
    })
}

fn port() -> u16 {
    std::env::var("RIGA_OPENCODE_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PORT)
}

fn endpoint_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .any(|dir| dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file())
}

/// Resolve the adapter command line, or an actionable error the UI can toast.
fn resolve_command() -> Result<(String, Vec<String>), String> {
    if let Ok(bin) = std::env::var("RIGA_OPENCODE_BIN")
        && !bin.trim().is_empty()
    {
        return Ok((bin, Vec::new()));
    }
    if let Ok(script) = std::env::var("RIGA_OPENCODE_ADAPTER")
        && !script.trim().is_empty()
    {
        return Ok(("node".to_string(), vec![script]));
    }
    if on_path("riga-opencode") {
        return Ok(("riga-opencode".to_string(), Vec::new()));
    }
    Err(
        "The RIGA OpenCode adapter was not found. Install `riga-opencode` on PATH, \
         or set RIGA_OPENCODE_BIN to the adapter executable (RIGA_OPENCODE_ADAPTER \
         accepts a .js entrypoint run with node)."
            .to_string(),
    )
}

async fn healthy(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    matches!(client.get(url).timeout(Duration::from_secs(2)).send().await, Ok(response) if response.status().is_success())
}

impl OpenCodeService {
    /// Ensure the adapter is running and return its Responses API endpoint.
    ///
    /// Idempotent: a healthy adapter is reused (including one started outside
    /// RIGA). Any failure to spawn or become healthy returns a `String` error
    /// (with the adapter's own output) that the provider-configure path forwards
    /// to the UI toast.
    pub async fn ensure(&self) -> Result<String, String> {
        let port = port();
        let endpoint = endpoint_for(port);
        let client = reqwest::Client::new();

        // Reap an adapter we own that has already exited.
        {
            let mut guard = self.running.lock().await;
            if let Some(running) = guard.as_mut()
                && matches!(running.child.try_wait(), Ok(Some(_)))
            {
                *guard = None;
            }
        }
        // Reuse anything already serving the port.
        if healthy(&client, port).await {
            return Ok(endpoint);
        }

        let (program, args) = resolve_command()?;
        let mut child = Command::new(&program)
            .args(&args)
            .env("RIGA_OPENCODE_HOST", "127.0.0.1")
            .env("RIGA_OPENCODE_PORT", port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start the RIGA OpenCode adapter ({program}): {error}"))?;

        // Keep a bounded tail of the adapter's output for the error message.
        let output = Arc::new(Mutex::new(String::new()));
        if let Some(stdout) = child.stdout.take() {
            drain(stdout, output.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            drain(stderr, output.clone());
        }

        // Replace any previously owned child with this one.
        {
            let mut guard = self.running.lock().await;
            if let Some(mut previous) = guard.take() {
                let _ = previous.child.kill().await;
            }
            *guard = Some(Running {
                child,
            });
        }

        // Wait for `/health`.
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if healthy(&client, port).await {
                return Ok(endpoint);
            }
            {
                let mut guard = self.running.lock().await;
                if let Some(running) = guard.as_mut()
                    && let Ok(Some(status)) = running.child.try_wait()
                {
                    *guard = None;
                    let tail = output.lock().await.clone();
                    return Err(format!(
                        "The RIGA OpenCode adapter exited ({status}) before becoming ready.{}",
                        tail_suffix(&tail)
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let mut guard = self.running.lock().await;
                if let Some(mut running) = guard.take() {
                    let _ = running.child.kill().await;
                }
                let tail = output.lock().await.clone();
                return Err(format!(
                    "The RIGA OpenCode adapter did not become ready within {}s. Is port {port} free (riga-mcp uses 8788)?{}",
                    START_TIMEOUT.as_secs(),
                    tail_suffix(&tail)
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// Ensure the adapter is running for an `opencode_open_ai_compatible` provider.
///
/// Returns the endpoint to use when the configured one is blank or loopback
/// (starting the adapter as a side effect), or `None` when the provider points
/// at a non-local endpoint that RIGA must not manage.
pub async fn ensure_local_endpoint(configured_endpoint: &str) -> Result<Option<String>, String> {
    let endpoint = configured_endpoint.trim();
    let is_local =
        endpoint.is_empty() || endpoint.contains("127.0.0.1") || endpoint.contains("localhost");
    if !is_local {
        return Ok(None);
    }
    let managed = service().ensure().await?;
    Ok(Some(if endpoint.is_empty() {
        managed
    } else {
        endpoint.to_string()
    }))
}

fn tail_suffix(tail: &str) -> String {
    let trimmed = tail.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("\n{trimmed}")
    }
}

fn drain<R>(mut reader: R, sink: Arc<Mutex<String>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 2048];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let mut text = sink.lock().await;
                    text.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if text.len() > 4000 {
                        let cut = text.len() - 4000;
                        *text = text.split_off(cut);
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_targets_the_v1_responses_root() {
        assert_eq!(endpoint_for(DEFAULT_PORT), "http://127.0.0.1:8791/v1");
    }

    #[test]
    fn resolve_command_reports_an_actionable_error_when_absent() {
        if std::env::var_os("RIGA_OPENCODE_BIN").is_none()
            && std::env::var_os("RIGA_OPENCODE_ADAPTER").is_none()
            && !on_path("riga-opencode")
        {
            let error = resolve_command().unwrap_err();
            assert!(error.contains("RIGA_OPENCODE_BIN"), "unhelpful: {error}");
        }
    }
}
