#!/usr/bin/env python3
"""Fetch Android source code snippets from cs.android.com links."""

import base64
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Optional
from urllib.parse import unquote


@dataclass
class ParsedURL:
    superproject_type: str  # "platform" or "kernel"
    branch: str
    path: str
    line_start: Optional[int]
    line_end: Optional[int]
    drc: Optional[str]

    @property
    def effective_ref(self) -> str:
        return self.drc if self.drc else self.branch


def parse_url(url: str) -> ParsedURL:
    """Parse a cs.android.com URL into its components."""
    url = unquote(url).strip()

    if "/search?" in url:
        raise ValueError("Search URLs are not supported. Provide a direct file link.")

    # Match the general cs.android.com pattern:
    # https://cs.android.com/android/{type}/superproject[/...]/+/{ref}:{path}[;l={lines}][;drc={hash}]
    pattern = (
        r"https?://cs\.android\.com/android/"
        r"(\w+)/superproject"  # (1) platform or kernel
        r"(?:/[^/+]*)?"  # optional extra segment like /main
        r"/\+/"
        r"([^:]+)"  # (2) branch/ref
        r":"
        r"([^;]+)"  # (3) file path
        r"(?:;l=(\d+)(?:-(\d+))?)?"  # (4) optional line start, (5) optional line end
        r"(?:;drc=([a-f0-9]+))?"  # (6) optional commit hash
    )
    m = re.match(pattern, url)
    if not m:
        raise ValueError(f"Could not parse URL: {url}")

    superproject_type = m.group(1)
    branch = m.group(2)
    path = m.group(3)
    line_start = int(m.group(4)) if m.group(4) else None
    line_end = int(m.group(5)) if m.group(5) else line_start
    drc = m.group(6)

    if superproject_type not in ("platform", "kernel"):
        raise ValueError(f"Unknown superproject type: {superproject_type}")

    return ParsedURL(
        superproject_type=superproject_type,
        branch=branch,
        path=path,
        line_start=line_start,
        line_end=line_end,
        drc=drc,
    )


def fetch_url(url: str, timeout: int = 15) -> bytes:
    """Fetch URL content with one retry on failure."""
    req = urllib.request.Request(url, headers={
        "User-Agent": "fetch-android-source/1.0",
    })
    for attempt in range(2):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except urllib.error.HTTPError:
            raise
        except (urllib.error.URLError, TimeoutError):
            if attempt == 0:
                time.sleep(2)
                continue
            raise


def resolve_and_fetch(parsed: ParsedURL) -> tuple[str, str, str]:
    """Resolve repo name and fetch file content.

    Returns (repo_name, file_path, decoded_content).
    """
    ref = parsed.effective_ref
    parts = parsed.path.split("/")

    if parsed.superproject_type == "kernel":
        # Kernel: first path component is the repo name
        if len(parts) < 2:
            raise ValueError(f"Kernel path too short: {parsed.path}")
        repo = f"kernel/{parts[0]}"
        filepath = "/".join(parts[1:])
        url = f"https://android.googlesource.com/{repo}/+/{ref}/{filepath}?format=TEXT"
        try:
            content = fetch_url(url)
            decoded = base64.b64decode(content).decode("utf-8", errors="replace")
            return repo, filepath, decoded
        except urllib.error.HTTPError as e:
            if e.code == 404:
                raise ValueError(
                    f"Could not fetch kernel source.\n"
                    f"  Tried: {repo} / {filepath} @ {ref}\n"
                    f"  The ref '{ref}' may be expired or renamed."
                ) from None
            raise

    # Platform: try different repo prefix lengths (2, 1, 3)
    tried = []
    for prefix_len in (2, 1, 3):
        if prefix_len >= len(parts):
            continue
        repo_suffix = "/".join(parts[:prefix_len])
        filepath = "/".join(parts[prefix_len:])
        repo = f"platform/{repo_suffix}"
        url = f"https://android.googlesource.com/{repo}/+/{ref}/{filepath}?format=TEXT"
        tried.append(repo)
        try:
            content = fetch_url(url)
            decoded = base64.b64decode(content).decode("utf-8", errors="replace")
            return repo, filepath, decoded
        except urllib.error.HTTPError as e:
            if e.code == 404:
                continue
            raise

    raise ValueError(
        f"Could not resolve repository for path: {parsed.path}\n"
        f"  Tried repos: {', '.join(tried)}\n"
        f"  The ref '{ref}' may be expired or the path is incorrect."
    )


def extract_lines(content: str, start: int, end: int) -> tuple[list[str], Optional[str]]:
    """Extract line range (1-indexed, inclusive). Returns (lines, warning)."""
    all_lines = content.split("\n")
    total = len(all_lines)
    warning = None

    if start > total:
        raise ValueError(f"Requested line {start} but file has only {total} lines.")

    if end > total:
        warning = f"Warning: Requested lines {start}-{end} but file has only {total} lines. Showing {start}-{total}."
        end = total

    return all_lines[start - 1 : end], warning


def format_output(
    repo: str, filepath: str, ref: str, start: int, end: int,
    lines: list[str], warning: Optional[str],
) -> str:
    """Format the final output."""
    line_spec = str(start) if start == end else f"{start}-{end}"
    parts = [
        f"# File: {filepath}",
        f"# Repo: {repo}",
        f"# Ref: {ref}",
        f"# Lines: {line_spec}",
    ]
    if warning:
        parts.append(f"# {warning}")
    parts.append("")
    parts.extend(lines)
    return "\n".join(parts)


def main():
    if len(sys.argv) != 2:
        print("Usage: fetch.py <cs.android.com URL>", file=sys.stderr)
        sys.exit(1)

    url = sys.argv[1]

    try:
        parsed = parse_url(url)
    except ValueError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        repo, filepath, content = resolve_and_fetch(parsed)
    except ValueError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
    except (urllib.error.URLError, TimeoutError) as e:
        print(f"Error: Network request failed: {e}", file=sys.stderr)
        sys.exit(1)

    all_lines = content.split("\n")
    total = len(all_lines)

    if parsed.line_start is None:
        # No line range specified: output entire file only if small enough
        if total > 100:
            print(
                f"Error: No line range specified and file has {total} lines (> 100).\n"
                f"  Please provide a URL with ';l=' to specify the line range.",
                file=sys.stderr,
            )
            sys.exit(1)
        start, end = 1, total
        lines = all_lines
        warning = None
    else:
        start = parsed.line_start
        end = parsed.line_end
        try:
            lines, warning = extract_lines(content, start, end)
        except ValueError as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)

    output = format_output(repo, filepath, parsed.effective_ref, start, end, lines, warning)
    print(output)


if __name__ == "__main__":
    main()
