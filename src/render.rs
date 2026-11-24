use crate::quota::Window;
use base64::{Engine, engine::general_purpose::STANDARD};

pub fn countdown(window: &Window, now: i64) -> String {
    let seconds = window.resets_at.saturating_sub(now).max(0);
    if seconds == 0 {
        return "Reset due".into();
    }
    let minutes = (seconds + 59) / 60;
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

pub fn image(
    label: &str,
    window: Option<&Window>,
    banked: Option<u64>,
    state: &str,
    now: i64,
) -> String {
    let color = if state == "Stale" {
        "#f6b44a"
    } else {
        "#67e8c0"
    };
    let percent = window
        .map(|w| format!("{:.0}", w.remaining()))
        .unwrap_or_else(|| "—".into());
    let circumference = 2.0 * std::f64::consts::PI * 41.0;
    let ring = circumference * window.map(|w| w.remaining()).unwrap_or(0.0) / 100.0;
    let duration = window
        .map(|w| {
            let m = w.window_duration_mins;
            if m % 1440 == 0 {
                format!("{}d", m / 1440)
            } else if m % 60 == 0 {
                format!("{}h", m / 60)
            } else {
                format!("{m}m")
            }
        })
        .unwrap_or_default();
    let countdown = window
        .map(|w| countdown(w, now))
        .unwrap_or_else(|| "Press key".into());
    let heading = if duration.is_empty() {
        label
    } else {
        &duration
    };
    let center = if window.is_some() {
        percent.as_str()
    } else if state == "Reading" {
        "…"
    } else if state == "Unavailable" {
        "N/A"
    } else {
        "—"
    };
    let detail = if state == "Stale" || state == "Reading" {
        state
    } else if window.is_some() {
        "% left"
    } else {
        "Refresh"
    };
    let center_font = if center.chars().count() >= 3 { 36 } else { 40 };
    let banked = banked.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
    let background = include_str!("../assets/icons/codex-background.svg");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
<rect width="144" height="144" rx="14" fill="#101820"/>
{background}
<g font-family="DejaVu Sans, sans-serif" text-anchor="middle" fill="#eef6fa">
<text x="7" y="21" text-anchor="start" font-size="21" font-weight="bold">{heading}</text>
<text x="137" y="21" text-anchor="end" font-size="23" font-weight="bold">↻ {banked}</text>
<circle cx="72" cy="66" r="41" fill="none" stroke="#2d3c47" stroke-width="7"/>
<circle cx="72" cy="66" r="41" fill="none" stroke="{color}" stroke-width="7" stroke-dasharray="{ring} {circumference}" transform="rotate(-90 72 66)"/>
<text x="72" y="76" font-size="{center_font}" font-weight="bold" letter-spacing="-2">{center}</text>
<text x="72" y="96" font-size="17" font-weight="bold" fill="{color}">{detail}</text>
<text x="72" y="136" font-size="24" font-weight="bold">{countdown}</text>
</g></svg>"##
    );
    format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn countdown_rounds_up_and_never_invents_a_reset() {
        let mut w = Window {
            used_percent: 25.0,
            window_duration_mins: 300,
            resets_at: 3661,
        };
        assert_eq!(countdown(&w, 0), "1h 2m");
        w.resets_at = 0;
        assert_eq!(countdown(&w, 10), "Reset due");
        assert_eq!(w.remaining(), 75.0);
    }
}
