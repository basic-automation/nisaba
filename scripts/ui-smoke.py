#!/usr/bin/env python3
"""UI smoke test: drive the real Nisaba desktop app over WebDriver (Linux).

    cargo build -p nisaba-tauri --bin nisaba-tauri --example ui_fixture
    scripts/ui-smoke.py --target-dir "$(cargo metadata --format-version 1 | jq -r .target_directory)"

What it does, and why it is safe to run on a machine with a real install:

* Seeds a throwaway install (`ui_fixture` example) in a fresh temp directory and
  launches the app from there -- the app reads `./config.toml` before anything else, so
  it never sees a real config or database. Every platform is disabled in that config.
* Runs the whole thing inside an unprivileged network namespace (`unshare -rn`) with only
  loopback up. Nothing the app starts -- adapters, the P2P onion service, a vendor
  plugin -- can reach the network, so it cannot touch live marketplace data.
* Drives the app through tauri-driver + WebKitWebDriver and asserts on what the
  marketplace page renders for each plugin's network access.

Needs: tauri-driver (`cargo install tauri-driver`), WebKitWebDriver (webkitgtk), a
Wayland or X11 session, and unprivileged user namespaces. Stdlib only.
"""

import argparse
import http.client
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

IN_NETNS = "NISABA_UI_SMOKE_NETNS"
PORT = 4444
NATIVE_PORT = 4445

# marketplace tab -> [(plugin display name, what its card must say about network access)]
EXPECTED = {
    "Available": [
        ("Fixture Restricted", "api.example.com, *.cdn.example.com"),
        ("Fixture Unrestricted", "Unrestricted"),
        ("Fixture From Peer", "Checked when installed"),
    ],
    "Installed": [
        ("Fixture Offline", "No network access"),
    ],
}


def webdriver(method, path, body=None, timeout=60):
    """One WebDriver call, retrying transient connection drops.

    The tauri-driver connection drops intermittently at arbitrary calls
    (run #166 died on a window-switch POST, run #168 on the first tab
    click's execute/sync) — a single dropped connection must not kill the
    whole run. Retry connection-level failures a few times; an HTTP error
    response is a real signal, not a transient, so it still fails at once.
    Retried scripts are all idempotent here (reads, same-tab clicks,
    same-route pushes), so a re-run after a drop is safe.
    """
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}{path}",
        data=data,
        method=method,
        headers={"Content-Type": "application/json"},
    )
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.load(resp)["value"]
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{method} {path}: {e.code} {e.read().decode(errors='replace')}")
        except (http.client.RemoteDisconnected, ConnectionError, urllib.error.URLError) as e:
            if attempt == 3:
                raise
            print(f"webdriver retry {method} {path} after {type(e).__name__}", file=sys.stderr)
            time.sleep(1)


def run_js(session, script):
    return webdriver("POST", f"/session/{session}/execute/sync", {"script": script, "args": []})


def run_js_idempotent(session, script, attempts=8):
    """run_js for scripts that are safe to repeat (same-tab clicks, same-route pushes).

    WebKitWebDriver sometimes answers an execute/sync with an empty-message
    500 ("unknown error") right after the wizard window is closed, and a
    retry a moment later succeeds. webdriver() deliberately does not retry
    HTTP errors, so retry them here, and only here, where repeating is safe.
    """
    for attempt in range(attempts):
        try:
            return run_js(session, script)
        except RuntimeError as e:
            if attempt == attempts - 1:
                raise
            print(f"execute/sync retry {attempt + 1}/{attempts - 1} after: {str(e)[:200]}", file=sys.stderr)
            time.sleep(1.5)


def wait_for(what, predicate, timeout=60):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = predicate()
            if last:
                return last
        except RuntimeError as e:
            last = e
        time.sleep(0.5)
    raise TimeoutError(f"timed out waiting for {what}; last saw: {last!r}"[:2000])


def wait_for_port(port, timeout=30):
    wait_for(f"port {port}", lambda: socket.socket().connect_ex(("127.0.0.1", port)) == 0, timeout)


def x11_screenshot(path):
    """Capture the main app window via X11, bypassing WebDriver screenshots.

    WebKitWebDriver's screenshot endpoint hangs in this environment, and its
    window-switch call drops the connection outright, so capture pixels
    directly: find the main window in the X tree by its exact native title
    ("Nisaba - Inventory Sync"), raise it, and grab it with ImageMagick.
    Fails loudly (never hangs) when the window or the tools are missing.
    """
    tree = subprocess.run(["xwininfo", "-root", "-tree"], capture_output=True, text=True)
    m = re.search(r'(0x[0-9a-f]+)\s+"Nisaba - Inventory Sync"', tree.stdout)
    if not m:
        raise RuntimeError(f"could not find main Nisaba window in:\n{tree.stdout}\n{tree.stderr}")
    win_id = m.group(1)
    subprocess.run(["xdotool", "windowraise", win_id], check=True)
    time.sleep(0.5)  # let the restack take effect before grabbing pixels
    subprocess.run(["import", "-window", win_id, path], check=True)


def smoke(app, fixture, workdir, screenshot, seed_catalog=False):
    # Loopback is down in a fresh namespace.
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    fixture_args = [fixture, workdir] + (["--with-sample-catalog"] if seed_catalog else [])
    out = subprocess.run(fixture_args, check=True, capture_output=True, text=True)
    print(f"fixture: {out.stdout.strip()}")

    # WebKitGTK's DMA-BUF renderer dies with "Error 71 (Protocol error) dispatching to
    # Wayland display" on some compositors; the fallback renders the same page.
    env = dict(os.environ, WEBKIT_DISABLE_DMABUF_RENDERER="1")
    driver = subprocess.Popen(
        [shutil.which("tauri-driver"), "--port", str(PORT), "--native-port", str(NATIVE_PORT)],
        cwd=workdir,  # the app inherits it, and so loads the fixture's ./config.toml
        env=env,
        stdout=open(os.path.join(workdir, "tauri-driver.log"), "w"),
        stderr=subprocess.STDOUT,
    )
    session = None
    try:
        wait_for_port(PORT)
        session = webdriver(
            "POST",
            "/session",
            {"capabilities": {"alwaysMatch": {"tauri:options": {"application": app}}}},
            timeout=120,
        )["sessionId"]

        # Wait for the Nuxt app to mount, then drive its router directly.
        wait_for(
            "the app to mount",
            lambda: run_js(
                session,
                "return !!(document.querySelector('#__nuxt') && "
                "document.querySelector('#__nuxt').__vue_app__)",
            ),
        )
        # A company with no platforms sends the app to the setup wizard on
        # launch. Let that redirect land first so it cannot overtake the push.
        deadline = time.time() + 8
        while time.time() < deadline and run_js(
            session,
            "return document.querySelector('#__nuxt').__vue_app__"
            ".config.globalProperties.$router.currentRoute.value.path",
        ) != "/setup":
            time.sleep(0.5)
        run_js_idempotent(
            session,
            "document.querySelector('#__nuxt').__vue_app__"
            ".config.globalProperties.$router.push('/marketplace')",
        )
        failures = []
        checks = 0
        for tab, cards in EXPECTED.items():
            run_js_idempotent(
                session,
                "const b = [...document.querySelectorAll('button')]"
                f".find(e => e.textContent.trim() === {json.dumps(tab)}); if (b) b.click();",
            )
            seen = {}

            def rendered(cards=cards):
                seen["page"] = run_js(session, "return location.pathname + '\\n' + document.body.innerText")
                return all(name in seen["page"] for name, _ in cards)

            try:
                wait_for(f"the {tab} cards", rendered)
            except TimeoutError:
                print(f"page was:\n{seen.get('page', '')[:3000]}", file=sys.stderr)
                raise

            for name, expected in cards:
                card = run_js(
                    session,
                    "const h = [...document.querySelectorAll('h3')]"
                    f".find(e => e.textContent.trim() === {json.dumps(name)});"
                    "let c = h; while (c && !c.innerText.includes('Network:')) c = c.parentElement;"
                    "return c ? c.innerText : null;",
                )
                lines = (card or "").splitlines()
                # The badge renders "Network:" and its value as separate lines.
                network = next(
                    (" ".join(lines[i : i + 2]) for i, l in enumerate(lines) if l.startswith("Network:")),
                    None,
                )
                ok = network is not None and expected in network
                checks += 1
                print(f"{'ok  ' if ok else 'FAIL'} [{tab}] {name}: {network!r}")
                if not ok:
                    failures.append(name)

            if screenshot:
                stem, ext = os.path.splitext(screenshot)
                path = f"{stem}-{tab.lower()}{ext or '.png'}"
                x11_screenshot(path)
                print(f"screenshot: {path}")

        # N-12.5: README screenshots of the main views, captured from the real app
        # running on the fixture install (so they never show real customer data).
        # With --screenshot out.png this saves out-dashboard.png, out-products.png,
        # out-listings.png, out-vendors.png and out-inventory.png next to the
        # per-marketplace-tab shots.
        if screenshot:
            stem, ext = os.path.splitext(screenshot)
            router = (
                "document.querySelector('#__nuxt').__vue_app__"
                ".config.globalProperties.$router"
            )
            for route, shot in [
                ("/", "dashboard"),
                ("/products", "products"),
                ("/listings", "listings"),
                ("/vendors", "vendors"),
                ("/inventory", "inventory"),
            ]:
                run_js(session, f"{router}.push({json.dumps(route)})")
                wait_for(
                    f"the {shot} view",
                    lambda route=route: run_js(session, "return location.pathname") == route,
                )
                time.sleep(2)  # let the view finish rendering
                path = f"{stem}-{shot}{ext or '.png'}"
                x11_screenshot(path)
                print(f"screenshot: {path}")

        # The views loop above left the app on /inventory; the time-limit check
        # needs the marketplace Installed tab, so navigate back first. (Without
        # --screenshot the app is already there; the push and click are no-ops.)
        back_router = (
            "document.querySelector('#__nuxt').__vue_app__"
            ".config.globalProperties.$router"
        )
        run_js(session, f"{back_router}.push('/marketplace')")
        wait_for("the marketplace again", lambda: "Installed" in run_js(session, "return document.body.innerText"))
        run_js(
            session,
            "const b = [...document.querySelectorAll('button')]"
            ".find(e => e.textContent.trim() === 'Installed'); if (b) b.click();",
        )

        # The installed plugin's time limit: the fixture's 2 h renders, and changing it
        # through the <select> round-trips through set_vendor_plugin_timeout.
        selected = "const s = document.querySelector('#timeout-offline'); return s ? s.selectedOptions[0].textContent.trim() : null;"
        checks += 1
        shown = run_js(session, selected)
        print(f"{'ok  ' if shown == '2 h' else 'FAIL'} [Installed] time limit shows {shown!r}")
        if shown != "2 h":
            failures.append("time limit (initial)")
        run_js(
            session,
            "const s = document.querySelector('#timeout-offline'); s.value = '60';"
            "s.dispatchEvent(new Event('change'));",
        )
        # A failed save would leave the DOM showing "1 h" anyway (the bound prop never
        # changed, so Vue never re-renders it), so leave the page and come back: the
        # value shown then was read back from the database.
        time.sleep(1)
        router = "document.querySelector('#__nuxt').__vue_app__.config.globalProperties.$router"
        run_js(session, f"{router}.push('/')")
        wait_for("leaving the marketplace", lambda: run_js(session, "return location.pathname") == "/")
        run_js(session, f"{router}.push('/marketplace')")
        wait_for("the marketplace again", lambda: "Installed" in run_js(session, "return document.body.innerText"))
        run_js(
            session,
            "const b = [...document.querySelectorAll('button')]"
            ".find(e => e.textContent.trim() === 'Installed'); if (b) b.click();",
        )
        checks += 1
        try:
            wait_for("the saved time limit", lambda: run_js(session, selected) == "1 h", timeout=15)
            print("ok   [Installed] time limit saved as '1 h' (re-read after navigating away)")
        except TimeoutError as e:
            print(f"FAIL [Installed] time limit change: {e}")
            failures.append("time limit (change)")

        if failures:
            raise SystemExit(f"{len(failures)} card(s) wrong: {', '.join(failures)}")
        print(f"ui-smoke: {checks} checks passed")
    finally:
        if session:
            try:
                webdriver("DELETE", f"/session/{session}", timeout=30)
            except Exception as e:  # the app may already be gone
                print(f"closing session: {e}", file=sys.stderr)
        driver.terminate()
        driver.wait(timeout=30)


# The webview sometimes dies about ten seconds after launch ("session deleted because of page
# crash or hang"), on a runner and a commit that pass on the next try. Retrying single calls
# (run_js_idempotent) cannot revive a dead session, so the whole app is restarted instead.
APP_ATTEMPTS = 3


def is_lost_session(error):
    """Whether `error` means the webview died, not that a check failed."""
    return isinstance(error, RuntimeError) and "invalid session id" in str(error)


def run_with_restarts(run_once, attempts=APP_ATTEMPTS, cleanup=lambda: None, pause=2):
    """Run `run_once(attempt)`, restarting it when the webview session is lost.

    Only a lost session restarts it. A failed check, a missing build or a crash that
    repeats on every attempt still fails the run.
    """
    for attempt in range(1, attempts + 1):
        try:
            return run_once(attempt)
        except RuntimeError as e:
            if not is_lost_session(e) or attempt == attempts:
                raise
            print(f"webview session lost (attempt {attempt}/{attempts}); restarting the app",
                  file=sys.stderr)
            cleanup()
            time.sleep(pause)


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--target-dir", required=True, help="cargo target directory")
    p.add_argument("--profile", default="debug")
    p.add_argument("--workdir", help="where to seed the throwaway install (default: a temp dir)")
    p.add_argument("--screenshot", help="save a PNG per marketplace tab plus one per main view "
                     "(dashboard, products, listings, vendors, inventory), e.g. out.png -> "
                     "out-available.png, out-dashboard.png, ...")
    p.add_argument("--seed-sample-catalog", action="store_true",
                   help="seed the fixture install with a small fictional product catalog "
                        "(for README screenshots; never real customer data)")
    args = p.parse_args()

    if os.environ.get(IN_NETNS) != "1":
        # Re-run ourselves with no network at all.
        env = dict(os.environ, **{IN_NETNS: "1"})
        os.execvpe("unshare", ["unshare", "-rn", sys.executable, *sys.argv], env)

    # Resolve to an absolute path up front: the script later runs tauri-driver with
    # cwd set to the temp fixture dir, and a relative path would make WebKitWebDriver
    # look for target/debug/nisaba-tauri there instead of under the repo root.
    target_dir = os.path.abspath(args.target_dir)
    base = os.path.join(target_dir, args.profile)
    app = os.path.join(base, "nisaba-tauri")
    fixture = os.path.join(base, "examples", "ui_fixture")
    for path in (app, fixture):
        if not os.path.exists(path):
            raise SystemExit(f"missing {path}; build it first (see the docstring)")
    if args.screenshot:
        os.makedirs(os.path.dirname(os.path.abspath(args.screenshot)), exist_ok=True)

    def run_once(attempt):
        # A fresh install per attempt: the dead one may be half-written. An explicit
        # --workdir is the caller's choice and is reused.
        workdir = args.workdir or tempfile.mkdtemp(prefix="nisaba-ui-smoke-")
        smoke(app, fixture, workdir, args.screenshot and os.path.abspath(args.screenshot),
              seed_catalog=args.seed_sample_catalog)

    def kill_leftover_app():
        # tauri-driver is stopped, but the app it launched can outlive it and hold its ports.
        subprocess.run(["pkill", "-f", app], check=False)

    run_with_restarts(run_once, cleanup=kill_leftover_app)


if __name__ == "__main__":
    main()
