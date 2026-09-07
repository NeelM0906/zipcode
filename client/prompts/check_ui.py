#!/usr/bin/env python3
"""Independent browser check of the synthetic UI fixture (Playwright required)."""

import argparse
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


def check(path: Path) -> None:
    with sync_playwright() as runtime:
        browser = runtime.chromium.launch(headless=True, chromium_sandbox=True)
        try:
            context = browser.new_context(service_workers="block")
            context.route("**/*", lambda route: route.abort())
            page = context.new_page()
            page.set_default_timeout(5000)
            page.set_content(path.read_text(encoding="utf-8"))
            button = page.get_by_role("button", name="Add", exact=True)
            count = page.locator("#count")
            expect(count).to_be_visible()
            expect(count).to_have_text("0")
            button.click()
            expect(count).to_be_visible()
            expect(count).to_have_text("1")
            button.focus()
            page.keyboard.press("Enter")
            expect(count).to_be_visible()
            expect(count).to_have_text("2")
            page.keyboard.press("Space")
            expect(count).to_be_visible()
            expect(count).to_have_text("3")
            print("PASS: pointer, Enter, and Space increment the visible count")
        finally:
            browser.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("html", type=Path)
    check(parser.parse_args().html)
