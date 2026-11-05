#!/usr/bin/env python3
"""MCP smoke test for an installed squall binary.

Starts the binary over stdio, completes the MCP handshake, calls `listmodels`, and
checks that every --expect string appears in the reply. `listmodels` only reports models
whose API key / CLI is available, so pass --expect only where the real environment is
loaded; elsewhere (e.g. a bare SSH session) run it without --expect as a handshake check.

    python3 -I scripts/mcp-smoke.py /path/to/squall --expect kimi-k3.1-pro

Reads only the stdlib, so it can be piped to a remote python3: `ssh host python3 - ...`.
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
        text = json.dumps(reply)
        missing = [e for e in expects if e not in text]
        if not reply or "error" in reply or missing:
            print(f"FAIL {binary}: missing {missing}" if missing else f"FAIL {binary}: {text[:300]}")
            return 1
        server = init["result"].get("serverInfo", {})
        print(f"OK   {binary}: {server.get('name')} {server.get('version')}, "
              f"listmodels has {expects or 'a reply'}")
        return 0
    finally:
        timer.cancel()
        proc.kill()


if __name__ == "__main__":
    sys.exit(main())
