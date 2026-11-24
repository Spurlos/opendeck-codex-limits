#!/usr/bin/env python3
"""Small strict app-server substitute; no credentials or network access."""
import json
import os
import sys
import time
from pathlib import Path

assert sys.argv[1:] == ["app-server"]
marker = Path(os.environ.get("PROBE_MARKER", Path(__file__).with_suffix(".pid")))
with marker.open("a") as log:
    log.write(f"{os.getpid()}\n")

mode = os.environ.get("PROBE_MODE", "success")
if mode == "hang":
    time.sleep(60)

def read():
    return json.loads(sys.stdin.readline())

def send(value):
    print(json.dumps(value), flush=True)

initialize = read()
assert initialize["method"] == "initialize"
send({"id": initialize["id"], "result": {}})
assert read()["method"] == "initialized"
request = read()
assert request == {"id": 2, "method": "account/rateLimits/read", "params": None}
time.sleep(0.2)
send({"method": "account/rateLimits/updated", "params": {}})
if mode == "error" or (mode == "fail-second" and len(marker.read_text().splitlines()) > 1):
    send({"id": 2, "error": {"code": -1, "message": "private diagnostic"}})
else:
    primary_reset = int(time.time()) + 7200
    if mode == "reset":
        reset_file = marker.with_suffix(".reset")
        if not reset_file.exists():
            reset_file.write_text(str(int(time.time()) + 2))
        primary_reset = int(reset_file.read_text())
    send({"id": 2, "result": {"rateLimitsByLimitId": {"codex": {
        "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": primary_reset},
        "secondary": {"usedPercent": 60, "windowDurationMins": 10080, "resetsAt": int(time.time()) + 86400}
    }}, "rateLimitResetCredits": {"availableCount": 3}}})
assert sys.stdin.read() == ""  # Must close stdin after the request.
if mode == "linger":
    time.sleep(60)
