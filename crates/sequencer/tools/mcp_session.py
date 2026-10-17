"""Drive `metal_seq mcp` through a scripted list of tool calls.

    python3 crates/sequencer/tools/mcp_session.py target/debug/metal_seq calls.json OUT_DIR

calls.json is a list of [tool_name, arguments] pairs. Text results print;
images are written to OUT_DIR/capture-N.png and the server's stderr to
OUT_DIR/server.log. Set ESEQ_CONFIG_DIR to a scratch folder to keep the
session's Local modules out of ~/.eseq.d.
"""
import base64
import json
import os
import subprocess
import sys
import time

binary, calls_path, out_dir = sys.argv[1], sys.argv[2], sys.argv[3]
calls = json.load(open(calls_path))
os.makedirs(out_dir, exist_ok=True)
server = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=open(os.path.join(out_dir, "server.log"), "w"), text=True)


def send(message):
    server.stdin.write(json.dumps(message) + "\n")
    server.stdin.flush()


def rpc(request_id, method, params=None):
    message = {"jsonrpc": "2.0", "id": request_id, "method": method}
    if params is not None:
        message["params"] = params
    send(message)
    line = server.stdout.readline()
    if not line:
        raise SystemExit("the server closed stdout; see server.log")
    return json.loads(line)


started = time.time()
info = rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                             "clientInfo": {"name": "mcp_session", "version": "0"}})
print("initialize:", info["result"]["serverInfo"], f"{time.time() - started:.1f}s")
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
print("tools:", [tool["name"] for tool in rpc(2, "tools/list")["result"]["tools"]])
for index, (name, arguments) in enumerate(calls):
    started = time.time()
    response = rpc(10 + index, "tools/call", {"name": name, "arguments": arguments})
    result = response.get("result", {})
    print(f"\n### {name} {json.dumps(arguments)[:120]} ({time.time() - started:.2f}s)"
          f"{' ERROR' if result.get('isError') else ''}")
    for item in result.get("content", []):
        if item["type"] == "text":
            print(item["text"])
        else:
            path = os.path.join(out_dir, f"capture-{index}.png")
            with open(path, "wb") as png:
                png.write(base64.b64decode(item["data"]))
            print("[image]", path)
    if "error" in response:
        print(response["error"])
server.stdin.close()
server.wait(timeout=30)
