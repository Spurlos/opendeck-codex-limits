# Codex Limits for OpenDeck

See how much of your Codex session and weekly quota remains on your Stream
Deck. The plugin uses your existing Codex CLI login and adds two key actions
to [OpenDeck](https://github.com/nekename/OpenDeck).

## Install

The release ZIP targets Linux x86-64 with glibc 2.36 or newer. You need a
native OpenDeck installation and a signed-in `codex` CLI on OpenDeck's
`PATH`. To build for your own Linux system, see [Build from source](#build-from-source).

1. Download `io.github.spurlos.codexlimits-linux-x86_64.zip` from
   [GitHub Releases](https://github.com/Spurlos/opendeck-codex-limits/releases).
2. Install the ZIP through OpenDeck's plugin manager.
3. Add **Session Quota** or **Weekly Quota** from the **Codex Limits**
   category to a key. Press either key to refresh both quota windows.

## Use the quota keys

Each key shows the percentage remaining, the quota window (for example,
`5h` or `7d`), and the time until it resets. The `↻` badge shows how many
reset credits are available when Codex reports them. Pressing a key only
reads quota information.

To enable automatic updates, select either action in OpenDeck, choose
your settings, and click **Save for all buttons**. Both options are off
by default.

- **Refresh once on startup** reads both quotas when the first quota key
  appears after OpenDeck starts.
- **Enable automatic refresh** checks both quotas at the selected interval
  while a quota key is visible. The default is 60 seconds, with a range of
  15 to 86,400 seconds. A reported quota reset can trigger an earlier check.

## Troubleshooting

- If a key shows **Press key**, press it to load the first reading.
  **Reset due** means the reported reset time has passed. Press the key
  for an updated reading.
- If a key shows **Stale** or **Unavailable**, open **Quota status** in
  the key settings to see the last successful update, any refresh error,
  and the next automatic attempt. Check that OpenDeck can find the
  signed-in `codex` CLI and inherits `CODEX_HOME` if you use a custom one.

The plugin keeps the last successful reading if a refresh fails and
restores it across restarts from `~/.cache/opendeck-codex-limits/quota.json`
or the corresponding path under `$XDG_CACHE_HOME`.

## Build from source

On Linux, install Rust through Rustup and install `jq`. The repository
selects the Rust toolchain. Run this from the repository root.

```sh
./scripts/package.sh
```

In OpenDeck settings, choose **Open config directory**. Copy
`dist/io.github.spurlos.codexlimits.sdPlugin` into its `plugins` directory
and restart OpenDeck.

Run the local checks before submitting a change.

```sh
cargo test --locked
./scripts/lint.sh
```

To check the Codex connection from a source checkout, run
`cargo run --locked -- --probe`. It prints quota data.

## License

[MIT](LICENSE). See [Third-Party Notices](THIRD_PARTY_NOTICES.txt) and
[Rust Dependency Licenses](RUST_DEPENDENCY_LICENSES.txt) for bundled
attributions.
