#!/usr/bin/env python3
"""Real two-tenant USER_INSTALL/BOT_DM Discord gateway acceptance.

Only localhost, disposable Ed25519/AES credentials and native read-only model tools.
Exercises the production executable, real backend sessions, private SQLite admission,
manual recovery and encrypted interaction credentials. It does not certify a real
Discord installation, ephemeral client UI, rate quota or paid provider. Queue SQL
observations are query-only; a temporary write lock changes no data. No persisted
state or clock is fabricated to obtain a passing recovery result.
"""
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.error
import urllib.request
from urllib.parse import parse_qs, urlsplit
import uuid

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/jiaclaw").resolve()
ENV = {key: value for key, value in os.environ.items() if not key.startswith("JIACLAW_")}
ENV.update(JIACLAW_LOG_LEVEL="off,jiaclaw::gateway::discord=error", JIACLAW_LOG_FORMAT="json",
           HTTP_PROXY="http://127.0.0.1:1", HTTPS_PROXY="http://127.0.0.1:1",
           ALL_PROXY="http://127.0.0.1:1", NO_PROXY="")
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOCK = threading.Lock()
ERRORS, MODEL_REQUESTS, SENDS, HANDSHAKES, ISSUED_TOKENS = [], [], [], [], []
MODEL_KEYS = set()
MODEL_SECRETS = {who: "local-model-" + uuid.uuid4().hex for who in ("alice", "bob")}
IDENTITIES = {
    "alice": {"app": "123456789012345670", "bot_user": "123456789012345671",
              "sender": "123456789012345672", "conversation": "123456789012345673",
              "command": "123456789012345674"},
    "bob": {"app": "223456789012345670", "bot_user": "223456789012345671",
            "sender": "223456789012345672", "conversation": "223456789012345673",
            "command": "223456789012345674"}}
BOT_TOKENS = {who: "local.bot." + uuid.uuid4().hex for who in IDENTITIES}
STATE_KEYS = {who: uuid.uuid4().hex + uuid.uuid4().hex for who in IDENTITIES}
PRIVATE_KEYS, EVENT_IDS, EVENT_TOKENS = {}, {}, {}
MODES = {who: "ok" for who in IDENTITIES}
RATE_COUNTS = {who: 0 for who in IDENTITIES}
STARTUP_FAULT = {"who": None, "case": None}
GATES = {name: (threading.Event(), threading.Event())
         for name in ("GATE_DISABLE", "GATE_CRASH")}
SEND_GATE = (threading.Event(), threading.Event())
PROCESSES = []


def check_errors():
    with LOCK:
        assert not ERRORS, ERRORS


def wait(predicate, label, seconds=25):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        check_errors()
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError("Timeout: " + label)


def observe_quiet(predicate, label, seconds=2.2):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        check_errors()
        assert predicate(), label
        time.sleep(.05)


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http(base, path, method="GET", body=None, headers=None):
    supplied = dict(headers or {})
    data = None
    if body is not None:
        supplied.setdefault("Content-Type", "application/json")
        data = json.dumps(body).encode()
    request = urllib.request.Request(base + path, method=method, data=data, headers=supplied)
    try:
        response = CLIENT.open(request, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(2 * 1024 * 1024 + 1)
        assert len(raw) <= 2 * 1024 * 1024, "unbounded HTTP response"
        return response.status, json.loads(raw) if raw else None


def stop(process, kill=False):
    if process is None or process.poll() is not None:
        return
    process.kill() if kill else process.terminate()
    try:
        process.wait(timeout=12)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        raise AssertionError("gateway/backend failed graceful shutdown")


def model_count(who, marker=None):
    with LOCK:
        return sum(entry["who"] == who and (marker is None or entry["marker"] == marker)
                   for entry in MODEL_REQUESTS)


def send_count(who):
    with LOCK:
        return sum(entry["who"] == who for entry in SENDS)


def rows(path, query, parameters=()):
    connection = sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, timeout=2)
    try:
        connection.execute("PRAGMA query_only=ON")
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(query, parameters)]
    finally:
        connection.close()


def snowflake(age_seconds=0):
    millis = int(time.time() * 1000) - int(age_seconds * 1000)
    return str(((millis - 1_420_070_400_000) << 22) | (uuid.uuid4().int & ((1 << 22) - 1)))


class LocalHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body, headers=None):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self.dispatch("GET")

    def do_POST(self):
        self.dispatch("POST")

    def do_PATCH(self):
        self.dispatch("PATCH")

    def dispatch(self, method):
        try:
            length = int(self.headers.get("Content-Length", "0"))
            assert 0 <= length <= 2 * 1024 * 1024, "unbounded fixture request"
            raw = self.rfile.read(length)
            body = json.loads(raw) if raw else {}
            path = urlsplit(self.path).path
            if path == "/v1/chat/completions":
                assert method == "POST"
                self.model(body)
            elif path.startswith("/api/v10/webhooks/"):
                self.platform(method, body)
            else:
                self.handshake(method)
        except (BrokenPipeError, ConnectionResetError):
            pass  # The gateway may die after the fixture accepted a real effect.
        except Exception as error:
            frame = traceback.extract_tb(error.__traceback__)[-1]
            with LOCK:
                ERRORS.append(type(error).__name__ + ": " + Path(frame.filename).name + ":" + str(frame.lineno))
            try:
                self.reply(500, {"error": "local fixture failure"})
            except (BrokenPipeError, ConnectionResetError):
                pass

    def handshake(self, method):
        assert method == "GET", "verification must never write application or command"
        assert len(self.headers.get_all("Authorization", [])) == 1
        who = next(who for who, token in BOT_TOKENS.items()
                   if self.headers.get("Authorization") == "Bot " + token)
        identity = IDENTITIES[who]
        url = urlsplit(self.path)
        assert not url.query
        with LOCK:
            HANDSHAKES.append({"who": who, "method": method, "path": url.path})
            fault = STARTUP_FAULT["case"] if STARTUP_FAULT["who"] == who else None
        if url.path == "/api/v10/applications/@me":
            result = {"id": identity["app"], "verify_key": identity["verify_key"],
                      "bot": {"id": identity["bot_user"], "bot": True},
                      "integration_types_config": {"1": {"oauth2_install_params": {
                          "scopes": ["applications.commands"], "permissions": "0"}}}}
            if fault == "app":
                result["id"] = IDENTITIES["bob"]["app"]
            elif fault == "verify_key":
                result["verify_key"] = IDENTITIES["bob"]["verify_key"]
            elif fault == "guild_install":
                result["integration_types_config"]["0"] = result["integration_types_config"]["1"]
            elif fault == "bot_scope":
                result["integration_types_config"]["1"]["oauth2_install_params"]["scopes"].append("bot")
        elif url.path == "/api/v10/users/@me":
            result = {"id": identity["bot_user"], "bot": True}
            if fault == "bot_identity":
                result["id"] = identity["sender"]
        elif url.path == "/api/v10/applications/" + identity["app"] + "/commands/" + identity["command"]:
            result = {"id": identity["command"], "application_id": identity["app"], "type": 1,
                      "name": "jiaclaw", "options": [{"type": 3, "name": "prompt", "required": True}],
                      "integration_types": [1], "contexts": [1], "version": "123456789012345679"}
            if fault == "command_context":
                result["contexts"] = [2]
            elif fault == "command_options":
                result["options"][0]["autocomplete"] = True
            elif fault == "command_id":
                result["id"] = IDENTITIES["bob"]["command"]
        else:
            raise AssertionError("unexpected verification path")
        self.reply(200, result)

    def model(self, body):
        who = next(who for who, secret in MODEL_SECRETS.items()
                   if self.headers.get("Authorization") == "Bearer " + secret)
        assert body["model"] == "fixture-channel"
        assert sorted(tool["function"]["name"] for tool in body["tools"]) == ["datetime_now", "json_query"]
        assert body["parallel_tool_calls"] is False
        prompt = next(message["content"] for message in reversed(body["messages"]) if message["role"] == "user")
        marker = re.search(r"CASE:([A-Z_]+)", prompt).group(1)
        last = body["messages"][-1]
        key = self.headers.get("Idempotency-Key")
        with LOCK:
            assert key and key not in MODEL_KEYS, "model request identity replayed"
            MODEL_KEYS.add(key)
            MODEL_REQUESTS.append({"who": who, "marker": marker, "last_role": last["role"]})
        if last["role"] != "tool":
            if marker in GATES:
                GATES[marker][0].set()
                assert GATES[marker][1].wait(180), "model gate expired"
            call = {"id": "call_" + uuid.uuid4().hex, "type": "function",
                    "function": {"name": "datetime_now", "arguments": "{}"}}
            message, reason = {"role": "assistant", "content": None, "tool_calls": [call]}, "tool_calls"
        else:
            previous = body["messages"][-2]
            assert previous["tool_calls"][0]["id"] == last["tool_call_id"]
            assert "error" not in json.loads(last["content"]), "native clock failed"
            content = who + " CASE:" + marker + " verified reply literal <@123456789>"
            if marker in ("UNKNOWN", "FOLLOW_UNKNOWN", "MULTIPART", "FOLLOW_CRASH"):
                content += "🧭界" * 500 + "x" * 9500  # ~13 KiB UTF-8, six UTF-16-bounded parts.
            elif marker == "OVERSIZE":
                content += "x" * 12500 + "🧭界"  # Below 16 KiB but seven parts: never truncate to six.
            message, reason = {"role": "assistant", "content": content}, "stop"
        self.reply(200, {"choices": [{"message": message, "finish_reason": reason}]})

    def platform(self, method, body):
        url = urlsplit(self.path)
        parts = url.path.split("/")
        assert len(parts) in (6, 8), "unexpected private webhook shape"
        app, token = parts[4:6]
        who, update = next((who, update) for (who, update), value in EVENT_TOKENS.items() if value == token)
        identity = IDENTITIES[who]
        assert app == identity["app"], "cross-tenant private webhook"
        assert self.headers.get("Authorization") is None, "bot credential leaked to interaction URL"
        if method == "PATCH":
            assert parts[6:] == ["messages", "@original"] and not url.query
            assert "flags" not in body, "edit original tried to change its visibility"
        else:
            assert method == "POST" and len(parts) == 6 and parse_qs(url.query) == {"wait": ["true"]}
            assert body["flags"] == 64, "followup lost ephemeral flag"
        assert body["allowed_mentions"] == {"parse": [], "replied_user": False}
        assert body["content"].strip() and len(body["content"].encode("utf-16-le")) // 2 <= 2000
        with LOCK:
            mode = MODES[who]
            receipt = snowflake()
            SENDS.append({"who": who, "update": update, "method": method, "mode": mode,
                          "text": body["content"], "receipt": receipt, "time_ms": int(time.time() * 1000)})
            if mode.startswith("rate"):
                RATE_COUNTS[who] += 1
                attempt = RATE_COUNTS[who]
        if mode == "unknown" or (mode == "follow_unknown" and method == "POST"):
            self.reply(503, {"message": "locally accepted but outcome ambiguous"})
        elif mode.startswith("rate"):
            delay = 1200 if mode == "rate_expiry" else (5 if attempt == 1 else 1)
            self.reply(429, {"message": "rate limited", "retry_after": float(delay), "global": False})
        else:
            if mode == "send_crash" or (mode == "follow_crash" and method == "POST"):
                SEND_GATE[0].set()
                assert SEND_GATE[1].wait(60), "send gate expired"
            self.reply(200, {"id": receipt, "channel_id": identity["conversation"],
                             "flags": 0 if mode == "public_receipt" else 64})


def main():
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), LocalHandler)
    fixture.daemon_threads = True
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="jiaclaw-gateway-discord-") as temp:
            root = Path(temp).resolve()
            base = "http://127.0.0.1:" + str(fixture.server_port)
            settings = {"bind": "127.0.0.1:" + str(port()),
                        "registry_path": str(root / "registry/users.sqlite3"),
                        "request_timeout_seconds": 150, "max_in_flight": 4, "backends": []}
            cfg = root / "gateway.json"
            gateway_url = "http://" + settings["bind"]
            backend, users, bindings = {}, {}, {}
            gateway = None

            def secret(name, value):
                path = root / name
                path.write_text(value)
                path.chmod(0o600)
                return str(path)

            def save():
                cfg.write_text(json.dumps(settings))

            def cli(*arguments, ok=True):
                result = subprocess.run([str(BINARY), "gateway", *arguments, "--config", str(cfg)],
                                        env=ENV, text=True, capture_output=True, timeout=20)
                if ok:
                    assert result.returncode == 0, (arguments[0], "CLI failed", result.returncode)
                    parsed = json.loads(result.stdout)
                    if isinstance(parsed, dict) and "token" in parsed:
                        ISSUED_TOKENS.append(parsed["token"])
                    return parsed
                assert result.returncode != 0, (arguments[0], "unexpected CLI success")
                return result

            def launch(arguments, url, name):
                with (root / (name + ".log")).open("ab") as output:
                    process = subprocess.Popen([str(BINARY), *arguments], env=ENV, stdout=output, stderr=output)
                PROCESSES.append(process)
                def healthy():
                    assert process.poll() is None, name + " startup failed; inspect private local log"
                    try:
                        return http(url, "/health")[0] == 200
                    except (OSError, urllib.error.URLError):
                        return False
                wait(healthy, name + " startup")
                return process

            def start_gateway():
                return launch(["gateway", "serve", "--config", str(cfg)], gateway_url, "gateway")

            def private_values():
                return [*BOT_TOKENS.values(), *STATE_KEYS.values(), *MODEL_SECRETS.values(),
                        *ISSUED_TOKENS, *EVENT_TOKENS.values(), *(entry["token"] for entry in backend.values())]

            def expect_start_failure():
                result = subprocess.run([str(BINARY), "gateway", "serve", "--config", str(cfg)],
                                        env=ENV, text=True, capture_output=True, timeout=25)
                assert result.returncode != 0, "invalid gateway configuration started"
                assert all(value not in result.stdout + result.stderr for value in private_values()), "credential in startup error"

            def channel_db(who):
                return root / "registry/discord" / (bindings[who]["id"] + ".sqlite3")

            def events(who):
                return rows(channel_db(who), "SELECT * FROM channel_events ORDER BY seq")

            def event(who, update):
                return next((item for item in events(who) if item["event_id"] == EVENT_IDS[(who, update)]), None)

            def deliveries(who, update):
                value = event(who, update)
                return [] if value is None else rows(channel_db(who), "SELECT * FROM channel_outbox WHERE event_id=? ORDER BY ordinal", (value["id"],))

            def hold(who):
                records = rows(Path(settings["registry_path"]), "SELECT * FROM write_holds WHERE user_id=?", (users[who]["user_id"],))
                return records[0] if records else None

            def pending_review(who):
                return wait(lambda: (item if item and item["state"] == "needs_review" else None)
                            if (item := hold(who)) else None, who + " review hold")

            def delivered(who, update):
                def complete():
                    found = deliveries(who, update)
                    return found if found and all(item["state"] == "delivered" for item in found) else None
                try:
                    result = wait(complete, who + " delivery " + str(update), 35)
                    wait(lambda: hold(who) is None, who + " settled hold")
                    return result
                except AssertionError:
                    # Metadata only: never expose prompts, message bodies, notes,
                    # ciphertext or private interaction webhook credentials.
                    current = hold(who)
                    diagnostic = {"tenant": who, "update": update,
                                  "event_status": (event(who, update) or {}).get("status"),
                                  "hold": None if current is None else {key: current[key] for key in ("request_id", "state", "reason")},
                                  "deliveries": [{key: item[key] for key in ("id", "ordinal", "state", "attempts", "error", "next_attempt_ms")}
                                                 for item in deliveries(who, update)[:8]],
                                  "model_calls": model_count(who), "platform_calls": send_count(who)}
                    print("SETTLEMENT DIAGNOSTIC " + json.dumps(diagnostic, sort_keys=True), flush=True)
                    raise

            def payload(who, update, marker):
                identity = IDENTITIES[who]
                key = (who, update)
                EVENT_IDS.setdefault(key, snowflake())
                EVENT_TOKENS.setdefault(key, "interaction." + uuid.uuid4().hex)
                return {"type": 2, "id": EVENT_IDS[key], "application_id": identity["app"],
                        "token": EVENT_TOKENS[key], "version": 1, "context": 1,
                        "authorizing_integration_owners": {"1": identity["sender"]},
                        "user": {"id": identity["sender"], "bot": False},
                        "channel_id": identity["conversation"],
                        "channel": {"id": identity["conversation"], "type": 1},
                        "data": {"id": identity["command"], "type": 1, "name": "jiaclaw",
                                 "options": [{"type": 3, "name": "prompt", "value": "CASE:" + marker}]}}

            def encoded(value):
                return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()

            def signed(who, raw, timestamp=None):
                timestamp = str(int(time.time())) if timestamp is None else str(timestamp)
                with tempfile.NamedTemporaryFile(dir=root, prefix="signature-", suffix=".bin") as source:
                    source.write(timestamp.encode() + raw)
                    source.flush()
                    signature = subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(PRIVATE_KEYS[who]),
                                                "-rawin", "-in", source.name],
                                               check=True, capture_output=True, timeout=5).stdout.hex()
                assert len(signature) == 128
                return {"X-Signature-Timestamp": timestamp, "X-Signature-Ed25519": signature,
                        "Content-Type": "application/json"}

            def webhook(who, value, expected=200, headers=None, query="", raw=None):
                raw = encoded(value) if raw is None else raw
                supplied = signed(who, raw) if headers is None else headers
                request = urllib.request.Request(gateway_url + "/hooks/discord/" + bindings[who]["id"] + query,
                                                 method="POST", data=raw, headers=supplied)
                start = time.monotonic()
                try:
                    response = CLIENT.open(request, timeout=4)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    body = response.read(64 * 1024 + 1)
                    assert len(body) <= 64 * 1024
                    assert response.status == expected, (who, response.status, expected)
                    assert time.monotonic() - start < 3, "Discord ACK exceeded 3-second platform deadline"
                    value = json.loads(body) if body else None
                    if expected == 200 and value != {"type": 1}:
                        assert value == {"type": 5, "data": {"flags": 64}}, "initial deferred reply is not ephemeral"
                    return value

            def inspect(who, kind, *arguments):
                value = cli("discord-inspect", "--binding", bindings[who]["id"], "--kind", kind, *arguments)
                text = json.dumps(value)
                assert all(token not in text for token in EVENT_TOKENS.values()), "admin inspection exposed plaintext interaction token"
                assert "sealed_token" not in text, "admin inspection exposed sealed credential"
                return value["result"][kind]

            def clear(who, ok=True):
                return cli("review-clear", "--user", users[who]["user_id"], "--confirm-backend-idle",
                           "--note", "Local fixture effect, private receipt and idle backend verified; no replay", ok=ok)

            def bearer(who):
                return {"Authorization": "Bearer " + users[who]["token"]}

            def backend_request(who, request_id, update, expected):
                status, value = http(backend[who]["url"], "/internal/channels/discord/requests/" + request_id,
                                     headers={"Authorization": "Bearer " + backend[who]["token"]})
                assert status == 200
                assert value == {"protocol": 3, "backend_id": who, "binding_id": bindings[who]["id"],
                                 "request_id": request_id, "event_id": EVENT_IDS[(who, update)],
                                 "session_id": "discord:" + bindings[who]["id"], "status": expected}, "metadata receipt leaked private fields or changed original identity"

            def resolve(who, delivery):
                with LOCK:
                    receipt = next(item["receipt"] for item in reversed(SENDS) if item["who"] == who)
                return cli("discord-resolve", "--binding", bindings[who]["id"], "--delivery", delivery,
                           "--receipt", "discord:" + receipt)

            for who in ("alice", "bob"):
                key = root / (who + "-ed25519.pem")
                subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(key)],
                               check=True, capture_output=True, timeout=5)
                key.chmod(0o600)
                public_der = subprocess.run(["openssl", "pkey", "-in", str(key), "-pubout", "-outform", "DER"],
                                            check=True, capture_output=True, timeout=5).stdout
                assert public_der.startswith(bytes.fromhex("302a300506032b6570032100")) and len(public_der) == 44
                PRIVATE_KEYS[who], IDENTITIES[who]["verify_key"] = key, public_der[-32:].hex()
                directory = root / who
                workspace = directory / "workspace"
                workspace.mkdir(parents=True)
                backend_token = "private-backend-" + uuid.uuid4().hex
                url = "http://127.0.0.1:" + str(port())
                config = {"agent": {"name": who, "description": "Tenant Discord fixture", "workspace_path": str(workspace),
                                    "system_instructions": "Use the fixed read-only tools.", "max_turns": 10},
                          "provider": {"provider_type": "brokerrouter", "base_url": base,
                                       "api_key": MODEL_SECRETS[who], "model": "wrong-default-route"},
                          "routing": {"channel": {"model": "fixture-channel"}},
                          "http": {"bind": url.removeprefix("http://"), "api_token": backend_token,
                                   "persist": True, "persist_path": "../state/sessions.sqlite3",
                                   "gateway_channel_chat": who == "alice", "shutdown_timeout_secs": 2},
                          "heartbeat": {"enabled": False}}
                path = directory / "config.json"
                path.write_text(json.dumps(config))
                backend[who] = {"config": config, "path": path, "url": url, "token": backend_token,
                                "db": directory / "state/sessions.sqlite3"}
                settings["backends"].append({"id": who, "url": url,
                                             "token_file": secret(who + "-backend.secret", backend_token)})
                backend[who]["process"] = launch(["serve", "--config", str(path)], url, who)
            save()
            for who in backend:
                users[who] = cli("user-add", "--backend", who)
                identity = IDENTITIES[who]
                if who == "bob":
                    cli("discord-bind", "--user", users[who]["user_id"], "--application-id", IDENTITIES["alice"]["app"],
                        "--verify-key", identity["verify_key"], "--bot-user-id", identity["bot_user"],
                        "--sender-id", identity["sender"], "--conversation-id", identity["conversation"],
                        "--command-id", identity["command"], ok=False)
                bindings[who] = cli("discord-bind", "--user", users[who]["user_id"],
                                    "--application-id", identity["app"], "--verify-key", identity["verify_key"],
                                    "--bot-user-id", identity["bot_user"], "--sender-id", identity["sender"],
                                    "--conversation-id", identity["conversation"], "--command-id", identity["command"])
            gateway = start_gateway()
            webhook("alice", payload("alice", 1, "DEFAULT_OFF"), expected=404)
            for path in ("/internal/channels/discord/status", "/internal/channels/discord-binding", "/internal/channels/discord/execute",
                         "/api/channels/events", "/hooks/inbound"):
                assert http(gateway_url, path, headers=bearer("alice"))[0] == 404
            assert http(backend["bob"]["url"], "/internal/channels/discord/status",
                        headers={"Authorization": "Bearer " + backend["bob"]["token"]})[0] == 404
            stop(gateway)
            settings["discord"] = [{"binding_id": bindings[who]["id"],
                                     "bot_token_file": secret(who + "-bot.secret", BOT_TOKENS[who]),
                                     "state_key_file": secret(who + "-state.secret", STATE_KEYS[who]),
                                     "api_base": base + "/api/v10", "allow_loopback": True} for who in backend]
            save()
            expect_start_failure()
            stop(backend["bob"]["process"])
            backend["bob"]["config"]["http"]["gateway_channel_chat"] = True
            backend["bob"]["path"].write_text(json.dumps(backend["bob"]["config"]))
            backend["bob"]["process"] = launch(["serve", "--config", str(backend["bob"]["path"])], backend["bob"]["url"], "bob")
            gateway = start_gateway()
            for fault in ("app", "verify_key", "guild_install", "bot_scope", "bot_identity", "command_context", "command_options", "command_id"):
                stop(gateway)
                with LOCK:
                    STARTUP_FAULT.update(who="alice", case=fault)
                expect_start_failure()
                with LOCK:
                    STARTUP_FAULT.update(who=None, case=None)
                gateway = start_gateway()
            for who in backend:
                assert webhook(who, {"type": 1}) == {"type": 1}
                data = payload(who, 2, "REJECTED")
                raw = encoded(data)
                webhook(who, data, expected=401, headers={})
                webhook(who, None, expected=415, raw=raw, headers=signed(who, raw) | {"Content-Type": "text/plain"})
                webhook(who, None, expected=401, raw=raw, headers=signed("bob" if who == "alice" else "alice", raw))
                wrong = signed(who, raw) | {"X-Signature-Ed25519": "0" * 128}
                before = (rows(Path(settings["registry_path"]), "SELECT seq,action FROM audit_events ORDER BY seq"), hold(who), events(who), model_count(who), send_count(who))
                registry_lock = sqlite3.connect(settings["registry_path"], timeout=2)
                try:
                    registry_lock.execute("BEGIN IMMEDIATE")
                    webhook(who, data, expected=401, headers=wrong)
                finally:
                    registry_lock.rollback()
                    registry_lock.close()
                assert before == (rows(Path(settings["registry_path"]), "SELECT seq,action FROM audit_events ORDER BY seq"), hold(who), events(who), model_count(who), send_count(who))
                for timestamp in (int(time.time()) - 360, int(time.time()) + 360):
                    webhook(who, None, expected=401, raw=raw, headers=signed(who, raw, timestamp))
                webhook(who, None, expected=401, raw=raw.replace(b"REJECTED", b"TAMPERED"), headers=signed(who, raw))
                webhook(who, data, expected=400, query="?backend=other")
                other = IDENTITIES["bob" if who == "alice" else "alice"]
                cases = [(('application_id',), other["app"]), (('context',), 2), (('type',), 3), (('version',), 2),
                         (('guild_id',), "1234567890"), (('member',), {}), (('message',), {}),
                         (('user', 'id'), other["sender"]), (('user', 'bot'), True), (('channel_id',), other["conversation"]),
                         (('channel', 'type'), 3), (('data', 'id'), other["command"]), (('data', 'name'), "arbitrary"),
                         (('data', 'type'), 2), (('data', 'resolved'), {}), (('data', 'options'), []),
                         (('data', 'options'), [{"type": 3, "name": "prompt", "value": "CASE:REJECTED", "options": []}]),
                         (('id',), "01"), (('id',), snowflake(14 * 60 - 100)), (('id',), snowflake(-60)),
                         (('token',), "..")]
                for path, value in cases:
                    bad = payload(who, 2, "REJECTED")
                    target = bad
                    for key in path[:-1]:
                        target = target[key]
                    target[path[-1]] = value
                    webhook(who, bad, expected=400)
                for owner in ({"1": other["sender"]}, {"0": "1234567890", "1": IDENTITIES[who]["sender"]}):
                    bad = payload(who, 2, "REJECTED")
                    bad["authorizing_integration_owners"] = owner
                    webhook(who, bad, expected=400)
                for duplicate in (raw[:-1] + b',"id":"1234567890"}',
                                  raw.replace(b'"name":"jiaclaw"', b'"name":"jiaclaw","name":"jiaclaw"')):
                    webhook(who, None, expected=400, raw=duplicate)
                webhook(who, None, expected=413, raw=encoded(data | {"padding": "x" * 70000}))
                for name in ("X-Signature-Timestamp", "X-Signature-Ed25519"):
                    connection = HTTPConnection(settings["bind"], timeout=4)
                    connection.putrequest("POST", "/hooks/discord/" + bindings[who]["id"])
                    for key, value in signed(who, raw).items():
                        connection.putheader(key, value)
                        if key == name:
                            connection.putheader(key, value)
                    connection.putheader("Content-Length", str(len(raw)))
                    connection.endheaders(raw)
                    response = connection.getresponse()
                    assert response.status == 401
                    response.read()
                    connection.close()
                connection = HTTPConnection(settings["bind"], timeout=4)
                connection.putrequest("POST", "/hooks/discord/" + bindings[who]["id"])
                for key, value in signed(who, raw).items():
                    connection.putheader(key, value)
                connection.putheader("Content-Length", str(len(raw)))
                started = time.monotonic()
                connection.endheaders(raw[:1])
                # While this request holds the binding's one body-reader permit,
                # another complete signed request must fail immediately with 429.
                blocked = False
                while time.monotonic() - started < 1.2:
                    status, _ = http(gateway_url, "/hooks/discord/" + bindings[who]["id"], "POST", data, wrong)
                    assert status in (401, 429), "unexpected concurrent auth/body response"
                    if status == 429:
                        blocked = True
                        break
                    time.sleep(.01)
                assert blocked, "partial request did not retain its bounded body-reader permit"
                response = connection.getresponse()
                assert response.status == 408 and time.monotonic() - started < 3
                response.read()
                connection.close()
                assert not events(who) and model_count(who) == 0 and send_count(who) == 0
            print("PASS 1: default off, backend opt-in, strict application/command manifests, original-byte Ed25519, DM ownership, body/signature/local-reader budgets", flush=True)

            for who in backend:
                data = payload(who, 100, "BASIC_" + who.upper())
                webhook(who, data)
                webhook(who, data)
            for who in backend:
                done = delivered(who, 100)
                assert len(done) == 1 and model_count(who) == 2 and len(events(who)) == 1
                assert done[0]["text"].startswith(who + " CASE:BASIC_")
                with LOCK:
                    sent = next(item for item in SENDS if item["who"] == who)
                assert sent["method"] == "PATCH" and sent["receipt"] == done[0]["receipt"]
                assert "literal <@123456789>" in sent["text"], "mention syntax should remain literal under disabled mentions"
                webhook(who, payload(who, 100, "CHANGED"), expected=409)
                token_conflict = payload(who, 100, "BASIC_" + who.upper())
                token_conflict["token"] = "changed.interaction." + uuid.uuid4().hex
                webhook(who, token_conflict, expected=409)
                assert model_count(who) == 2
                owner = rows(channel_db(who), "SELECT * FROM gateway_discord_owner")[0]
                assert owner["protocol"] == 3 and owner["backend_id"] == who
                assert owner["user_id"] == users[who]["user_id"] and owner["binding_id"] == bindings[who]["id"]
                assert owner["state_key_fingerprint"] == hashlib.sha256(bytes.fromhex(STATE_KEYS[who])).hexdigest()
                assert not rows(channel_db(who), "SELECT * FROM sessions")
                saved = rows(backend[who]["db"], "SELECT id,messages FROM sessions")
                assert len(saved) == 1 and saved[0]["id"] == "discord:" + bindings[who]["id"]
                assert "BASIC_" + who.upper() in saved[0]["messages"]
                assert "BASIC_" + ("BOB" if who == "alice" else "ALICE") not in saved[0]["messages"]
                cli("discord-inspect", "--binding", bindings[who]["id"], "--kind", "events", ok=False)
            stop(gateway)
            alice_db, bob_db = channel_db("alice"), channel_db("bob")
            for path in (alice_db, bob_db):
                assert not Path(str(path) + "-wal").exists(), "closed queue retained uncheckpointed WAL"
            temporary = alice_db.with_suffix(".ownership-swap")
            alice_db.rename(temporary)
            bob_db.rename(alice_db)
            temporary.rename(bob_db)
            try:
                expect_start_failure()
                cli("discord-inspect", "--binding", bindings["alice"]["id"], "--kind", "events", ok=False)
            finally:
                alice_db.rename(temporary)
                bob_db.rename(alice_db)
                temporary.rename(bob_db)
            state_path = Path(settings["discord"][0]["state_key_file"])
            original_key = state_path.read_text()
            state_path.write_text(uuid.uuid4().hex + uuid.uuid4().hex)
            try:
                expect_start_failure()  # Both backend and gateway retain the original key fingerprint.
            finally:
                state_path.write_text(original_key)
            gateway = start_gateway()
            readonly = cli("key-add", "--user", users["alice"]["user_id"], "--read-only")
            cli("key-revoke", "--key", users["alice"]["key_id"])
            users["alice"].update(readonly)
            assert http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))[0] == 403
            webhook("alice", payload("alice", 112, "READ_ONLY_KEY"))
            delivered("alice", 112)
            users["alice"].update(cli("key-add", "--user", users["alice"]["user_id"]))
            print("PASS 2: two real native backends, exact dedup/token conflict, private original receipt, permanent owner/key fingerprints and independent read-only HTTP authority", flush=True)

            before = send_count("alice")
            webhook("alice", payload("alice", 101, "GATE_DISABLE"))
            wait(GATES["GATE_DISABLE"][0].is_set, "model admitted before user disable")
            assert http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))[0] in (409, 429)
            webhook("alice", payload("alice", 111, "QUEUED_UNDER_HOLD"))
            assert event("alice", 111)["status"] == "received" and model_count("alice", "QUEUED_UNDER_HOLD") == 0
            queued = event("alice", 111)
            sealed = queued["sealed_token"]
            assert sealed and EVENT_TOKENS[("alice", 111)] not in sealed
            assert json.loads(queued["spec"])["sealed_token"] is None
            cli("user-disable", "--user", users["alice"]["user_id"])
            webhook("alice", payload("alice", 102, "DISABLED"), expected=403)
            GATES["GATE_DISABLE"][1].set()
            wait(lambda: event("alice", 101)["status"] == "completed", "previously admitted model completes")
            observe_quiet(lambda: send_count("alice") == before, "disabled user sent queued reply")
            cli("user-enable", "--user", users["alice"]["user_id"])
            delivered("alice", 101)
            delivered("alice", 111)
            webhook("alice", payload("alice", 115, "MULTIPART"))
            parts = delivered("alice", 115)
            assert len(parts) == 6, "six-part reply/five followup boundary not exercised"
            with LOCK:
                sends = [item for item in SENDS if item["who"] == "alice" and item["update"] == 115]
            assert [item["method"] for item in sends] == ["PATCH"] + ["POST"] * 5
            assert "".join(item["text"] for item in sends) == "".join(item["text"] for item in parts)
            print("PASS 3: shared HTTP execution admission, encrypted queued tokens, user-disable gate, real Unicode original plus five ephemeral followups", flush=True)

            for update, marker, mode, unknown_ordinal in ((103, "UNKNOWN", "unknown", 0), (116, "FOLLOW_UNKNOWN", "follow_unknown", 1)):
                with LOCK:
                    MODES["alice"] = mode
                before = send_count("alice")
                webhook("alice", payload("alice", update, marker))
                wait(lambda: any(item["state"] == "unknown" for item in deliveries("alice", update)), "ambiguous private platform effect")
                current = deliveries("alice", update)
                first_hold = pending_review("alice")
                assert len(current) == 6 and current[unknown_ordinal]["state"] == "unknown"
                assert all(item["state"] == "delivered" for item in current[:unknown_ordinal])
                assert all(item["state"] == "pending" for item in current[unknown_ordinal + 1:])
                calls = model_count("alice")
                observe_quiet(lambda: send_count("alice") == before + unknown_ordinal + 1 and model_count("alice") == calls,
                              "unknown original/followup replayed model or platform")
                assert http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))[0] == 409
                stop(gateway)
                inspected = inspect("alice", "deliveries", "--event", event("alice", update)["id"])
                assert inspected[unknown_ordinal]["state"] == "unknown"
                assert any(item["request_id"] == first_hold["request_id"] and item["kind"] == "delivery" for item in inspect("alice", "operations"))
                clear("alice", ok=False)
                gateway = start_gateway()
                observe_quiet(lambda: send_count("alice") == before + unknown_ordinal + 1, "restart replayed private webhook", 1.3)
                stop(gateway)
                resolve("alice", current[unknown_ordinal]["id"])
                clear("alice")
                with LOCK:
                    MODES["alice"] = "ok"
                gateway = start_gateway()
                final = delivered("alice", update)
                assert model_count("alice") == calls and send_count("alice") == before + len(final)
            with LOCK:
                MODES["alice"] = "public_receipt"
            before = send_count("alice")
            webhook("alice", payload("alice", 117, "PUBLIC_RECEIPT"))
            wait(lambda: any(item["state"] == "unknown" for item in deliveries("alice", 117)), "public receipt rejected")
            pending_review("alice")
            stop(gateway)
            cli("discord-cancel", "--binding", bindings["alice"]["id"], "--event", event("alice", 117)["id"])
            cli("discord-purge", "--binding", bindings["alice"]["id"], "--event", event("alice", 117)["id"])
            clear("alice")
            with LOCK:
                MODES["alice"] = "ok"
            gateway = start_gateway()
            webhook("alice", payload("alice", 118, "OVERSIZE"))
            wait(lambda: event("alice", 118)["status"] == "needs_review", "oversized whole reply held")
            pending_review("alice")
            assert not deliveries("alice", 118) and send_count("alice") == before + 1, "oversized reply truncated or sent"
            stop(gateway)
            cli("discord-cancel", "--binding", bindings["alice"]["id"], "--event", event("alice", 118)["id"])
            cli("discord-purge", "--binding", bindings["alice"]["id"], "--event", event("alice", 118)["id"])
            clear("alice")
            gateway = start_gateway()
            print("PASS 4: ambiguous original/followup holds block suffix and survive restart; offline inspect/resolve/cancel/purge; public receipts and oversized replies fail closed", flush=True)

            with LOCK:
                MODES["alice"], RATE_COUNTS["alice"] = "rate", 0
            webhook("alice", payload("alice", 104, "RATE"))
            limited = wait(lambda: next((item for item in deliveries("alice", 104) if item["state"] == "retry_wait" and item["attempts"] == 1), None), "persisted first cooldown")
            with LOCK:
                first_send = next(item for item in reversed(SENDS) if item["who"] == "alice")
            assert limited["next_attempt_ms"] >= first_send["time_ms"] + 4900
            first_expiry = limited["expires_ms"]
            stop(gateway)
            gateway = start_gateway()
            while int(time.time() * 1000) < limited["next_attempt_ms"] - 100:
                with LOCK:
                    assert RATE_COUNTS["alice"] == 1, "restart forgot persisted cooldown"
                time.sleep(.05)
            failed = wait(lambda: next((item for item in deliveries("alice", 104) if item["state"] == "permanent_failed" and item["attempts"] == 5), None), "fifth bounded rate attempt", 30)
            pending_review("alice")
            assert failed["expires_ms"] == first_expiry, "retry extended interaction lifetime"
            observe_quiet(lambda: RATE_COUNTS["alice"] == 5, "sixth private webhook attempt")
            stop(gateway)
            clear("alice", ok=False)
            cli("discord-cancel", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
            cli("discord-purge", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
            clear("alice")
            with LOCK:
                MODES["alice"], RATE_COUNTS["alice"] = "rate_expiry", 0
            gateway = start_gateway()
            calls = model_count("alice")
            webhook("alice", payload("alice", 104, "RATE"))
            observe_quiet(lambda: model_count("alice") == calls, "purged event lost dedup tombstone", 1.3)
            webhook("alice", payload("alice", 119, "RATE_EXPIRY"))
            expired = wait(lambda: next((item for item in deliveries("alice", 119) if item["state"] == "expired"), None), "cooldown exceeds fixed token lifetime")
            assert expired["attempts"] == 1 and expired["sealed_token"] is None
            original_hold = pending_review("alice")
            observe_quiet(lambda: RATE_COUNTS["alice"] == 1 and hold("alice")["request_id"] == original_hold["request_id"], "expiry cleared unknown hold or retried")
            stop(gateway)
            cli("discord-cancel", "--binding", bindings["alice"]["id"], "--event", expired["event_id"])
            cli("discord-purge", "--binding", bindings["alice"]["id"], "--event", expired["event_id"])
            clear("alice")
            with LOCK:
                MODES["alice"] = "ok"
            gateway = start_gateway()
            print("PASS 5: persistent decimal cooldown, five-attempt ceiling, fixed expiry and tombstones; excessive cooldown expires sealed delivery credential and retains review hold", flush=True)

            webhook("alice", payload("alice", 105, "GATE_CRASH"))
            wait(GATES["GATE_CRASH"][0].is_set, "real backend model admission before SIGKILL")
            active_hold = hold("alice")
            assert active_hold and active_hold["state"] == "in_flight"
            backend_request("alice", active_hold["request_id"], 105, "admitted")
            before = send_count("alice")
            stop(gateway, kill=True)
            assert gateway.returncode < 0 and backend["alice"]["process"].poll() is None
            gateway = start_gateway()
            assert pending_review("alice")["request_id"] == active_hold["request_id"]
            assert event("alice", 105)["status"] == "needs_review"
            webhook("bob", payload("bob", 106, "BOB_DURING_HOLD"))
            delivered("bob", 106)
            GATES["GATE_CRASH"][1].set()
            wait(lambda: model_count("alice", "GATE_CRASH") == 2, "live backend finishes detached native tool loop")
            wait(lambda: any("GATE_CRASH verified reply" in item["messages"] for item in rows(backend["alice"]["db"], "SELECT messages FROM sessions")), "original backend transaction committed")
            backend_request("alice", active_hold["request_id"], 105, "completed")
            observe_quiet(lambda: model_count("alice", "GATE_CRASH") == 2 and send_count("alice") == before,
                          "restart replayed model or synthesized missing outbox", 1.3)
            stop(gateway)
            record = next(item for item in inspect("alice", "events") if item["id"] == event("alice", 105)["id"])
            assert record["status"] == "needs_review"
            assert any(item["request_id"] == active_hold["request_id"] and item["kind"] == "event" for item in inspect("alice", "operations"))
            clear("alice", ok=False)
            cli("discord-cancel", "--binding", bindings["alice"]["id"], "--event", record["id"])
            clear("alice")
            gateway = start_gateway()
            webhook("alice", payload("alice", 107, "AFTER_REVIEW"))
            delivered("alice", 107)
            assert model_count("alice", "GATE_CRASH") == 2
            print("PASS 6: SIGKILL gateway during actual native backend call, admitted/completed metadata receipt and original session commit; no backend/model replay, Bob remains isolated", flush=True)

            for update, marker, mode, unknown_ordinal in ((113, "SEND_CRASH", "send_crash", 0), (120, "FOLLOW_CRASH", "follow_crash", 1)):
                SEND_GATE[0].clear()
                SEND_GATE[1].clear()
                with LOCK:
                    MODES["alice"] = mode
                before = send_count("alice")
                webhook("alice", payload("alice", update, marker))
                wait(SEND_GATE[0].is_set, "fixture accepted real " + ("PATCH" if unknown_ordinal == 0 else "POST") + " before SIGKILL")
                send_hold = hold("alice")
                assert send_hold and send_hold["state"] == "in_flight"
                submitting = deliveries("alice", update)
                assert submitting[unknown_ordinal]["state"] == "submitting"
                stop(gateway, kill=True)
                SEND_GATE[1].set()
                with LOCK:
                    MODES["alice"] = "ok"
                gateway = start_gateway()
                assert pending_review("alice")["request_id"] == send_hold["request_id"]
                unknown = deliveries("alice", update)
                assert unknown[unknown_ordinal]["state"] == "unknown"
                calls = model_count("alice", marker)
                observe_quiet(lambda: send_count("alice") == before + unknown_ordinal + 1 and model_count("alice", marker) == calls,
                              "SIGKILL recovery replayed accepted private webhook")
                stop(gateway)
                assert any(item["request_id"] == send_hold["request_id"] and item["kind"] == "delivery" for item in inspect("alice", "operations"))
                resolve("alice", unknown[unknown_ordinal]["id"])
                clear("alice")
                gateway = start_gateway()
                final = delivered("alice", update)
                assert send_count("alice") == before + len(final) and model_count("alice", marker) == calls
            print("PASS 7: SIGKILL after actual original PATCH and followup POST, durable original UUID claims, unknown hold and offline receipt review; no webhook replay", flush=True)

            # A genuine unknown send supplies a stable held user. Filling its inbox
            # cannot start another backend call; no synthetic SQL quota is injected.
            with LOCK:
                MODES["bob"] = "unknown"
            webhook("bob", payload("bob", 121, "CAPACITY_HOLD"))
            wait(lambda: any(item["state"] == "unknown" for item in deliveries("bob", 121)), "capacity user's genuine unknown send")
            original = pending_review("bob")
            before_model, before_send = model_count("bob"), send_count("bob")
            queue_lock = sqlite3.connect(channel_db("bob"), timeout=2)
            before_rows = len(events("bob"))
            try:
                queue_lock.execute("BEGIN IMMEDIATE")
                webhook("bob", payload("bob", 122, "STORAGE_BUSY"), expected=503)
            finally:
                queue_lock.rollback()
                queue_lock.close()
            assert len(events("bob")) == before_rows
            available = 1000 - before_rows
            for number in range(available):
                webhook("bob", payload("bob", 5000 + number, "QUEUED_CAPACITY"))
            assert len(events("bob")) == 1000
            webhook("bob", payload("bob", 9000, "CAPACITY_REJECTED"), expected=429)
            assert model_count("bob") == before_model and send_count("bob") == before_send
            assert hold("bob")["request_id"] == original["request_id"]
            stop(gateway)
            assert len(inspect("bob", "events", "--limit", "100")) == 100
            assert len(inspect("bob", "events", "--limit", "100", "--offset", "100")) == 100
            assert inspect("bob", "events", "--limit", "100", "--offset", "10000") == []
            cli("discord-inspect", "--binding", bindings["bob"]["id"], "--kind", "events", "--limit", "101", ok=False)
            clear("bob", ok=False)
            gateway = start_gateway()
            observe_quiet(lambda: model_count("bob") == before_model and send_count("bob") == before_send,
                          "full held inbox replayed a backend/platform request", 1.3)
            print("PASS 8: actual 1000 signed durable inbox admissions, 1001st capacity rejection, SQLite write-busy within ingress deadline, bounded offline pages and shared hold survives restart", flush=True)

            cli("discord-revoke", "--binding", bindings["bob"]["id"])
            webhook("bob", payload("bob", 123, "REVOKED"), expected=403)
            cli("user-enable", "--user", users["bob"]["user_id"])
            webhook("bob", payload("bob", 123, "REVOKED"), expected=403)
            identity = IDENTITIES["bob"]
            cli("discord-bind", "--user", users["bob"]["user_id"], "--application-id", "323456789012345670",
                "--verify-key", identity["verify_key"], "--bot-user-id", identity["bot_user"],
                "--sender-id", identity["sender"], "--conversation-id", identity["conversation"],
                "--command-id", identity["command"], ok=False)
            stop(gateway)
            expect_start_failure()
            settings["discord"] = [entry for entry in settings["discord"] if entry["binding_id"] == bindings["alice"]["id"]]
            save()
            gateway = start_gateway()
            webhook("bob", payload("bob", 123, "REVOKED"), expected=404)
            webhook("alice", payload("alice", 110, "ALICE_STILL_LIVE"))
            delivered("alice", 110)
            clear("bob", ok=False)  # Live gateway lock applies even to revoked runtime-off queues.
            stop(gateway)
            clear("bob", ok=False)  # Stopped retained queue still contains unresolved work.
            cli("discord-cancel", "--binding", bindings["bob"]["id"], "--event", event("bob", 121)["id"])
            clear("bob")  # Only the reviewed unknown effect is cleared; received commands stay queued.
            assert sum(item["status"] == "received" for item in events("bob")) == available
            assert model_count("bob") == before_model and send_count("bob") == before_send
            print("PASS 9: irreversible revoke, retained runtime-off unknown blocks unsafe review-clear, explicit offline cancel preserves received queue and permanent app reservation; Alice continues independently", flush=True)
            for process in PROCESSES:
                stop(process)
            for logfile in root.glob("*.log"):
                contents = logfile.read_text()
                assert all(value not in contents for value in private_values()), "process log exposed a credential"
            for database in root.rglob("*.sqlite3"):
                raw = database.read_bytes()
                assert all(token.encode() not in raw for token in EVENT_TOKENS.values()), "SQLite stored plaintext interaction credential"
            check_errors()
    finally:
        for _, release in GATES.values():
            release.set()
        SEND_GATE[1].set()
        for process in PROCESSES:
            stop(process, kill=True)
        fixture.shutdown()
        fixture.server_close()


if __name__ == "__main__":
    main()
