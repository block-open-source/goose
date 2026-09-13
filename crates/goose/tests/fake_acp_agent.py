#!/usr/bin/env python3
"""A minimal ACP agent over stdio used by the generic custom-ACP tests.

The agent is intentionally dependency-free: goose's generic ACP provider is
launched with the system Python interpreter, and every observable effect (argv,
cwd, environment, initialize/session/new/prompt traffic) is written to a JSON
transcript so a test can assert on the launch and handshake semantics instead of
on goose internals.

Environment:
  FAKE_ACP_RECORD     JSON transcript path (optional; no transcript without it)
  FAKE_ACP_MODE       normal | exit_before_handshake | fail_handshake |
                      require_auth | slow
  FAKE_ACP_REPLY      text streamed back for every prompt
  FAKE_ACP_HEARTBEAT  file rewritten with an increasing tick while running
  FAKE_ACP_MCP_HTTP   "1" advertises the HTTP MCP capability
  FAKE_ACP_MODES      comma-separated session modes; the first is the current one
  FAKE_ACP_SESSION_RESUME  "1" advertises session/load and session/close
  FAKE_ACP_WATCH_ENV  comma-separated environment variable names to capture
"""

import json
import os
import sys
import threading
import time

TRANSCRIPT = os.environ.get("FAKE_ACP_RECORD")
MODE = os.environ.get("FAKE_ACP_MODE", "normal")
REPLY = os.environ.get("FAKE_ACP_REPLY", "fake-acp-agent-reply")
HEARTBEAT = os.environ.get("FAKE_ACP_HEARTBEAT")
MODES = [mode for mode in os.environ.get("FAKE_ACP_MODES", "").split(",") if mode]
WATCH_ENV = [name for name in os.environ.get("FAKE_ACP_WATCH_ENV", "").split(",") if name]

TRANSCRIPT_LOCK = threading.Lock()
STATE = {}


def write_transcript_locked():
    if not TRANSCRIPT:
        return
    temporary = TRANSCRIPT + ".tmp"
    with open(temporary, "w") as handle:
        handle.write(json.dumps(STATE, indent=2))
    os.replace(temporary, TRANSCRIPT)


def record(key, value):
    with TRANSCRIPT_LOCK:
        STATE[key] = value
        write_transcript_locked()


def record_message(key, value):
    with TRANSCRIPT_LOCK:
        STATE.setdefault(key, []).append(value)
        write_transcript_locked()


def heartbeat_loop():
    if not HEARTBEAT:
        return
    ticks = 0
    while True:
        ticks += 1
        try:
            temporary = HEARTBEAT + ".tmp"
            with open(temporary, "w") as handle:
                handle.write(str(ticks))
            os.replace(temporary, HEARTBEAT)
        except OSError:
            pass
        time.sleep(0.02)


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def read_message():
    while True:
        line = sys.stdin.readline()
        if line == "":
            return None
        line = line.strip()
        if line:
            return json.loads(line)


def prompt_text(params):
    parts = []
    for block in params.get("prompt") or []:
        if block.get("type") == "text":
            parts.append(block.get("text", ""))
    return "".join(parts)


def send_chunk(params, text):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": params.get("sessionId"),
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        }
    )


def main():
    with TRANSCRIPT_LOCK:
        STATE.update(
            argv=sys.argv[1:],
            cwd=os.getcwd(),
            pid=os.getpid(),
            interpreter=sys.executable,
            env={name: os.environ.get(name) for name in WATCH_ENV},
        )
        write_transcript_locked()

    threading.Thread(target=heartbeat_loop, daemon=True).start()

    if MODE == "exit_before_handshake":
        sys.exit(3)

    pending_prompts = []

    while True:
        message = read_message()
        if message is None:
            record("stdinClosed", True)
            return 0

        method = message.get("method")
        request_id = message.get("id")
        params = message.get("params") or {}

        if method == "initialize":
            record("initialize", params)
            if MODE == "fail_handshake":
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32603, "message": "handshake refused"},
                    }
                )
                continue
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "protocolVersion": params.get("protocolVersion", 1),
                        "agentCapabilities": {
                            "loadSession": os.environ.get("FAKE_ACP_SESSION_RESUME") == "1",
                            "sessionCapabilities": {"close": {}},
                            "mcpCapabilities": {
                                "http": os.environ.get("FAKE_ACP_MCP_HTTP") == "1",
                                "sse": False,
                            },
                        },
                        "agentInfo": {"name": "fake-acp-agent", "version": "0.0.0"},
                    },
                }
            )
        elif method == "session/new":
            record("sessionNew", params)
            result = {"sessionId": "fake-session-1"}
            if MODES:
                result["modes"] = {
                    "currentModeId": MODES[0],
                    "availableModes": [{"id": mode, "name": mode} for mode in MODES],
                }
            send({"jsonrpc": "2.0", "id": request_id, "result": result})
        elif method == "session/load":
            record("sessionLoad", params)
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        elif method == "session/prompt":
            record_message(
                "prompts",
                {"sessionId": params.get("sessionId"), "text": prompt_text(params)},
            )
            if MODE == "require_auth":
                send(
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {
                            "code": -32000,
                            "message": "authentication required: run 'fake-agent login'",
                        },
                    }
                )
                continue
            send_chunk(params, REPLY)
            if MODE == "slow":
                pending_prompts.append(request_id)
            else:
                send({"jsonrpc": "2.0", "id": request_id, "result": {"stopReason": "end_turn"}})
        elif method == "session/cancel":
            record_message("cancels", params)
            for pending in pending_prompts:
                send({"jsonrpc": "2.0", "id": pending, "result": {"stopReason": "cancelled"}})
            pending_prompts.clear()
        elif method == "session/set_config_option":
            record_message("configOptions", params)
            send({"jsonrpc": "2.0", "id": request_id, "result": {"configOptions": []}})
        elif method == "session/set_mode":
            record_message("modes", params)
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        elif method == "session/close":
            record_message("closes", params)
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        elif request_id is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": "unhandled method " + str(method)},
                }
            )


if __name__ == "__main__":
    sys.exit(main())
