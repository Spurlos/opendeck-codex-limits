#![cfg(target_os = "linux")]

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[tokio::test]
async fn both_keys_share_probe_and_retain_stale_data() {
    check_shared_probe(false).await;
}

#[tokio::test]
async fn startup_refresh_reads_both_quotas_once_without_polling() {
    check_shared_probe(true).await;
}

async fn check_shared_probe(startup: bool) {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("probes");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut child = start_plugin(dir.path(), &listener, "fail-second");
    timeout(Duration::from_secs(10), async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let registration: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(
            registration,
            json!({"event":"registerPlugin","uuid":"io.github.spurlos.codexlimits"})
        );
        assert_eq!(
            next_event(&mut ws, "getGlobalSettings").await["context"],
            "io.github.spurlos.codexlimits"
        );
        ws.send(Message::Text(
            json!({"event":"didReceiveGlobalSettings","payload":{"settings":{}}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        assert_eq!(
            next_event(&mut ws, "setGlobalSettings").await["payload"]["autoRefreshEnabled"],
            false
        );
        let events = appear_buttons(&mut ws).await;
        if startup {
            configure(&mut ws, true, false, 60).await; // Global settings can arrive after the buttons.
        } else {
            let mut initial = HashMap::new();
            while initial.len() < 2 {
                let value = next_event(&mut ws, "setImage").await;
                let svg = decode(&value);
                assert!(svg.contains("Press key"));
                initial.insert(value["context"].as_str().unwrap().to_string(), svg);
            }
            assert!(!marker.exists(), "manual mode must not fetch on appearance");
            // Alternate the keys rapidly; only one app-server may start.
            for i in 0..8 {
                let mut event = events[i % 2].clone();
                event["event"] = json!("keyUp");
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
        }
        let mut fresh = HashMap::new();
        while fresh.len() < 2 {
            let value: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            if value["event"] == "setImage" {
                let svg = decode(&value);
                if svg.contains("% left") {
                    assert!(svg.contains("↻ 3"));
                    fresh.insert(value["context"].as_str().unwrap().to_string(), svg);
                }
            }
        }
        assert!(fresh["session-key"].contains(">75</text>"));
        assert!(fresh["weekly-key"].contains(">40</text>"));
        request_status(&mut ws, &events[0]).await;
        let fresh_status = status_matching(&mut ws, |s| {
            s["lastSuccessAt"].is_i64() && s["consecutiveFailures"] == 0 && s["refreshing"] == false
        })
        .await;
        assert!(fresh_status["nextRefreshAt"].is_null());
        let pids = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(pids.lines().count(), 1);
        assert!(
            !std::path::Path::new(&format!("/proc/{}", pids.trim())).exists(),
            "probe must be reaped before rendering fresh data"
        );
        if startup {
            for event in &events {
                let mut hidden = event.clone();
                hidden["event"] = json!("willDisappear");
                ws.send(Message::Text(hidden.to_string().into()))
                    .await
                    .unwrap();
            }
            for event in &events {
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            wait_for_fresh(&mut ws, 2).await;
            assert!(
                timeout(
                    Duration::from_millis(400),
                    status_matching(&mut ws, |s| {
                        s["refreshing"] == true || s["consecutiveFailures"] != 0
                    })
                )
                .await
                .is_err(),
                "returning buttons must reuse the startup reading"
            );
            assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);
        }
        let mut event = events[1].clone();
        event["event"] = json!("keyUp");
        ws.send(Message::Text(event.to_string().into()))
            .await
            .unwrap();
        let mut stale = HashMap::new();
        while stale.len() < 2 {
            let value: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            if value["event"] == "setImage" {
                let svg = decode(&value);
                if svg.contains("Stale") {
                    stale.insert(value["context"].as_str().unwrap().to_string(), svg);
                }
            }
        }
        assert!(stale["session-key"].contains(">75</text>"));
        assert!(stale["weekly-key"].contains(">40</text>"));
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
        request_status(&mut ws, &events[1]).await;
        let stale_status = status_matching(&mut ws, |s| {
            s["consecutiveFailures"] == 1 && s["refreshing"] == false
        })
        .await;
        assert_eq!(stale_status["lastSuccessAt"], fresh_status["lastSuccessAt"]);
        assert_eq!(stale_status["errorCode"], "requestFailed");
        assert!(!stale_status.to_string().contains("private diagnostic"));
        // Enabling auto causes one immediate failed probe, then backs off to 60s.
        configure(&mut ws, startup, true, 15).await;
        let retry_status = status_matching(&mut ws, |s| {
            s["consecutiveFailures"] == 2 && s["refreshing"] == false
        })
        .await;
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(retry_status["nextRefreshAt"].as_i64().unwrap() >= current_time + 58);
        let mut manual = events[0].clone();
        manual["event"] = json!("keyUp");
        ws.send(Message::Text(manual.to_string().into()))
            .await
            .unwrap();
        let manual_status = status_matching(&mut ws, |s| {
            s["consecutiveFailures"] == 3 && s["refreshing"] == false
        })
        .await;
        assert_eq!(
            manual_status["lastSuccessAt"],
            fresh_status["lastSuccessAt"]
        );
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            4,
            "manual refresh must bypass the pending backoff"
        );
        ws.close(None).await.unwrap();
        drop(ws);
        assert!(child.wait().await.unwrap().success());
    })
    .await
    .expect("plugin protocol test timed out");
}

type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn next_event(ws: &mut Socket, event: &str) -> Value {
    loop {
        let value: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        if value["event"] == event {
            return value;
        }
    }
}

async fn configure(ws: &mut Socket, startup: bool, enabled: bool, seconds: u64) {
    let settings = json!({
        "refreshOnStartup": startup,
        "autoRefreshEnabled": enabled,
        "refreshIntervalSeconds": seconds,
    });
    send_global_settings(ws, &settings).await;
    assert_eq!(
        next_event(ws, "setGlobalSettings").await["payload"],
        settings
    );
}

async fn send_global_settings(ws: &mut Socket, settings: &Value) {
    ws.send(Message::Text(
        json!({"event":"didReceiveGlobalSettings","payload":{"settings":settings}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
}

async fn request_status(ws: &mut Socket, instance: &Value) {
    ws.send(Message::Text(json!({"event":"sendToPlugin", "action":instance["action"],"context":instance["context"],"payload":{"type":"getStatus"}}).to_string().into())).await.unwrap();
}

async fn appear_buttons(ws: &mut Socket) -> Vec<Value> {
    let mut events = Vec::new();
    for (context, action) in [("session-key", "session"), ("weekly-key", "weekly")] {
        let event = json!({
            "event": "willAppear",
            "action": format!("io.github.spurlos.codexlimits.{action}"),
            "context": context,
            "device": "test-device",
            "payload": {
                "settings": {},
                "controller": "Keypad",
                "coordinates": {"row": 0, "column": 0},
                "state": 0,
                "isInMultiAction": false,
            },
        });
        ws.send(Message::Text(event.to_string().into()))
            .await
            .unwrap();
        events.push(event);
    }
    events
}

async fn status_matching(ws: &mut Socket, predicate: impl Fn(&Value) -> bool) -> Value {
    loop {
        let event = next_event(ws, "sendToPropertyInspector").await;
        if event["payload"]["type"] == "quotaStatus" && predicate(&event["payload"]) {
            return event["payload"].clone();
        }
    }
}

async fn wait_for_fresh(ws: &mut Socket, count: usize) {
    let mut contexts = std::collections::HashSet::new();
    while contexts.len() < count {
        let value = next_event(ws, "setImage").await;
        if decode(&value).contains("% left") {
            contexts.insert(value["context"].as_str().unwrap().to_string());
        }
    }
}

#[tokio::test]
async fn restores_shared_settings_and_refreshes_both_keys_at_reset() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("probes");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut child = start_plugin(dir.path(), &listener, "reset");
    timeout(Duration::from_secs(25), async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        next_event(&mut ws, "registerPlugin").await;
        next_event(&mut ws, "getGlobalSettings").await;
        configure(&mut ws, true, true, 3600).await; // Simulate settings persisted across a restart.
        assert!(!marker.exists(), "no polling without a visible quota key");
        let events = appear_buttons(&mut ws).await;
        wait_for_fresh(&mut ws, 2).await;
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            1,
            "startup and automatic refresh must share the initial probe"
        );
        // No tap: refresh at the reported reset, far before the one-hour interval.
        loop {
            let value = next_event(&mut ws, "setImage").await;
            if decode(&value).contains("Updating") {
                break;
            }
        }
        wait_for_fresh(&mut ws, 2).await;
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            2,
            "the same expired reset must not cause a loop"
        );
        configure(&mut ws, true, true, 120).await;
        configure(&mut ws, true, false, 120).await;
        // Manual refresh remains available with polling off.
        let mut event = events[1].clone();
        event["event"] = json!("keyUp");
        ws.send(Message::Text(event.to_string().into()))
            .await
            .unwrap();
        wait_for_fresh(&mut ws, 2).await;
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 3);
        for event in &events {
            let mut event = event.clone();
            event["event"] = json!("willDisappear");
            ws.send(Message::Text(event.to_string().into()))
                .await
                .unwrap();
        }
        configure(&mut ws, true, true, 15).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            3,
            "hidden keys must not start an immediate probe"
        );
        ws.send(Message::Text(events[0].to_string().into()))
            .await
            .unwrap();
        // The cached frame comes first on appearance; then Updating; then new data.
        loop {
            let value = next_event(&mut ws, "setImage").await;
            if decode(&value).contains("Updating") {
                break;
            }
        }
        wait_for_fresh(&mut ws, 1).await;
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 4);
        ws.close(None).await.unwrap();
        drop(ws);
        assert!(child.wait().await.unwrap().success());
    })
    .await
    .expect("automatic refresh protocol test timed out");
}

#[tokio::test]
async fn restores_saved_quota_on_restart_and_preserves_it_during_a_failed_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache/opendeck-codex-limits/quota.json");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, b"{ corrupt JSON").unwrap();
    let corrupt_inode = std::fs::metadata(&cache).unwrap().ino();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    timeout(Duration::from_secs(10), async {
        let mut child = start_plugin(dir.path(), &listener, "success");
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        next_event(&mut ws, "getGlobalSettings").await;
        let events = appear_buttons(&mut ws).await;
        assert!(decode(&next_event(&mut ws, "setImage").await).contains("Press key"));
        send_global_settings(&mut ws, &json!({"refreshOnStartup": true})).await;
        loop {
            let frame = next_event(&mut ws, "setImage").await;
            if decode(&frame).contains("Reading…") {
                break;
            }
        }
        wait_for_fresh(&mut ws, 2).await;
        request_status(&mut ws, &events[0]).await;
        let fresh = status_matching(&mut ws, |s| {
            s["lastSuccessAt"].is_i64() && s["refreshing"] == false
        })
        .await;
        let saved = std::fs::read(&cache).unwrap();
        let snapshot: Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(snapshot["lastSuccessAt"], fresh["lastSuccessAt"]);
        assert_eq!(snapshot["quota"]["bankedResets"], 3);
        let saved_inode = std::fs::metadata(&cache).unwrap().ino();
        assert_ne!(
            saved_inode, corrupt_inode,
            "successful refresh atomically replaces the file"
        );
        ws.close(None).await.unwrap();
        drop(ws);
        assert!(child.wait().await.unwrap().success());

        let mut child = start_plugin(dir.path(), &listener, "error");
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        next_event(&mut ws, "getGlobalSettings").await;
        let events = appear_buttons(&mut ws).await;
        assert_saved_frames(&mut ws, "% left").await;
        request_status(&mut ws, &events[0]).await;
        let restored = status_matching(&mut ws, |s| s["lastSuccessAt"].is_i64()).await;
        assert_eq!(restored["lastSuccessAt"], snapshot["lastSuccessAt"]);
        send_global_settings(&mut ws, &json!({"refreshOnStartup": true})).await;
        assert_saved_frames(&mut ws, "Updating").await;
        assert_saved_frames(&mut ws, "Stale").await;
        assert_eq!(std::fs::read(&cache).unwrap(), saved);
        assert_eq!(std::fs::metadata(&cache).unwrap().ino(), saved_inode);
        ws.close(None).await.unwrap();
        drop(ws);
        assert!(child.wait().await.unwrap().success());
    })
    .await
    .expect("quota cache restart test timed out");
}

async fn assert_saved_frames(ws: &mut Socket, label: &str) {
    let mut frames = HashMap::new();
    while frames.len() < 2 {
        let frame = next_event(ws, "setImage").await;
        let svg = decode(&frame);
        if svg.contains(label) {
            frames.insert(frame["context"].as_str().unwrap().to_owned(), svg);
        }
    }
    assert!(frames["session-key"].contains(">75</text>"));
    assert!(frames["weekly-key"].contains(">40</text>"));
}

fn start_plugin(dir: &Path, listener: &TcpListener, mode: &str) -> Child {
    let executable = dir.join("codex");
    std::fs::write(&executable, include_str!("fixtures/codex.py")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![dir.to_owned()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    Command::new(env!("CARGO_BIN_EXE_opendeck-codex-limits"))
        .args([
            "-port",
            &listener.local_addr().unwrap().port().to_string(),
            "-pluginUUID",
            "io.github.spurlos.codexlimits",
            "-registerEvent",
            "registerPlugin",
            "-info",
            "{\"devices\":[]}",
        ])
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("PROBE_MARKER", dir.join("probes"))
        .env("PROBE_MODE", mode)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

fn decode(value: &Value) -> String {
    let data = value["payload"]["image"].as_str().unwrap();
    String::from_utf8(
        STANDARD
            .decode(data.strip_prefix("data:image/svg+xml;base64,").unwrap())
            .unwrap(),
    )
    .unwrap()
}
