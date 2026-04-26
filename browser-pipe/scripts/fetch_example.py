#!/usr/bin/env python3
"""
browser-pipe example: ensure daemon is running, then fetch example.com

Usage:
    python fetch_example.py [--url URL] [--referrer URL]

The script will:
  1. Run `browser-pipe ensure-daemon` to make sure the daemon is up
  2. Fetch http://example.com via the daemon's HTTP /fetch endpoint
  3. Print the response body

Prerequisites:
  - browser-pipe is installed and available in PATH
"""

from __future__ import annotations

import json
import subprocess
import sys
import urllib.request

DAEMON_ADDR = "127.0.0.1:10129"
FETCH_URL = f"http://{DAEMON_ADDR}/fetch"


def ensure_daemon() -> None:
    """Start the daemon if it isn't already running."""
    result = subprocess.run(
        ["browser-pipe", "ensure-daemon"],
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode != 0:
        print(f"ensure-daemon failed (exit {result.returncode}):", file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        sys.exit(1)
    print(f"[ensure-daemon] {result.stdout.strip()}")


def fetch(url: str, *, method: str = "GET", referrer: str | None = None) -> dict:
    """Fetch a URL through the browser-pipe daemon's HTTP endpoint."""
    req = urllib.request.Request(FETCH_URL, method=method)
    req.add_header("X-Forwarded-Url", url)
    if referrer:
        req.add_header("X-Referrer", referrer)

    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="Fetch example.com via browser-pipe")
    parser.add_argument(
        "--url",
        default="http://example.com",
        help="URL to fetch (default: http://example.com)",
    )
    parser.add_argument(
        "--referrer",
        default=None,
        help="Referrer URL for partitioned cookies (CHIPS) support",
    )
    args = parser.parse_args()

    ensure_daemon()

    print(f"\nFetching {args.url} ...")
    result = fetch(args.url, referrer=args.referrer)

    status = result.get("status")
    status_text = result.get("statusText", "")
    body = result.get("body", "")
    error = result.get("error")

    if error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)

    print(f"Status: {status} {status_text}")
    print(f"Redirected: {result.get('redirected', False)}")
    print(f"Final URL: {result.get('url', '')}")
    print(f"\n--- Response Body ({len(body)} chars) ---\n")
    print(body)


if __name__ == "__main__":
    main()
