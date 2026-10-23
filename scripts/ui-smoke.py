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
import base64
import json
import os
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
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}{path}",
        data=data,
        method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.load(resp)["value"]
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{method} {path}: {e.code} {e.read().decode(errors='replace')}")


def run_js(session, script):
    return webdriver("POST", f"/session/{session}/execute/sync", {"script": script, "args": []})


def pin_main_window(session):
    """Pin the WebDriver session to the main app window.

    N-11.1 moved the setup wizard into a dedicated settings window ("Nisaba —
    Platform Setup"), so the main window ("Nisaba - Inventory Sync") no longer
    navigates to /setup on launch and the session can no longer assume which
    window it drives. Re-pin before each drive section in case the session
    followed focus elsewhere.
    """

    def pinned():
        try:
            handles = webdriver("GET", f"/session/{session}/window/handles")
        except RuntimeError:
            # Driver doesn't expose window handles; trust the current window
            # when its title already identifies the main window.
            return "Inventory Sync" in run_js(session, "return document.title || ''")
        for handle in handles:
            webdriver("POST", f"/session/{session}/window", {"handle": handle})
            if "Inventory Sync" in run_js(session, "return document.title || ''"):
                return True
        return False

    wait_for("the main window", pinned)


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

        # The main window no longer navigates to /setup on launch (the wizard lives
        # in the dedicated settings window since N-11.1). Pin the session to the
        # main window, wait for the Nuxt app to mount, then drive its router.
        pin_main_window(session)
        wait_for(
            "the app to mount",
            lambda: run_js(
                session,
                "return !!(document.querySelector('#__nuxt') && "
                "document.querySelector('#__nuxt').__vue_app__)",
            ),
        )
        run_js(
            session,
            "document.querySelector('#__nuxt').__vue_app__"
            ".config.globalProperties.$router.push('/marketplace')",
        )
        failures = []
        checks = 0
        for tab, cards in EXPECTED.items():
            run_js(
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
                png = webdriver("GET", f"/session/{session}/screenshot")
                with open(path, "wb") as f:
                    f.write(base64.b64decode(png))
                print(f"screenshot: {path}")

        # N-12.5: README screenshots of the main views, captured from the real app
        # running on the fixture install (so they never show real customer data).
        # With --screenshot out.png this saves out-dashboard.png, out-products.png,
        # out-listings.png, out-vendors.png and out-inventory.png next to the
        # per-marketplace-tab shots.
        if screenshot:
            pin_main_window(session)
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
                png = webdriver("GET", f"/session/{session}/screenshot")
                path = f"{stem}-{shot}{ext or '.png'}"
                with open(path, "wb") as f:
                    f.write(base64.b64decode(png))
                print(f"screenshot: {path}")

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
    workdir = args.workdir or tempfile.mkdtemp(prefix="nisaba-ui-smoke-")
    if args.screenshot:
        os.makedirs(os.path.dirname(os.path.abspath(args.screenshot)), exist_ok=True)
    smoke(app, fixture, workdir, args.screenshot and os.path.abspath(args.screenshot),
          seed_catalog=args.seed_sample_catalog)


if __name__ == "__main__":
    main()
