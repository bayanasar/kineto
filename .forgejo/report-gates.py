#!/usr/bin/env python3
"""Expose per-step Forgejo commit statuses when Actions job/log APIs are unavailable."""

import json
import os
import sys
import urllib.error
import urllib.request

STATE = {
    "success": "success",
    "failure": "failure",
    "cancelled": "warning",
    "skipped": "warning",
}


def main() -> int:
    token = os.environ.get("STATUS_TOKEN", "").strip()
    sha = os.environ.get("HEAD_SHA", "").strip()
    server = os.environ.get("GITHUB_SERVER_URL", "").rstrip("/")
    repo = os.environ.get("GITHUB_REPOSITORY", "").strip()
    run_number = os.environ.get("GITHUB_RUN_NUMBER", "").strip()
    steps = json.loads(os.environ.get("STEPS") or "{}")

    if not token:
        print("missing STATUS_TOKEN; cannot publish per-gate statuses")
        return 1
    if not (sha and server and repo):
        print(f"missing status context: sha={sha!r} server={server!r} repo={repo!r}")
        return 1

    named = {key: value for key, value in steps.items() if not key.isdigit()}
    expected = 0
    posted = 0

    for name, step in sorted(named.items()):
        outcome = (step or {}).get("outcome")
        if not outcome:
            continue
        expected += 1
        state = STATE.get(outcome, "warning")
        body = json.dumps(
            {
                "state": state,
                "context": f"gate/{name}",
                "description": f"{outcome} in this run",
                "target_url": (
                    f"{server}/{repo}/actions/runs/{run_number}"
                    if run_number
                    else f"{server}/{repo}/actions"
                ),
            }
        ).encode()

        request = urllib.request.Request(
            f"{server}/api/v1/repos/{repo}/statuses/{sha}",
            data=body,
            method="POST",
            headers={
                "Authorization": f"token {token}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                response.read()
            posted += 1
            print(f"gate/{name}: {state}")
        except urllib.error.HTTPError as error:
            print(f"gate/{name}: POST failed: HTTP {error.code} {error.read()[:200]!r}")
        except OSError as error:
            print(f"gate/{name}: POST failed: {error}")

    if posted != expected:
        print(f"published {posted}/{expected} gate statuses")
        return 1

    print(f"published {posted} gate statuses on {sha[:8]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
