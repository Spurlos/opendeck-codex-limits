mod diagnostics;
mod probe;
mod quota;
mod render;
mod settings;

use openaction::*;
use settings::{Polling, RefreshSettings};
use std::{
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

const SESSION: &str = "com.spurlos.codexlimits.session";
const WEEKLY: &str = "com.spurlos.codexlimits.weekly";

#[derive(Default)]
struct Cache {
    quota: Option<quota::Quota>,
    failed: bool,
    refreshing: bool,
    last_success_at: Option<i64>,
    error: Option<diagnostics::Failure>,
}

struct Shared {
    cache: Mutex<Cache>,
    // Held across the entire probe, including cleanup. try_lock coalesces presses.
    probe: Arc<Mutex<()>>,
    rendering: Mutex<()>,
    polling: watch::Sender<Polling>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            cache: Mutex::default(),
            probe: Arc::default(),
            rendering: Mutex::default(),
            polling: watch::channel(Polling::default()).0,
        }
    }
}

static SHARED: LazyLock<Arc<Shared>> = LazyLock::new(|| Arc::new(Shared::default()));

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
            last_success_at: cache.last_success_at,
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
        let window = cache.quota.as_ref().and_then(|q| {
            if weekly {
                q.secondary.as_ref()
            } else {
                q.primary.as_ref()
            }
        });
        let state = if cache.refreshing {
            "Reading"
        } else if cache.failed && cache.quota.is_some() {
            "Stale"
        } else if cache.failed || (cache.quota.is_some() && window.is_none()) {
            "Unavailable"
        } else if cache.quota.is_some() {
            ""
        } else {
            "Manual"
        };
        let image = render::image(
            if weekly { "Weekly" } else { "Session" },
            window,
            cache.quota.as_ref().and_then(|q| q.banked_resets),
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

    fn refresh(self: &Arc<Self>) {
        let Ok(guard) = self.probe.clone().try_lock_owned() else {
            return;
        };
        self.polling.send_modify(|s| s.refreshing = true);
        let shared = self.clone();
        tokio::spawn(async move {
            shared.cache.lock().await.refreshing = true;
            shared.paint_all().await;
            shared.publish_status().await;
            let result = probe::read().await;
            let fresh = result.as_ref().ok().cloned();
            let mut cache = shared.cache.lock().await;
            cache.refreshing = false;
            match result {
                Ok(quota) => {
                    cache.quota = Some(quota);
                    cache.failed = false;
                    cache.last_success_at = Some(now());
                    cache.error = None;
                }
                Err(error) => {
                    cache.failed = true;
                    cache.error = Some(diagnostics::Failure::from_error(&error));
                    eprintln!("Quota refresh failed: {}", cache.error.unwrap());
                }
            }
            drop(cache);
            shared.paint_all().await;
            drop(guard);
            shared.polling.send_modify(|s| {
                s.completed(fresh.as_ref(), now());
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
                SHARED.paint(instance).await?;
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
                SHARED.refresh();
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
    register_action(SessionAction).await;
    register_action(WeeklyAction).await;
    global_events::set_global_event_handler(&GlobalHandler);
    let polling = tokio::spawn(async {
        let mut receiver = SHARED.polling.subscribe();
        while settings::wait_for_refresh(&mut receiver).await {
            SHARED.refresh();
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
