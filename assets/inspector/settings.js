"use strict";

const form = document.getElementById("refresh-settings");
const controls = document.getElementById("controls");
const enabled = document.getElementById("enabled");
const interval = document.getElementById("interval");
const status = document.getElementById("status");
const startup = document.getElementById("startup");
let socket;
let context;
let current = {};
let pending = false;
let saveTimeout;
let diagnostics;
let lastAnnouncedError = null;
const refreshState = document.getElementById("refresh-state");
const refreshError = document.getElementById("refresh-error");
const lastSuccess = document.getElementById("last-success");
const readingAge = document.getElementById("reading-age");
const nextRefresh = document.getElementById("next-refresh");

function age(seconds) {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}

function renderDiagnostics() {
  if (!diagnostics) return;
  const now = Math.floor(Date.now() / 1000);
  const success = diagnostics.lastSuccessAt;
  if (Number.isFinite(success) && success > 0) {
    const date = new Date(success * 1000);
    lastSuccess.dateTime = date.toISOString();
    lastSuccess.textContent = date.toLocaleString();
    readingAge.textContent = `(${age(Math.max(0, now - success))} ago)`;
  } else {
    lastSuccess.removeAttribute("datetime");
    lastSuccess.textContent = "Never";
    readingAge.textContent = "";
  }
  refreshState.textContent = diagnostics.refreshing ? "Refreshing both quotas…"
    : diagnostics.errorCode ? (success ? "Showing the last successful reading." : "No quota reading available yet.")
    : success ? "Quota reading available." : "Press a quota key to read quota now.";
  // Only errors use a live region. Age/countdown ticks and brief loading states
  // stay readable without repeatedly interrupting screen-reader speech.
  const error = diagnostics.errorMessage ?? "";
  if (error !== lastAnnouncedError) {
    refreshError.textContent = error;
    lastAnnouncedError = error;
  }
  const failures = diagnostics.consecutiveFailures ?? 0;
  const retry = failures ? ` Retry backoff after ${failures} failed ${failures === 1 ? "attempt" : "attempts"}.` : "";
  nextRefresh.textContent = !diagnostics.autoRefreshEnabled ? "Periodic refresh is off."
    : !diagnostics.hasVisibleButtons ? "Automatic refresh is paused while no quota buttons are visible."
    : diagnostics.refreshing ? "The next automatic refresh is scheduled after this attempt finishes."
    : Number.isFinite(diagnostics.nextRefreshAt) ? `Next automatic refresh in ${age(Math.max(0, diagnostics.nextRefreshAt - now))}.${retry}`
    : "Waiting for the next automatic refresh.";
}

function send(event, payload) {
  socket.send(JSON.stringify({ event, context, ...(payload === undefined ? {} : { payload }) }));
}

function updateEnabled() {
  interval.disabled = !enabled.checked;
  interval.setAttribute("aria-invalid", "false");
}

// OpenDeck invokes this Stream Deck-compatible entrypoint after loading the page.
window.connectElgatoStreamDeckSocket = (port, uuid, registerEvent) => {
  context = uuid; // Inspector requests use the action instance context, not plugin UUID.
  socket = new WebSocket(`ws://127.0.0.1:${port}`);
  socket.addEventListener("open", () => {
    socket.send(JSON.stringify({ event: registerEvent, uuid }));
    send("getGlobalSettings");
    send("sendToPlugin", { type: "getStatus" });
  });
  socket.addEventListener("message", (message) => {
    let event;
    try { event = JSON.parse(message.data); } catch { return; }
    if (event.event === "sendToPropertyInspector" && event.payload?.type === "quotaStatus") {
      diagnostics = event.payload;
      renderDiagnostics();
      return;
    }
    if (event.event !== "didReceiveGlobalSettings") return;
    current = event.payload?.settings ?? {};
    startup.checked = current.refreshOnStartup === true;
    enabled.checked = current.autoRefreshEnabled === true;
    interval.value = String(current.refreshIntervalSeconds ?? 60);
    controls.disabled = false;
    updateEnabled();
    clearTimeout(saveTimeout);
    status.textContent = pending ? "Saved for all quota buttons." : "Settings shared by all quota buttons.";
    pending = false;
  });
  const disconnected = () => {
    controls.disabled = true;
    pending = false;
    clearTimeout(saveTimeout);
    status.textContent = "Disconnected. Select the button again to reconnect.";
    diagnostics = undefined;
    refreshState.textContent = "Disconnected; quota status is no longer updating.";
    nextRefresh.textContent = "Reconnect to see the next scheduled refresh.";
  };
  socket.addEventListener("close", disconnected);
  socket.addEventListener("error", disconnected);
};

enabled.addEventListener("change", updateEnabled);
interval.addEventListener("invalid", () => {
  interval.setAttribute("aria-invalid", "true");
  status.textContent = "Enter a whole number from 15 to 86,400 seconds.";
});

// Update display age locally; this never requests quota or starts app-server.
setInterval(renderDiagnostics, 1000);
interval.addEventListener("input", () => {
  if (interval.validity.valid) interval.setAttribute("aria-invalid", "false");
});

form.addEventListener("submit", (event) => {
  event.preventDefault();
  if (!socket || socket.readyState !== WebSocket.OPEN || pending) return;
  // Disabled manual-mode interval inputs retain their last saved, valid value.
  const seconds = enabled.checked ? interval.valueAsNumber : (current.refreshIntervalSeconds ?? 60);
  if (!Number.isInteger(seconds) || seconds < 15 || seconds > 86400) return;
  pending = true;
  controls.disabled = true;
  status.textContent = "Saving…";
  send("setGlobalSettings", {
    ...current,
    refreshOnStartup: startup.checked,
    autoRefreshEnabled: enabled.checked,
    refreshIntervalSeconds: seconds,
  });
  // Plugin persists validated settings and broadcasts them to every inspector.
  saveTimeout = setTimeout(() => {
    pending = false;
    controls.disabled = false;
    updateEnabled();
    status.textContent = "Save was not confirmed. Try again or reopen the settings.";
  }, 5000);
});
