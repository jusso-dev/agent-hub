#!/usr/bin/env python3
"""Call one hub-mcp tool over stdio, the way an MCP host would.

Usage: mcp_call.py <hub-mcp binary> <tool> '<json args>'
Prints the tool's text result. Exits 1 if the tool reported an error.
"""
import json
import subprocess
import sys


def main():
    binary, tool, args = sys.argv[1], sys.argv[2], json.loads(sys.argv[3] if len(sys.argv) > 3 else "{}")
    p = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def send(obj):
        p.stdin.write(json.dumps(obj) + "\n")
        p.stdin.flush()

    def recv(want):
        for line in p.stdout:
            msg = json.loads(line)
            if msg.get("id") == want:
                return msg
        raise SystemExit("hub-mcp closed stdout")

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "demo", "version": "0"}}})
    recv(1)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    res = recv(2)
    p.stdin.close()
    p.wait(timeout=10)
    if "error" in res:
        print(json.dumps(res["error"]), file=sys.stderr)
        sys.exit(1)
    result = res["result"]
    for c in result.get("content", []):
        if c.get("type") == "text":
            print(c["text"])
    sys.exit(1 if result.get("isError") else 0)


if __name__ == "__main__":
    main()
