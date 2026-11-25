#!/usr/bin/env python3
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
TARGET_BASE = Path(os.environ.get("CARGO_TARGET_DIR") or (ROOT / "target"))
BINARY = TARGET_BASE / "release" / "hqcode"
ASSETS_DIR = ROOT / "docs" / "assets"
CHROMIUM = shutil.which("chromium") or "/home/codes/third_party/browser-tools/bin/chromium"
PORT = 19901


def wait_for_health(url: str, proc: subprocess.Popen, timeout_secs: float = 15.0) -> None:
    import urllib.request

    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"hqcode server exited prematurely with {proc.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=1.0) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise TimeoutError("Timed out waiting for hqcode server /api/health")


def setup_sample_repo(ws: Path) -> None:
    subprocess.run(["git", "init", "-q"], cwd=ws, check=True)
    subprocess.run(["git", "config", "user.name", "HQCode Bot"], cwd=ws, check=True)
    subprocess.run(["git", "config", "user.email", "bot@hqcode.dev"], cwd=ws, check=True)
    (ws / "src").mkdir(parents=True, exist_ok=True)
    (ws / "README.md").write_text("# Demo Service\n\nPersistent agent workspace.\n")
    (ws / "src" / "router.rs").write_text(
        "pub fn route(path: &str) -> &'static str {\n"
        "    match path {\n"
        "        \"/health\" => \"ok\",\n"
        "        _ => \"not_found\",\n"
        "    }\n"
        "}\n"
    )
    subprocess.run(["git", "add", "-A"], cwd=ws, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "initial service"], cwd=ws, check=True)

    # Create clean realistic unstaged changes for Diff Review & screenshot.
    (ws / "src" / "router.rs").write_text(
        "pub fn route(path: &str) -> &'static str {\n"
        "    match path {\n"
        "        \"/health\" => \"ok\",\n"
        "        \"/ready\" => \"ready\",\n"
        "        \"/metrics\" => \"metrics\",\n"
        "        _ => \"not_found\",\n"
        "    }\n"
        "}\n"
    )


def main() -> None:
    assert BINARY.is_file(), f"Release binary not found: {BINARY}"
    ASSETS_DIR.mkdir(parents=True, exist_ok=True)

    tmp_root = Path(tempfile.mkdtemp(prefix="hqcode-e2e-"))
    data_dir = tmp_root / "data"
    ws_dir = tmp_root / "workspace"
    data_dir.mkdir(parents=True)
    ws_dir.mkdir(parents=True)
    setup_sample_repo(ws_dir)

    proc = subprocess.Popen(
        [
            str(BINARY),
            "serve",
            "--addr",
            f"127.0.0.1:{PORT}",
            "--data-dir",
            str(data_dir),
            "--workspace",
            str(ws_dir),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    created_session_ids = []
    try:
        wait_for_health(f"http://127.0.0.1:{PORT}/api/health", proc)

        pw_file = data_dir / ".hqcode-initial-password"
        deadline = time.time() + 5.0
        while not pw_file.exists() and time.time() < deadline:
            time.sleep(0.1)
        assert pw_file.exists(), "Initial password file was not created"
        pw_lines = pw_file.read_text().splitlines()
        password = next(l.split("=", 1)[1] for l in pw_lines if l.startswith("password="))

        page_errors = []
        console_errors = []

        with sync_playwright() as p:
            browser = p.chromium.launch(
                executable_path=CHROMIUM,
                headless=True,
                args=["--no-sandbox", "--disable-gpu"],
            )
            context = browser.new_context(
                viewport={"width": 1440, "height": 900},
                locale="en-US",
            )
            page = context.new_page()
            page.on("pageerror", lambda err: page_errors.append(str(err)))
            page.on(
                "console",
                lambda msg: console_errors.append(msg.text)
                if msg.type == "error"
                else None,
            )

            # 1. Login flow
            page.goto(f"http://127.0.0.1:{PORT}/", wait_until="networkidle")
            page.fill("#login-username", "hqcode")
            page.fill("#login-password", password)
            page.click("#login-form button[type='submit']")
            page.wait_for_selector("#app-view:not(.hidden)", timeout=10000)
            assert not pw_file.exists(), "Initial password file should be deleted after login"

            # 2. Launch persistent tmux shell & send marker command
            page.click("#btn-new-shell")
            page.wait_for_selector("#xterm-host:not(.hidden)", timeout=10000)
            page.wait_for_selector("#session-tabs .session-tab", timeout=10000)
            page.wait_for_selector("#conn-pill.live", timeout=10000)
            time.sleep(0.4)

            page.click("#xterm-host")
            page.keyboard.type("echo HQCODE_E2E_PERSIST_OK", delay=15)
            page.keyboard.press("Enter")
            page.wait_for_function(
                """() => {
                    const dom = document.querySelector('.xterm-rows')?.innerText || '';
                    const t = window.__hqTerm;
                    let bufText = '';
                    if (t?.buffer?.active) {
                        const b = t.buffer.active;
                        for (let i = 0; i < b.length; i++) {
                            bufText += (b.getLine(i)?.translateToString(true) || '') + '\\n';
                        }
                    }
                    return dom.includes('HQCODE_E2E_PERSIST_OK') || bufText.includes('HQCODE_E2E_PERSIST_OK');
                }""",
                timeout=10000,
            )

            # 3. Reload browser and verify tmux session backlog survives intact
            page.reload(wait_until="networkidle")
            page.wait_for_selector("#xterm-host:not(.hidden)", timeout=10000)
            page.wait_for_function(
                """() => {
                    const dom = document.querySelector('.xterm-rows')?.innerText || '';
                    const t = window.__hqTerm;
                    let bufText = '';
                    if (t?.buffer?.active) {
                        const b = t.buffer.active;
                        for (let i = 0; i < b.length; i++) {
                            bufText += (b.getLine(i)?.translateToString(true) || '') + '\\n';
                        }
                    }
                    return dom.includes('HQCODE_E2E_PERSIST_OK') || bufText.includes('HQCODE_E2E_PERSIST_OK');
                }""",
                timeout=10000,
            )

            # Record session IDs for cleanup verification
            sessions_resp = page.evaluate(
                "async () => (await (await fetch('/api/terminal/sessions')).json()).sessions"
            )
            created_session_ids = [s["id"] for s in sessions_resp]
            assert len(created_session_ids) >= 1

            # 3b. Two-click close confirmation prevents accidental session kill
            page.click("#btn-new-shell")
            page.wait_for_function(
                "async () => (await (await fetch('/api/terminal/sessions')).json()).sessions.length === 2",
                timeout=10000,
            )
            page.locator("#session-list .session-item .session-close-btn").nth(1).click()
            page.wait_for_selector("#session-list .session-item .session-close-btn.confirming", timeout=5000)
            after_first_click = page.evaluate(
                "async () => (await (await fetch('/api/terminal/sessions')).json()).sessions"
            )
            assert len(after_first_click) == 2, "First click on close button must not kill session"
            page.locator("#session-list .session-item .session-close-btn.confirming").click()
            page.wait_for_function(
                "async () => (await (await fetch('/api/terminal/sessions')).json()).sessions.length === 1",
                timeout=10000,
            )

            # 3c. Remote QR Connect modal + unauthenticated mobile pairing + Enter send + geometry + revoke
            page.click("#btn-open-connect")
            page.wait_for_selector("#connect-modal:not(.hidden)", timeout=5000)
            page.wait_for_selector("#connect-qr-svg svg", timeout=5000)
            page.wait_for_selector("#connect-loopback-warn:not(.hidden)", timeout=5000)
            pair_url = page.input_value("#connect-pair-url")
            assert "/pair?code=" in pair_url, f"unexpected pair_url: {pair_url}"
            page.click("#btn-close-connect")

            paired_mobile_context = browser.new_context(
                viewport={"width": 390, "height": 844},
                has_touch=True,
                is_mobile=True,
                locale="en-US",
            )
            paired_page = paired_mobile_context.new_page()
            paired_page.on("pageerror", lambda err: page_errors.append(str(err)))
            paired_page.on(
                "console",
                lambda msg: console_errors.append(msg.text)
                if msg.type == "error"
                else None,
            )
            paired_page.goto(pair_url, wait_until="networkidle")
            paired_page.wait_for_selector("#app-view:not(.hidden)", timeout=10000)
            paired_page.wait_for_selector("#xterm-host:not(.hidden)", timeout=10000)
            paired_page.wait_for_selector("#mobile-session-select", timeout=10000)
            paired_page.wait_for_selector("#conn-pill.live", timeout=10000)
            assert "code=" not in paired_page.url, f"pair code leaked in URL: {paired_page.url}"
            time.sleep(0.4)

            keys_box = paired_page.locator("#hq-mobile-keys").bounding_box()
            term_box = paired_page.locator("#terminal-container").bounding_box()
            brand_box = paired_page.locator(".brand-name").bounding_box()
            connect_btn_box = paired_page.locator("#btn-open-connect").bounding_box()
            diff_btn_box = paired_page.locator("#btn-open-diff").bounding_box()
            logout_box = paired_page.locator("#btn-logout").bounding_box()
            assert keys_box is not None and term_box is not None
            assert brand_box is not None and connect_btn_box is not None and diff_btn_box is not None and logout_box is not None
            assert brand_box["x"] + brand_box["width"] <= connect_btn_box["x"] + 1, (
                f"mobile topbar overlap: brand={brand_box}, connect={connect_btn_box}"
            )
            assert connect_btn_box["x"] + connect_btn_box["width"] <= diff_btn_box["x"] + 1, (
                f"mobile topbar overlap: connect={connect_btn_box}, diff={diff_btn_box}"
            )
            assert logout_box["x"] + logout_box["width"] <= 390, (
                f"mobile logout overflows viewport: logout={logout_box}"
            )
            assert abs((keys_box["y"] + keys_box["height"]) - 844) <= 2, f"mobile keys bottom mismatch: {keys_box}"
            assert term_box["y"] + term_box["height"] <= keys_box["y"] + 2, (
                f"terminal overlaps mobile keys: term={term_box}, keys={keys_box}"
            )

            paired_page.fill("#mobile-quick-input", "echo HQCODE_MOBILE_OK")
            paired_page.press("#mobile-quick-input", "Enter")
            paired_page.wait_for_function(
                """() => {
                    const dom = document.querySelector('.xterm-rows')?.innerText || '';
                    const t = window.__hqTerm;
                    let bufText = '';
                    if (t?.buffer?.active) {
                        const b = t.buffer.active;
                        for (let i = 0; i < b.length; i++) {
                            bufText += (b.getLine(i)?.translateToString(true) || '') + '\\n';
                        }
                    }
                    return dom.includes('HQCODE_MOBILE_OK') || bufText.includes('HQCODE_MOBILE_OK');
                }""",
                timeout=10000,
            )
            paired_page.screenshot(path=str(ASSETS_DIR / "screenshot-mobile.png"))
            paired_page.close()

            # Revoke paired device from desktop and confirm mobile context cookie immediately gets 401
            page.click("#btn-open-connect")
            page.wait_for_selector("#paired-devices-list .device-row button", timeout=5000)
            page.click("#paired-devices-list .device-row button")
            page.wait_for_selector("#paired-devices-list .device-row.revoked", timeout=5000)
            page.click("#btn-close-connect")
            revoked_resp = paired_mobile_context.request.get(f"http://127.0.0.1:{PORT}/api/system/info")
            assert revoked_resp.status == 401, f"revoked device expected 401, got {revoked_resp.status}"
            paired_mobile_context.close()

            # 4. Open Git Diff drawer, verify colorized lines & capture desktop screenshot
            page.click("#btn-open-diff")
            page.wait_for_selector("#diff-drawer:not(.hidden)", timeout=5000)
            page.wait_for_selector("#diff-viewer .diff-row.add", timeout=5000)
            page.wait_for_selector("#rss-pill:not(.hidden)", timeout=5000)
            time.sleep(0.3)
            page.screenshot(path=str(ASSETS_DIR / "screenshot-desktop.png"))

            # 5. Test Commit flow & EN/ZH toggle
            page.fill("#commit-message-input", "feat: add ready and metrics routes")
            page.click("#btn-git-commit")
            page.wait_for_function(
                "() => (document.querySelector('#diff-file-list')?.innerText || '').toLowerCase().includes('clean')",
                timeout=10000,
            )
            page.click("#btn-close-diff")
            page.click("#btn-lang-toggle")
            page.wait_for_function(
                "() => document.querySelector('#btn-lang-toggle')?.innerText === '中文'",
                timeout=5000,
            )
            page.click("#btn-lang-toggle")
            page.wait_for_function(
                "() => document.querySelector('#btn-lang-toggle')?.innerText === 'EN'",
                timeout=5000,
            )

            # 6. Close sessions through API and verify tmux cleanup
            for sid in created_session_ids:
                page.evaluate(
                    f"async () => await fetch('/api/terminal/sessions/{sid}', {{ method: 'DELETE' }})"
                )
            time.sleep(0.3)

            browser.close()

        assert not page_errors, f"Unexpected page errors: {page_errors}"
        assert not console_errors, f"Unexpected console errors: {console_errors}"

        for sid in created_session_ids:
            chk = subprocess.run(
                ["tmux", "-L", f"hqcode_{sid}", "has-session", "-t", f"hqcode_{sid}"],
                capture_output=True,
            )
            assert chk.returncode != 0, f"tmux session hqcode_{sid} survived cleanup"

        print("E2E_BROWSER_PASS: desktop + reload persistence + git diff/commit + mobile layout + screenshots OK")
    finally:
        for sid in created_session_ids:
            subprocess.run(
                ["tmux", "-L", f"hqcode_{sid}", "kill-server"],
                capture_output=True,
            )
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        shutil.rmtree(tmp_root, ignore_errors=True)


if __name__ == "__main__":
    main()
