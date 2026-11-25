//! OpenDeck plugin for displaying Codex subscription quota on two keys.
//!
//! Both actions share one probe, cache, and refresh schedule. Key redraws can
//! update the countdown from cached quota without starting a probe.

mod cache;
mod diagnostics;
mod probe;
mod quota;
mod render;
mod settings;

use openaction::*;
use render::KeyState;
use settings::{Polling, RefreshSettings};
use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

const SESSION: &str = "io.github.spurlos.codexlimits.session";
const WEEKLY: &str = "io.github.spurlos.codexlimits.weekly";

#[derive(Default)]
struct Cache {
    snapshot: Option<cache::Snapshot>,
    refreshing: bool,
    error: Option<diagnostics::Failure>,
}

struct Shared {
    cache: Mutex<Cache>,
    cache_path: Option<PathBuf>,
    // Held through probe cleanup to coalesce overlapping refresh requests.
    probe: Arc<Mutex<()>>,
    rendering: Mutex<()>,
    polling: watch::Sender<Polling>,
}

impl Shared {
    fn new() -> Self {
        let cache_path = cache::path();
        let snapshot = cache_path.as_deref().and_then(|path| {
            cache::Snapshot::load(path).unwrap_or_else(|_| {
                eprintln!("Cannot load the saved Codex quota cache");
                None
            })
        });
        Self {
            cache: Mutex::new(Cache {
                snapshot,
                ..Default::default()
            }),
            cache_path,
            probe: Arc::default(),
            rendering: Mutex::default(),
            polling: watch::channel(Polling::default()).0,
        }
    }
}

static SHARED: LazyLock<Arc<Shared>> = LazyLock::new(|| Arc::new(Shared::new()));

#[derive(PartialEq, Eq)]
enum RefreshTrigger {
    Manual,
    Scheduled,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

impl Shared {
    async fn status(&self) -> diagnostics::Status {
        let cache = self.cache.lock().await;
        let schedule = self.polling.borrow();
        diagnostics::Status {
            r#type: "quotaStatus",
            last_success_at: cache
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.last_success_at),
            refreshing: cache.refreshing,
            error_code: cache.error,
            error_message: cache.error.map(|error| error.message()),
            consecutive_failures: schedule.failures,
            next_refresh_at: schedule.deadline().map(|deadline| {
                now().saturating_add(
                    deadline.saturating_duration_since(Instant::now()).as_secs() as i64
                )
            }),
            auto_refresh_enabled: schedule
                .settings
                .as_ref()
                .is_some_and(|s| s.auto_refresh_enabled),
            has_visible_buttons: !schedule.visible.is_empty(),
        }
    }

    async fn publish_status(&self) {
        let status = self.status().await;
        for action in [SESSION, WEEKLY] {
            for instance in visible_instances(action).await {
                let _ = instance.send_to_property_inspector(&status).await;
            }
        }
    }

    async fn paint(&self, instance: &Instance) -> OpenActionResult<()> {
        let cache = self.cache.lock().await;
        let weekly = instance.action_uuid == WEEKLY;
        let quota = cache.snapshot.as_ref().map(|snapshot| &snapshot.quota);
        let window = quota.and_then(|q| {
            if weekly {
                q.secondary.as_ref()
            } else {
                q.primary.as_ref()
            }
        });
        let state = match (window.is_some(), cache.refreshing, cache.error.is_some()) {
            (true, true, _) => KeyState::Updating,
            (true, false, true) => KeyState::Stale,
            (true, false, false) => KeyState::Ready,
            (false, true, _) => KeyState::Reading,
            (false, false, true) => KeyState::Unavailable,
            (false, false, false) if quota.is_some() => KeyState::Unavailable,
            (false, false, false) => KeyState::Manual,
        };
        let image = render::image(
            if weekly { "Weekly" } else { "Session" },
            window,
            quota.and_then(|q| q.banked_resets),
            state,
            now(),
        );
        drop(cache);
        instance.set_image(Some(image), None).await
    }

    async fn paint_all(&self) {
        let _guard = self.rendering.lock().await;
        for action in [SESSION, WEEKLY] {
            for instance in visible_instances(action).await {
                if self.paint(&instance).await.is_err() {
                    eprintln!("Cannot update quota key");
                }
            }
        }
    }

    /// Starts a refresh of both quota windows if a probe is idle.
    ///
    /// Scheduled requests recheck their deadline after acquiring the probe
    /// lock because visibility or settings may have changed. Manual requests
    /// can refresh during retry backoff. Overlapping requests share the result
    /// of the probe already in progress.
    fn refresh(self: &Arc<Self>, trigger: RefreshTrigger) {
        let Ok(guard) = self.probe.clone().try_lock_owned() else {
            return;
        };
        if trigger == RefreshTrigger::Scheduled
            && self
                .polling
                .borrow()
                .deadline()
                .is_none_or(|deadline| deadline > Instant::now())
        {
            return;
        }
        self.polling.send_modify(|s| {
            s.has_attempted = true;
            s.refreshing = true;
        });
        let shared = self.clone();
        tokio::spawn(async move {
            shared.cache.lock().await.refreshing = true;
            shared.paint_all().await;
            shared.publish_status().await;
            let result = probe::read().await.map(|quota| cache::Snapshot {
                quota,
                last_success_at: now(),
            });
            if let (Ok(snapshot), Some(path)) = (&result, shared.cache_path.clone()) {
                let snapshot = snapshot.clone();
                if !matches!(
                    tokio::task::spawn_blocking(move || snapshot.save(&path)).await,
                    Ok(Ok(()))
                ) {
                    eprintln!("Cannot save the Codex quota cache");
                }
            }
            let fresh = result.as_ref().ok().map(|snapshot| snapshot.quota.clone());
            let mut cache = shared.cache.lock().await;
            cache.refreshing = false;
            match result {
                Ok(snapshot) => {
                    cache.snapshot = Some(snapshot);
                    cache.error = None;
                }
                Err(error) => {
                    cache.error = Some(diagnostics::Failure::from_error(&error));
                    eprintln!("Quota refresh failed: {}", cache.error.unwrap());
                }
            }
            drop(cache);
            shared.paint_all().await;
            shared.polling.send_modify(|s| {
                s.completed(fresh.as_ref(), now());
                drop(guard);
            });
            shared.publish_status().await;
        });
    }
}

// Both action types share the same cache and probe, following the SDK's Action pattern.
macro_rules! quota_action {
    ($name:ident, $uuid:expr) => {
        struct $name;
        #[async_trait]
        impl Action for $name {
            const UUID: ActionUuid = $uuid;
            type Settings = serde_json::Value;
            async fn will_appear(
                &self,
                instance: &Instance,
                _: &Self::Settings,
            ) -> OpenActionResult<()> {
                {
                    let _guard = SHARED.rendering.lock().await;
                    SHARED.paint(instance).await?;
                }
                SHARED.polling.send_modify(|s| {
                    s.visible.insert(instance.instance_id.clone());
                });
                SHARED.publish_status().await;
                Ok(())
            }
            async fn will_disappear(
                &self,
                instance: &Instance,
                _: &Self::Settings,
            ) -> OpenActionResult<()> {
                SHARED.polling.send_modify(|s| {
                    s.visible.remove(&instance.instance_id);
                });
                SHARED.publish_status().await;
                Ok(())
            }
            async fn key_up(&self, _: &Instance, _: &Self::Settings) -> OpenActionResult<()> {
                SHARED.refresh(RefreshTrigger::Manual);
                Ok(())
            }
            async fn property_inspector_did_appear(
                &self,
                instance: &Instance,
                _: &Self::Settings,
            ) -> OpenActionResult<()> {
                instance
                    .send_to_property_inspector(SHARED.status().await)
                    .await
            }
            async fn send_to_plugin(
                &self,
                instance: &Instance,
                _: &Self::Settings,
                payload: &serde_json::Value,
            ) -> OpenActionResult<()> {
                if payload.get("type").and_then(|v| v.as_str()) == Some("getStatus") {
                    instance
                        .send_to_property_inspector(SHARED.status().await)
                        .await?;
                }
                Ok(())
            }
        }
    };
}
quota_action!(SessionAction, SESSION);
quota_action!(WeeklyAction, WEEKLY);

struct GlobalHandler;
#[async_trait]
impl global_events::GlobalEventHandler for GlobalHandler {
    async fn plugin_ready(&self) -> OpenActionResult<()> {
        get_global_settings().await
    }

    async fn did_receive_global_settings(
        &self,
        event: global_events::DidReceiveGlobalSettingsEvent,
    ) -> OpenActionResult<()> {
        let settings = serde_json::from_value::<RefreshSettings>(event.payload.settings)
            .ok()
            .filter(RefreshSettings::valid)
            .unwrap_or_else(|| {
                eprintln!("Invalid refresh settings; keeping previous values or manual defaults");
                SHARED.polling.borrow().settings.clone().unwrap_or_default()
            });
        SHARED.polling.send_if_modified(|s| {
            if s.settings.as_ref() == Some(&settings) {
                return false;
            }
            s.configure(settings.clone());
            true
        });
        // Persist normalized defaults and broadcast to all open inspectors.
        // OpenDeck sends plugin-originated global settings only to inspectors.
        set_global_settings(settings).await?;
        SHARED.publish_status().await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--probe") {
        println!("{}", serde_json::to_string_pretty(&probe::read().await?)?);
        return Ok(());
    }
    LazyLock::force(&SHARED);
    register_action(SessionAction).await;
    register_action(WeeklyAction).await;
    global_events::set_global_event_handler(&GlobalHandler);
    let polling = tokio::spawn(async {
        let mut receiver = SHARED.polling.subscribe();
        while settings::wait_for_refresh(&mut receiver).await {
            SHARED.refresh(RefreshTrigger::Scheduled);
        }
    });
    let ticker = tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await;
            SHARED.paint_all().await;
        }
    });
    let result = run(std::env::args().collect()).await;
    ticker.abort();
    polling.abort();
    result?;
    Ok(())
}
