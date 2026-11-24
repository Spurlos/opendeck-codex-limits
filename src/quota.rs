use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub used_percent: f64,
    pub window_duration_mins: u64,
    pub resets_at: i64,
}

impl Window {
    pub fn remaining(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
    pub banked_resets: Option<u64>,
}

pub fn parse(result: Value) -> Result<Quota> {
    let snapshot = result
        .pointer("/rateLimitsByLimitId/codex")
        .filter(|v| !v.is_null())
        .or_else(|| result.get("rateLimits"))
        .ok_or_else(|| anyhow::anyhow!("Codex quota unavailable"))?;
    let window = |name: &str| -> Result<Option<Window>> {
        match snapshot.get(name).filter(|v| !v.is_null()) {
            Some(v) => {
                let w: Window = serde_json::from_value(v.clone())?;
                if !w.used_percent.is_finite() || w.window_duration_mins == 0 || w.resets_at < 0 {
                    bail!("Invalid quota window");
                }
                Ok(Some(w))
            }
            None => Ok(None),
        }
    };
    let primary = window("primary")?;
    let secondary = window("secondary")?;
    if primary.is_none() && secondary.is_none() {
        bail!(crate::diagnostics::Failure::UnsupportedAccount);
    }
    Ok(Quota {
        primary,
        secondary,
        banked_resets: result
            .pointer("/rateLimitResetCredits/availableCount")
            .and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(used: f64) -> Value {
        json!({"primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":2000000000}})
    }

    #[test]
    fn prefers_codex_and_preserves_unknown_reset_count() {
        let q = parse(
            json!({"rateLimits": snapshot(10.0), "rateLimitsByLimitId":{"codex":snapshot(30.0)}}),
        )
        .unwrap();
        assert_eq!(q.primary.unwrap().remaining(), 70.0);
        assert_eq!(q.banked_resets, None);
    }

    #[test]
    fn legacy_zero_resets_and_clamping() {
        let q = parse(
            json!({"rateLimits":snapshot(130.0), "rateLimitResetCredits":{"availableCount":0}}),
        )
        .unwrap();
        assert_eq!(q.primary.unwrap().remaining(), 0.0);
        assert_eq!(q.banked_resets, Some(0));
        assert_eq!(
            parse(json!({"rateLimits":snapshot(-10.0)}))
                .unwrap()
                .primary
                .unwrap()
                .remaining(),
            100.0
        );
    }

    #[test]
    fn reads_account_reset_credits_alongside_quota_snapshots() {
        let q = parse(json!({
            "rateLimits": snapshot(10.0),
            "rateLimitsByLimitId": {"codex": snapshot(30.0)},
            "rateLimitResetCredits": {"availableCount": 3}
        }))
        .unwrap();
        assert_eq!(q.banked_resets, Some(3));
        assert_eq!(q.primary.unwrap().remaining(), 70.0);
    }

    #[test]
    fn rejects_missing_and_malformed_data() {
        for s in [
            json!({}),
            json!({"rateLimits":null}),
            json!({"rateLimits":{"primary":{"usedPercent":0}}}),
        ] {
            assert!(parse(s).is_err());
        }
    }
}
