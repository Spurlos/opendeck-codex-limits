# Codex Limits for OpenDeck

A Linux OpenDeck plugin showing Codex subscription quota on two Stream Deck keys.
Press either key to update both windows, or enable automatic refresh with a
shared, configurable interval. It uses the existing Codex login and runs
independently of T3 Code.

The implementation follows the official [OpenAction Rust SDK starter example](https://github.com/OpenActionAPI/rust)
and the [counter plugin layout](https://github.com/OpenActionPlugins/counter), linked
from the [OpenAction manifest documentation](https://openaction.amankhanna.me/manifest.html).
OpenDeck links to OpenAction in its [README](https://github.com/nekename/OpenDeck).
The SDK handles registration, WebSocket messages and action lifecycle events.

## Build and install

Requirements: Linux, Rustup (the toolchain file selects Rust 1.89), `jq` for
packaging, a native OpenDeck installation, and an authenticated `codex`
executable on OpenDeck's PATH.
Inherited `CODEX_HOME` is respected. No separate credentials are stored.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash scripts/package.sh
```

Copy `dist/io.github.spurlos.codexlimits.sdPlugin` into the `plugins` folder inside
OpenDeck's config directory (use **Open config directory** in OpenDeck settings).
Restart OpenDeck and add **Session Quota** and **Weekly Quota** to keys. The package
contains a binary for the machine it was built on; rebuild on other architectures.

## Release version

Set the version in `Cargo.toml`, let Cargo update `Cargo.lock`, and commit
both files. The packaging script adds that version to the bundled manifest
using `assets/manifest.json` as its template. Run `bash scripts/tag-release.sh`
to create a local annotated tag for the committed Cargo version, such as
`v0.1.0`.

## Shared refresh settings

Select either quota button in OpenDeck and click **Save for all buttons** after
editing its refresh options. Both options are off by default and persist across
restarts.

- **Refresh once on startup** reads both quota windows when the first quota
  button appears after OpenDeck starts. Every quota button shares the result,
  including when you switch profiles. This works with periodic refresh off.
- **Enable automatic refresh** reads both quotas immediately, then uses the
  **Refresh interval (seconds)**. The default is 60 seconds, and the interval
  accepts whole seconds from 15 to 86,400.

If you previously enabled the startup checkbox on an individual button, enable
it once in the shared settings.

On upgrading from the initial version, remove and re-add one quota action to
expose the new inspector: OpenDeck stores the old inspector path in existing
action instances. The shared settings still apply to your other existing keys.

Periodic refresh waits the configured interval after each completed probe.
A manual press also restarts that interval.
Polling pauses when no quota buttons are visible and resumes when one returns.
While automatic refresh is enabled, the next reported session or weekly reset
also triggers a shared refresh, even if it occurs before the configured interval.
Reset times already in the past are not retriggered after a successful read.
Uncheck automatic refresh and save to stop future scheduled probes; any probe
already running finishes and exits. Manual presses always work. One shared timer
and probe serve every action instance, including both quota windows.

After a failed probe, automatic retries wait twice the configured interval,
then four times, and so on, capped at 15 minutes (or the configured interval
when it is longer). An overdue reset never bypasses this backoff. A successful
read restores the normal interval and updates the reset schedule. Pressing a key
always bypasses a pending retry delay, while still preventing overlapping probes.

The **Quota status** section in either inspector shows the last successful update
with its local timestamp and age, any actionable refresh error, and the next
automatic attempt, including retry backoff. Failed reads preserve the timestamp
and cached quotas so their age remains visible. The status is shared by both
buttons and uses runtime messages; reading status never starts app-server.
Error messages are fixed, sanitized descriptions for missing executable, login,
unsupported account, timeout, malformed response and other failures. Raw account
errors are not forwarded to the inspector.

The plugin loads the last successful reading on startup and shares it across
all quota keys. While the next refresh runs, keys keep showing their last
known values with an **Updating** label. Without a reading, a key shows
**Press key** while idle and **Reading…** during a refresh.

Each successful refresh atomically replaces one shared JSON file at
`$XDG_CACHE_HOME/opendeck-codex-limits/quota.json` (by default
`~/.cache/opendeck-codex-limits/quota.json`). The file holds both quota windows,
the banked reset count, and the last successful read time. Failed reads and
countdown updates leave the file alone.

Quota keys show percentage remaining, a depleting circle, the reported window duration, time until
reset, and a `↻` banked-reset count (`?` means unavailable). Assets use a 144 × 144
canvas for 72 × 72 physical keys, with a 36–40px central number, 24px countdown,
and 23px reset badge. The top-left duration (for example `5h` or `7d`) identifies
the quota window. Pressing a key only
reads quota; it never redeems resets. Countdown updates locally every 15 seconds.
When a reset time passes, **Reset due** prompts a new reading; quota is never
assumed to have replenished. Failures retain previous values marked **Stale**,
or show **Unavailable** when there has been no successful reading.

To check the Codex connection without OpenDeck:

```sh
cargo run --locked -- --probe
```

This prints normalized quota data, not credentials. The probe starts
`codex app-server`, performs `initialize` / `initialized`, then calls
`account/rateLimits/read` with `params: null`. It prefers the `codex` entry in
`rateLimitsByLimitId`, falling back to legacy `rateLimits`. The reset count comes
from the response-level `rateLimitResetCredits.availableCount`. Probes share a cache,
coalesce overlapping refreshes and have a 30-second timeout. Each closes stdin
and waits up to two seconds before forcing termination and reaping the process.
No conversation or generation is started. Plugin errors go to OpenDeck's plugin
log; app-server stderr is discarded to avoid logging sensitive diagnostics.

The scheduler has virtual-time tests for intervals, disable/enable, visibility,
backoff, reset deadlines and probe lifecycle. Protocol integration tests exercise
real reset-time polling, status delivery, quota restoration across plugin
restarts, and manual refresh during backoff.
To exercise two settings inspectors in headless Google Chrome (installed at
`/usr/bin/google-chrome`):

```sh
uv run --with playwright --with websockets python tests/inspector.py
```

Physical-device validation is tracked in Beads under `ocl-z32.3`.
Flatpak is outside the current scope.

## License

The plugin is licensed under the [MIT License](LICENSE). See
[Third-Party Notices](THIRD_PARTY_NOTICES.txt) for attribution.
