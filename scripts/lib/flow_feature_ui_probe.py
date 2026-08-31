#!/usr/bin/env python3
"""Live-browser probe for the v0.4 Flow feature-flag UI gate."""

from __future__ import annotations

import argparse
import json
import re
import urllib.request

from playwright.sync_api import sync_playwright


def api_json(url: str, token: str, method: str = "GET", body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--frontend-url", required=True)
    parser.add_argument("--api-url", required=True)
    parser.add_argument("--workspace-id", required=True)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--object-id", required=True)
    parser.add_argument("--token", required=True)
    args = parser.parse_args()

    forms_path = f"/workspace/{args.workspace_id}/projects/{args.project_id}/forms"
    flow_root = f"/workspace/{args.workspace_id}/flow"
    flow_object = f"{flow_root}/{args.object_id}"
    feature_url = f"{args.api_url}/api/v1/workspaces/{args.workspace_id}/features/flow"
    forms_api_url = f"{args.api_url}/api/v1/projects/{args.project_id}/forms"
    observations: dict[str, object] = {"browser": "chromium", "requests": [], "request_failures": [], "console": []}
    violations: list[str] = []

    before_forms = api_json(forms_api_url, args.token)
    if before_forms.get("code") != 0:
        violations.append(f"Forms live control failed before flag toggle: {before_forms}")

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(executable_path="/usr/bin/chromium", headless=True, args=["--no-sandbox"])
        context = browser.new_context()
        page = context.new_page()

        # Establish the frontend origin before writing localStorage. An init script also runs
        # on the initial opaque about:blank document, where localStorage is unavailable; an
        # explicit origin visit makes this authentication setup observable and deterministic.
        page.goto(args.frontend_url + "/auth/login", wait_until="domcontentloaded", timeout=30_000)
        page.evaluate("token => localStorage.setItem('auth_token', token)", args.token)
        page.reload(wait_until="domcontentloaded", timeout=30_000)
        observations["stored_token_present"] = page.evaluate("localStorage.getItem('auth_token') !== null")

        def record_response(response) -> None:
            if "/api/v1/" in response.url or "/features/flow" in response.url or "/forms" in response.url:
                observations["requests"].append({"url": response.url, "status": response.status})

        page.on("response", record_response)
        page.on("requestfailed", lambda request: observations["request_failures"].append({"url": request.url, "failure": request.failure}))
        page.on("console", lambda message: observations["console"].append({"type": message.type, "text": message.text}))

        # Positive control: enabled Flow must expose both the workspace navigation entry and
        # the actual direct route. Without this control, a UI that never exposes Flow would
        # incorrectly pass the disabled-state assertions.
        enabled = api_json(
            feature_url,
            args.token,
            "PUT",
            {"enabled": True, "idempotency_key": "ui-probe-enable"},
        )
        if enabled.get("code") != 0:
            violations.append(f"admin could not enable Flow for UI positive control: {enabled}")
        enabled_feature_read = api_json(feature_url, args.token)
        page.goto(args.frontend_url + forms_path, wait_until="networkidle", timeout=30_000)
        enabled_nav_count = page.locator(f'a[href^="{flow_root}"]').count()
        forms_enabled_url = page.url
        page.goto(args.frontend_url + flow_object, wait_until="networkidle", timeout=30_000)
        page.wait_for_timeout(500)
        enabled_direct_has_navigator = page.get_by_text("Feature UI control", exact=True).count() > 0
        enabled_direct_has_disabled = page.get_by_text(re.compile(r"Flow (is not enabled|未启用)"), exact=False).count() > 0
        enabled_body_text = page.locator("body").inner_text()
        if not enabled_direct_has_navigator:
            violations.append("enabled positive control did not expose the live Flow direct URL")

        # Required negative: false removes the entry and direct URL fails closed to the safe
        # disabled page. Both are observed through a real browser talking to the live API.
        disabled = api_json(
            feature_url,
            args.token,
            "PUT",
            {"enabled": False, "idempotency_key": "ui-probe-disable"},
        )
        if disabled.get("code") != 0:
            violations.append(f"admin could not disable Flow for UI negative control: {disabled}")
        disabled_feature_read = api_json(feature_url, args.token)
        page.goto(args.frontend_url + forms_path, wait_until="networkidle", timeout=30_000)
        disabled_nav_count = page.locator(f'a[href^="{flow_root}"]').count()
        forms_disabled_url = page.url
        page.goto(args.frontend_url + flow_object, wait_until="networkidle", timeout=30_000)
        page.wait_for_timeout(500)
        disabled_direct_has_navigator = page.get_by_text("Feature UI control", exact=True).count() > 0
        disabled_direct_has_safe_page = page.get_by_text(re.compile(r"Flow (is not enabled|未启用)"), exact=False).count() > 0
        disabled_body_text = page.locator("body").inner_text()
        if disabled_nav_count != 0:
            violations.append(f"flow_enabled=false left {disabled_nav_count} Flow navigation entry/entries visible")
        if disabled_direct_has_navigator or not disabled_direct_has_safe_page:
            violations.append("flow_enabled=false did not make the direct URL render the safe disabled page")

        browser.close()

    after_forms = api_json(forms_api_url, args.token)
    if after_forms != before_forms:
        violations.append("Forms live API response changed when only flow_enabled was toggled")
    if not forms_enabled_url.endswith(forms_path) or not forms_disabled_url.endswith(forms_path):
        violations.append("Forms browser route was not reachable in both enabled and disabled states")

    observations.update(
        {
            "enabled": {
                "feature_read": enabled_feature_read,
                "navigation_entry_count": enabled_nav_count,
                "direct_url_has_navigator": enabled_direct_has_navigator,
                "direct_url_has_disabled_page": enabled_direct_has_disabled,
                "body_text": enabled_body_text[:2000],
            },
            "disabled": {
                "feature_read": disabled_feature_read,
                "navigation_entry_count": disabled_nav_count,
                "direct_url_has_navigator": disabled_direct_has_navigator,
                "direct_url_has_safe_page": disabled_direct_has_safe_page,
                "body_text": disabled_body_text[:2000],
            },
            "forms": {
                "live_api_before": before_forms,
                "live_api_after": after_forms,
                "identical_across_toggle": before_forms == after_forms,
                "browser_route_enabled": forms_enabled_url,
                "browser_route_disabled": forms_disabled_url,
            },
        }
    )
    print(json.dumps({"observations": observations, "violations": violations}, separators=(",", ":")))
    return 0 if not violations else 1


if __name__ == "__main__":
    raise SystemExit(main())
