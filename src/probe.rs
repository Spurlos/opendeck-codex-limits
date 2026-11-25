use crate::diagnostics::Failure;
use crate::quota::{self, Quota};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

async fn response(reader: &mut (impl AsyncBufRead + Unpin), id: u64) -> Result<Value> {
    loop {
        let mut bytes = Vec::new();
        // Bound individual JSON lines, including notifications, to 1 MiB.
        let n = (&mut *reader)
            .take(1024 * 1024)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| Failure::ProcessFailed)?;
        if n == 0 {
            bail!(Failure::ProcessFailed);
        }
        if bytes.last() != Some(&b'\n') {
            bail!(Failure::InvalidResponse);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                // Do not expose raw errors or account data in logs/key images.
                bail!(Failure::remote(error));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| Failure::InvalidResponse.into());
        }
    }
}

/// Reads account quota using the `codex` executable on the inherited
/// `PATH` and its existing login. The request neither starts a conversation
/// nor redeems reset credits.
///
/// # Errors
///
/// Returns an error if Codex cannot start, the account cannot provide quota
/// data, or the request fails, including a 30-second response timeout.
pub async fn read() -> Result<Quota> {
    read_from("codex", Duration::from_secs(30)).await
}

/// Performs the app-server handshake and quota request within `limit`.
///
/// Cleanup has a separate two-second grace period. The probe closes stdin
/// and drains stdout while waiting for Codex to exit, then kills and reaps
/// the child if cleanup stalls.
pub(crate) async fn read_from(executable: &str, limit: Duration) -> Result<Quota> {
    let mut child = Command::new(executable)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Failure::MissingExecutable
            } else {
                Failure::StartFailed
            }
        })?;
    let mut stdin = child.stdin.take().context("Missing Codex stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().context("Missing Codex stdout")?);
    let result = timeout(limit, async {
        stdin.write_all(format!("{}\n", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"opendeck_codex_limits","version":env!("CARGO_PKG_VERSION")}}})).as_bytes()).await?;
        response(&mut stdout, 1).await?;
        stdin.write_all(b"{\"method\":\"initialized\"}\n").await?;
        stdin.write_all(b"{\"id\":2,\"method\":\"account/rateLimits/read\",\"params\":null}\n").await?;
        quota::parse(response(&mut stdout, 2).await?).map_err(|error| {
            error.downcast_ref::<Failure>().copied().unwrap_or(Failure::InvalidResponse).into()
        })
    }).await.map_err(|_| anyhow::Error::from(Failure::Timeout)).and_then(|r| r);
    // EOF gives app-server a chance to exit normally; force and reap if it stalls.
    drop(stdin);
    // Drain any shutdown notifications so a full pipe cannot prevent graceful exit.
    let cleanup = timeout(Duration::from_secs(2), async {
        let mut sink = tokio::io::sink();
        tokio::try_join!(child.wait(), tokio::io::copy(&mut stdout, &mut sink))
    })
    .await;
    match cleanup {
        Ok(Ok((status, _))) if status.success() => result,
        Ok(Ok(_)) if result.is_ok() => anyhow::bail!(Failure::ProcessFailed),
        Ok(Ok(_)) => result,
        _ => {
            child.kill().await.context("Cannot stop Codex probe")?;
            child.wait().await.context("Cannot reap Codex probe")?;
            result
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    async fn fixture(mode: &str, limit: Duration) -> Result<Quota> {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("codex");
        let marker = dir.path().join("pid");
        let source = include_str!("../tests/fixtures/codex.py");
        let script_text = format!(
            "#!/usr/bin/env python3\nimport os\nos.environ['PROBE_MARKER'] = {}\nos.environ['PROBE_MODE'] = {}\n{}",
            serde_json::to_string(marker.to_str().unwrap()).unwrap(),
            serde_json::to_string(mode).unwrap(),
            source.lines().skip(1).collect::<Vec<_>>().join("\n"),
        );
        std::fs::write(&script, script_text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = read_from(script.to_str().unwrap(), limit).await;
        let pid = std::fs::read_to_string(marker).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "probe was not reaped"
        );
        result
    }

    #[tokio::test]
    async fn handshake_and_graceful_exit() {
        let quota = fixture("success", Duration::from_secs(5)).await.unwrap();
        assert_eq!(quota.primary.unwrap().remaining(), 75.0);
        assert_eq!(quota.secondary.unwrap().remaining(), 40.0);
        assert_eq!(quota.banked_resets, Some(3));
    }

    #[tokio::test]
    async fn timeout_kills_and_reaps() {
        let error = fixture("hang", Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(Failure::from_error(&error), Failure::Timeout);
    }

    #[tokio::test]
    async fn shutdown_stall_is_bounded() {
        assert!(fixture("linger", Duration::from_secs(5)).await.is_ok());
    }

    #[tokio::test]
    async fn remote_errors_are_sanitized() {
        let error = fixture("error", Duration::from_secs(5)).await.unwrap_err();
        assert!(!format!("{error:#}").contains("private diagnostic"));
    }

    #[tokio::test]
    async fn missing_executable_is_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_from(
            dir.path().join("not-installed").to_str().unwrap(),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(Failure::from_error(&error), Failure::MissingExecutable);
    }

    #[tokio::test]
    async fn malformed_response_is_actionable() {
        let mut reader = BufReader::new(b"{invalid}\n".as_slice());
        let error = response(&mut reader, 2).await.unwrap_err();
        assert_eq!(Failure::from_error(&error), Failure::InvalidResponse);
    }
}
