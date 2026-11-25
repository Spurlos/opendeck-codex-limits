"""Inspector integration check: uv run --with playwright --with websockets python tests/inspector.py.

Uses an installed Google Chrome and a local OpenAction settings socket substitute.
"""
import asyncio
import json
import time
from pathlib import Path

from playwright.async_api import async_playwright
from websockets.asyncio.server import serve


async def main():
    settings = {}
    clients = set()
    saves = []
    messages = []
    diagnostics = {"type": "quotaStatus", "lastSuccessAt": None, "refreshing": False,
                   "errorCode": None, "errorMessage": None, "consecutiveFailures": 0,
                   "nextRefreshAt": None, "autoRefreshEnabled": False, "hasVisibleButtons": True}

    async def send_diagnostics(socket):
        await socket.send(json.dumps({"event": "sendToPropertyInspector", "payload": diagnostics}))

    async def broadcast_diagnostics():
        for client in tuple(clients):
            await send_diagnostics(client)

    async def handler(socket):
        nonlocal settings
        clients.add(socket)
        context = None
        try:
            async for raw in socket:
                message = json.loads(raw)
                messages.append(message)
                if message["event"] == "registerPropertyInspector":
                    context = message["uuid"]
                elif message["event"] == "getGlobalSettings":
                    assert message["context"] == context
                    await socket.send(json.dumps({"event": "didReceiveGlobalSettings", "payload": {"settings": settings}}))
                elif message["event"] == "setGlobalSettings":
                    assert message["context"] == context
                    settings = message["payload"]
                    saves.append(settings)
                    diagnostics["autoRefreshEnabled"] = settings["autoRefreshEnabled"]
                    for client in tuple(clients):
                        await client.send(json.dumps({"event": "didReceiveGlobalSettings", "payload": {"settings": settings}}))
                        await send_diagnostics(client)
                elif message["event"] == "sendToPlugin":
                    assert message["payload"] == {"type": "getStatus"}
                    await send_diagnostics(socket)
        finally:
            clients.remove(socket)

    async with serve(handler, "127.0.0.1", 0) as server, async_playwright() as playwright:
        browser = await playwright.chromium.launch(executable_path="/usr/bin/google-chrome", headless=True)
        try:
            port = server.sockets[0].getsockname()[1]
            pages = []
            errors = []

            async def connect(page, context):
                await page.goto((Path(__file__).resolve().parents[1] / "assets/inspector/index.html").as_uri())
                await page.evaluate("args => window.connectElgatoStreamDeckSocket(...args)", [port, context, "registerPropertyInspector"])
                await page.get_by_role("button", name="Save for all buttons").wait_for()
                await page.wait_for_function("!document.getElementById('controls').disabled")

            for context in ["session-key", "weekly-key"]:
                page = await browser.new_page(viewport={"width": 350, "height": 650})
                page.on("pageerror", lambda error: errors.append(str(error)))
                await connect(page, context)
                pages.append(page)
            first, second = pages
            startup_label = "Refresh once on startup"
            assert not await first.get_by_label(startup_label).is_checked()
            await first.get_by_label(startup_label).check()
            assert not await second.get_by_label(startup_label).is_checked()
            await first.locator("#last-success").filter(has_text="Never").wait_for()
            assert not await first.get_by_label("Enable automatic refresh").is_checked()
            assert await first.get_by_label("Refresh interval (seconds)").is_disabled()
            await first.get_by_label("Enable automatic refresh").check()
            await first.get_by_label("Refresh interval (seconds)").fill("75")
            assert saves == [], "editing must not save before explicit submission"
            await first.get_by_role("button", name="Save for all buttons").click()
            await first.get_by_role("status").filter(has_text="Saved for all quota buttons.").wait_for()
            await second.wait_for_function("document.getElementById('startup').checked && document.getElementById('enabled').checked && document.getElementById('interval').value === '75'")
            assert saves == [{"refreshOnStartup": True, "autoRefreshEnabled": True, "refreshIntervalSeconds": 75}]
            updated_at = int(time.time()) - 120
            diagnostics.update(lastSuccessAt=updated_at, nextRefreshAt=int(time.time()) + 60)
            await broadcast_diagnostics()
            await first.wait_for_function("document.getElementById('last-success').hasAttribute('datetime')")
            assert "ago" in await first.locator("#reading-age").inner_text()
            # Diagnostic broadcasts must not overwrite an unfinished settings edit.
            await first.get_by_label("Refresh interval (seconds)").fill("99")
            diagnostics.update(errorCode="loginRequired", errorMessage="Codex login required. Run codex login, then refresh again.",
                               consecutiveFailures=2, nextRefreshAt=int(time.time()) + 240)
            await broadcast_diagnostics()
            await first.locator("#refresh-error").filter(has_text="Codex login required").wait_for()
            await second.locator("#refresh-error").filter(has_text="Codex login required").wait_for()
            assert "last successful reading" in await first.locator("#refresh-state").inner_text()
            assert "Retry backoff after 2 failed attempts" in await first.locator("#next-refresh").inner_text()
            assert await first.get_by_label("Refresh interval (seconds)").input_value() == "99"
            assert await first.locator("#last-success").get_attribute("datetime") is not None
            diagnostics.update(errorCode=None, errorMessage=None, consecutiveFailures=0, lastSuccessAt=int(time.time()))
            await broadcast_diagnostics()
            await first.wait_for_function("document.getElementById('refresh-error').textContent === ''")
            await connect(first, "session-key")  # Reopening restores the shared setting.
            assert await first.get_by_label(startup_label).is_checked()
            assert await first.get_by_label("Enable automatic refresh").is_checked()
            assert await first.get_by_label("Refresh interval (seconds)").input_value() == "75"
            await first.get_by_label("Refresh interval (seconds)").fill("0")
            await first.get_by_role("button", name="Save for all buttons").click()
            assert await first.get_by_label("Refresh interval (seconds)").get_attribute("aria-invalid") == "true"
            assert len(saves) == 1, "invalid interval must not be submitted"
            await second.get_by_label("Enable automatic refresh").uncheck()
            await second.get_by_role("button", name="Save for all buttons").click()
            await first.wait_for_function("!document.getElementById('enabled').checked")
            assert saves[-1] == {"refreshOnStartup": True, "autoRefreshEnabled": False, "refreshIntervalSeconds": 75}
            assert await first.get_by_label("Refresh interval (seconds)").is_disabled()
            await first.locator("#next-refresh").filter(has_text="Periodic refresh is off").wait_for()
            await second.get_by_label(startup_label).uncheck()
            await second.get_by_role("button", name="Save for all buttons").click()
            await first.wait_for_function("!document.getElementById('startup').checked")
            assert saves[-1] == {"refreshOnStartup": False, "autoRefreshEnabled": False, "refreshIntervalSeconds": 75}
            assert errors == [], errors
            assert all(m["context"] in ["session-key", "weekly-key"] for m in messages if "context" in m)
            print("Inspector checks passed: shared startup and periodic settings, validation, freshness, errors, backoff and recovery.")
        finally:
            await browser.close()


asyncio.run(main())
