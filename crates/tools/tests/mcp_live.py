"""Network-free stdio MCP fixture for same-session lifecycle tests."""
import json
import os
import sys
import time

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mcp-live-fixture", "version": "1"},
        }
    elif method == "tools/list":
        time.sleep(float(sys.argv[1]) if len(sys.argv) > 1 else 0)
        result = {"tools": [{
            "name": "live_echo",
            "description": "Echo a value from the live server.",
            "inputSchema": {
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
            },
            "annotations": {"readOnlyHint": True},
        }]}
    elif method == "tools/call":
        value = request["params"]["arguments"]["value"]
        result = {"content": [{"type": "text", "text": f"live:{value}:{os.getpid()}"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {
            "code": -32601, "message": "Method not found",
        }}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
