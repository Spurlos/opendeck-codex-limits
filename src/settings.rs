use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::{
    sync::watch,
    time::{Instant, sleep_until},
};

pub const MIN_INTERVAL: u64 = 15;
pub const MAX_INTERVAL: u64 = 86_400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RefreshSettings {
    pub refresh_on_startup: bool,
    pub auto_refresh_enabled: bool,
    pub refresh_interval_seconds: u64,
}

impl Default for RefreshSettings {
    fn default() -> Self {
        Self {
            refresh_on_startup: false,
            auto_refresh_enabled: false,
            refresh_interval_seconds: 60,
        }
    }
}

impl RefreshSettings {
    pub fn valid(&self) -> bool {
        (MIN_INTERVAL..=MAX_INTERVAL).contains(&self.refresh_interval_seconds)
    }
}

#[derive(Clone, Default)]
pub struct Polling {
    pub settings: Option<RefreshSettings>,
    pub visible: HashSet<String>,
    pub refreshing: bool,
    pub has_attempted: bool,
    pub finished_at: Option<Instant>,
    pub failures: u32,
    pub reset_at: Option<Instant>,
}

impl Polling {
    pub fn configure(&mut self, settings: RefreshSettings) {
        if settings.auto_refresh_enabled
            && !self
                .settings
                .as_ref()
                .is_some_and(|s| s.auto_refresh_enabled)
        {
            self.finished_at = None; // Enabling starts with an immediate shared refresh.
        }
        self.settings = Some(settings);
    }

    pub fn deadline(&self) -> Option<Instant> {
        let settings = self.settings.as_ref()?;
        if self.visible.is_empty() || self.refreshing {
            return None;
        }
        if settings.refresh_on_startup && !self.has_attempted {
            return Some(Instant::now());
        }
        if !settings.auto_refresh_enabled {
            return None;
        }
        let base = settings.refresh_interval_seconds;
        let delay = base
            .saturating_mul(1u64 << self.failures.min(16))
            .min(base.max(900));
        let regular = self
            .finished_at
            .map(|t| t + std::time::Duration::from_secs(delay))
            .unwrap_or_else(Instant::now);
        // An expired reset cannot bypass error backoff or cause a retry loop.
        Some(if self.failures == 0 {
            self.reset_at
                .map(|reset| reset.min(regular))
                .unwrap_or(regular)
        } else {
            regular
        })
    }

    pub fn completed(&mut self, quota: Option<&crate::quota::Quota>, unix_now: i64) {
        self.refreshing = false;
        self.finished_at = Some(Instant::now());
        if let Some(quota) = quota {
            self.failures = 0;
            self.reset_at = [quota.primary.as_ref(), quota.secondary.as_ref()]
                .into_iter()
                .flatten()
                .map(|w| w.resets_at)
                .filter(|reset| *reset > unix_now)
                .min()
                .and_then(|reset| {
                    Instant::now().checked_add(std::time::Duration::from_secs(
                        reset.saturating_sub(unix_now) as u64,
                    ))
                });
        } else {
            self.failures = self.failures.saturating_add(1);
        }
    }
}

/// Rebuild the single timer whenever settings, visibility or probe state change.
/// No missed-tick catch-up: the next interval begins when the probe finishes.
pub async fn wait_for_refresh(receiver: &mut watch::Receiver<Polling>) -> bool {
    loop {
        let deadline = receiver.borrow_and_update().deadline();
        if let Some(deadline) = deadline {
            tokio::select! {
                biased;
                result = receiver.changed() => if result.is_err() { return false; },
                _ = sleep_until(deadline) => return true,
            }
        } else if receiver.changed().await.is_err() {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn active() -> Polling {
        Polling {
            settings: Some(RefreshSettings {
                auto_refresh_enabled: true,
                refresh_interval_seconds: 60,
                ..Default::default()
            }),
            visible: HashSet::from(["session".into(), "weekly".into()]),
            ..Default::default()
        }
    }

    #[test]
    fn validates_custom_interval_and_defaults_to_manual() {
        assert!(!RefreshSettings::default().auto_refresh_enabled);
        for seconds in [0, 14, 86_401, u64::MAX] {
            assert!(
                !RefreshSettings {
                    refresh_interval_seconds: seconds,
                    ..Default::default()
                }
                .valid()
            );
        }
        for seconds in [15, 60, 120, 300, 86_400] {
            assert!(
                RefreshSettings {
                    refresh_interval_seconds: seconds,
                    ..Default::default()
                }
                .valid()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn polls_immediately_then_waits_custom_interval_from_completion() {
        let (sender, mut receiver) = watch::channel(active());
        assert!(wait_for_refresh(&mut receiver).await);
        sender.send_modify(|s| {
            s.finished_at = Some(Instant::now());
            s.settings.as_mut().unwrap().refresh_interval_seconds = 120;
        });
        let task = tokio::spawn(async move { wait_for_refresh(&mut receiver).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(119)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(task.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn disabling_cancels_timer_and_enabling_refreshes_immediately() {
        let mut state = active();
        state.finished_at = Some(Instant::now());
        let (sender, mut receiver) = watch::channel(state);
        let task = tokio::spawn(async move { wait_for_refresh(&mut receiver).await });
        tokio::task::yield_now().await;
        sender.send_modify(|s| s.configure(RefreshSettings::default()));
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(!task.is_finished());
        sender.send_modify(|s| {
            s.configure(RefreshSettings {
                auto_refresh_enabled: true,
                ..Default::default()
            })
        });
        assert!(task.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_settings_visibility_and_probe_cleanup() {
        let (sender, mut receiver) = watch::channel(Polling::default());
        let task = tokio::spawn(async move { wait_for_refresh(&mut receiver).await });
        tokio::task::yield_now().await;
        sender.send_modify(|s| {
            s.configure(RefreshSettings {
                auto_refresh_enabled: true,
                ..Default::default()
            })
        });
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(!task.is_finished());
        sender.send_modify(|s| {
            s.visible.insert("weekly".into());
            s.refreshing = true;
        });
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(!task.is_finished());
        sender.send_modify(|s| {
            s.refreshing = false;
            s.finished_at = Some(Instant::now());
        });
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(task.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn interval_change_and_hiding_cancel_existing_deadline() {
        let mut state = active();
        state.finished_at = Some(Instant::now());
        let (sender, mut receiver) = watch::channel(state);
        let task = tokio::spawn(async move { wait_for_refresh(&mut receiver).await });
        tokio::task::yield_now().await;
        sender.send_modify(|s| s.settings.as_mut().unwrap().refresh_interval_seconds = 300);
        tokio::time::advance(Duration::from_secs(120)).await;
        assert!(!task.is_finished());
        sender.send_modify(|s| s.visible.clear());
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(!task.is_finished());
        sender.send_modify(|s| {
            s.visible.insert("session".into());
        });
        assert!(task.await.unwrap());
    }

    fn quota(primary: i64, secondary: i64) -> crate::quota::Quota {
        let window = |resets_at| crate::quota::Window {
            used_percent: 30.0,
            window_duration_mins: 300,
            resets_at,
        };
        crate::quota::Quota {
            primary: Some(window(primary)),
            secondary: Some(window(secondary)),
            banked_resets: Some(3),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn failures_back_off_with_cap_and_success_restores_normal_interval() {
        let mut state = active();
        for expected in [120, 240, 480, 900, 900] {
            state.completed(None, 1000);
            assert_eq!(
                state
                    .deadline()
                    .unwrap()
                    .duration_since(Instant::now())
                    .as_secs(),
                expected
            );
        }
        state.completed(Some(&quota(10_000, 20_000)), 1000);
        assert_eq!(state.failures, 0);
        assert_eq!(
            state
                .deadline()
                .unwrap()
                .duration_since(Instant::now())
                .as_secs(),
            60
        );
        state.settings.as_mut().unwrap().refresh_interval_seconds = 3600;
        state.completed(None, 1000);
        assert_eq!(
            state
                .deadline()
                .unwrap()
                .duration_since(Instant::now())
                .as_secs(),
            3600
        );
    }

    #[tokio::test(start_paused = true)]
    async fn refreshes_at_earliest_window_reset_before_long_poll_interval() {
        let mut state = active();
        state.settings.as_mut().unwrap().refresh_interval_seconds = 3600;
        state.completed(Some(&quota(1030, 1600)), 1000);
        let (_sender, mut receiver) = watch::channel(state);
        let task = tokio::spawn(async move { wait_for_refresh(&mut receiver).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(task.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_reset_never_loops_or_bypasses_failure_backoff() {
        let mut state = active();
        state.completed(Some(&quota(1001, 1001)), 1000);
        tokio::time::advance(Duration::from_secs(2)).await;
        state.completed(None, 1002);
        assert_eq!(
            state
                .deadline()
                .unwrap()
                .duration_since(Instant::now())
                .as_secs(),
            120
        );
        state.completed(Some(&quota(1001, 1001)), 1002);
        assert!(state.reset_at.is_none());
        assert_eq!(
            state
                .deadline()
                .unwrap()
                .duration_since(Instant::now())
                .as_secs(),
            60
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reset_schedule_obeys_manual_mode_visibility_and_shared_completion() {
        let mut state = active();
        state.completed(Some(&quota(1001, 1001)), 1000);
        state.settings.as_mut().unwrap().auto_refresh_enabled = false;
        assert!(state.deadline().is_none());
        state.settings.as_mut().unwrap().auto_refresh_enabled = true;
        state.visible.clear();
        assert!(state.deadline().is_none());
        state.visible.insert("weekly".into());
        state.refreshing = true;
        assert!(state.deadline().is_none());
        // A manual refresh succeeds during backoff: immediately use its new reset.
        state.failures = 5;
        state.completed(Some(&quota(1030, 1600)), 1000);
        assert_eq!(
            state
                .deadline()
                .unwrap()
                .duration_since(Instant::now())
                .as_secs(),
            30
        );
    }
}
