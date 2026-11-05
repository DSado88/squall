#!/usr/bin/env python3
"""MCP smoke test for an installed squall binary.

Starts the binary over stdio, completes the MCP handshake, calls `listmodels`, and
checks that the reply is a real tool result (not an error) listing every --expect key
exactly. `listmodels` only reports models
whose API key / CLI is available, so pass --expect only where the real environment is
loaded; elsewhere (e.g. a bare SSH session) run it without --expect as a handshake check.

    python3 -I scripts/mcp-smoke.py /path/to/squall --expect kimi-k3.1-pro

Stdlib only and Python 3.9 compatible: the Studio runs it with /usr/bin/python3.
"""

import json
import subprocess
import sys
import threading


def main() -> int:
    args = sys.argv[1:]
    if not args:
        print("usage: mcp-smoke.py BINARY [--expect TEXT]...", file=sys.stderr)
        return 2
    binary, expects = args[0], []
    i = 1
    while i < len(args):
        if args[i] == "--expect" and i + 1 < len(args):
            expects.append(args[i + 1])
            i += 2
        else:
            i += 1

    proc = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True)
    timer = threading.Timer(60, proc.kill)
    timer.start()

    def send(msg):
        proc.stdin.write(json.dumps(msg) + "\n")
        proc.stdin.flush()

    def recv(want_id):
        for line in proc.stdout:
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == want_id:
                return msg
        return None

    try:
        send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "squall-smoke", "version": "1"}}})
        init = recv(1)
        if not init or "result" not in init:
            print(f"FAIL {binary}: no initialize result ({init})")
            return 1
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
              "params": {"name": "listmodels", "arguments": {}}})
        reply = recv(2)
        keys, problem = listed_keys(reply)
        if problem:
            print(f"FAIL {binary}: {problem}")
            return 1
        missing = [e for e in expects if e not in keys]
        if missing:
            print(f"FAIL {binary}: listmodels lacks {missing} (has {sorted(keys)})")
            return 1
        server = init["result"].get("serverInfo", {})
        print(f"OK   {binary}: {server.get('name')} {server.get('version')}, "
              f"listmodels has {expects or 'a valid reply'}")
        return 0
    finally:
        timer.cancel()
        proc.kill()
        proc.wait()


def listed_keys(reply):
    """Model keys from a listmodels reply, or a reason it isn't a valid one.

    Keys come from the first column of the markdown table, so matching is exact:
    `kimi-k3` must not pass because `kimi-k3.1-pro` is listed.
    """
    result = reply.get("result") if isinstance(reply, dict) else None
    if not isinstance(result, dict):
        return set(), "no result in listmodels reply: " + json.dumps(reply)[:200]
    if result.get("isError"):
        return set(), "listmodels returned a tool error: " + json.dumps(result)[:200]
    texts = [c.get("text", "") for c in result.get("content", []) if isinstance(c, dict)]
    if not texts:
        return set(), "listmodels reply has no content"
    try:
        table = json.loads(texts[0]).get("content", "")
    except (ValueError, AttributeError):
        table = texts[0]
    keys = set()
    for line in table.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if line.lstrip().startswith("|") and cells and cells[0] not in ("Model", "") \
                and not set(cells[0]) <= set("-: "):
            keys.add(cells[0])
    return keys, None


if __name__ == "__main__":
    sys.exit(main())
