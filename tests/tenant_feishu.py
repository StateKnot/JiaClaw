#!/usr/bin/env python3
"""Real two-tenant Feishu gateway acceptance; localhost and synthetic secrets only.

Runs the production binary, native model/tool loop, private SQLite queues and CLI.
It does not qualify a real Feishu installation or paid model provider. SQLite observations
are read only; one temporary write-lock transaction rolls back without changing data.
Closed real queue files are exchanged to verify owner rejection. No normal data, clock
or provider fault hook is fabricated.
"""
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import base64
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
ENV.update(JIACLAW_LOG_LEVEL="off,jiaclaw::gateway::feishu=error", JIACLAW_LOG_FORMAT="json",
           HTTP_PROXY="http://127.0.0.1:1",
           HTTPS_PROXY="http://127.0.0.1:1", ALL_PROXY="http://127.0.0.1:1", NO_PROXY="")
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOCK = threading.Lock()
ERRORS, MODEL_REQUESTS, SENDS = [], [], []
MODEL_KEYS = set()
MODEL_SECRETS = {who: "local-model-" + uuid.uuid4().hex for who in ("alice", "bob")}
IDENTITIES = {who: {"app": "cli_LOCAL" + who.upper(), "tenant": "tenant_" + who,
                   "bot": "ou_bot_" + who, "human": "ou_human_" + who, "chat": "oc_private_" + who}
              for who in ("alice", "bob")}
APP_SECRETS = {who: "app_" + uuid.uuid4().hex + uuid.uuid4().hex for who in IDENTITIES}
ENCRYPT_KEYS = {who: "enc_" + uuid.uuid4().hex + uuid.uuid4().hex for who in IDENTITIES}
VERIFICATION_TOKENS = {who: "verify_" + uuid.uuid4().hex for who in IDENTITIES}
BOT_TOKENS = {who: "tenant_token_" + uuid.uuid4().hex for who in IDENTITIES}
AUTH_GATE = (threading.Event(), threading.Event())
AUTH_GATED = {who: False for who in IDENTITIES}
AUTH_CALLS = []
MODES = {who: "ok" for who in IDENTITIES}
RATE_COUNTS = {who: 0 for who in IDENTITIES}
HANDSHAKES, ISSUED_TOKENS = [], []
STARTUP_FAULT = {"who": None, "case": None}
GATES = {name: (threading.Event(), threading.Event())
         for name in ("GATE_DISABLE", "GATE_CRASH", "GATE_REVOKE")}
SEND_GATE = (threading.Event(), threading.Event())

PROCESSES = []


def check_errors():
    with LOCK:
        assert not ERRORS, ERRORS


def wait(predicate, label, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        check_errors()
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError("Timeout: " + label)


def observe_quiet(predicate, label, seconds=4.2):
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



def finish_log_metadata(path, request_id, user_id):
    """Extract only the fixed Feishu settlement event from a bounded log tail."""
    if request_id is None:
        return []
    try:
        with path.open("rb") as source:
            size = source.seek(0, os.SEEK_END)
            start = max(0, size - 64 * 1024)
            source.seek(start)
            lines = source.read(64 * 1024).splitlines()
            if start:
                lines = lines[1:]  # A truncated first record is not evidence.
    except OSError:
        return []
    allowed_codes = {"io_busy", "io_task_failed", "registry_busy", "registry_storage",
                     "registry_io", "registry_validation"}
    found = []
    for line in lines:
        if len(line) > 8 * 1024:
            continue
        try:
            record = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(record, dict):
            continue
        fields = record.get("fields")
        if (record.get("target") != "jiaclaw::gateway::feishu" or record.get("level") != "ERROR"
                or not isinstance(fields, dict)
                or fields.get("message") != "Feishu write hold requires administrator review"
                or fields.get("request_id") != request_id or fields.get("user_id") != user_id
                or type(fields.get("code")) is not str or fields["code"] not in allowed_codes
                or type(fields.get("known")) is not bool):
            continue
        found.append({key: fields[key] for key in ("code", "request_id", "user_id", "known")})
    return found[-8:]


def model_count(who, marker=None):
    with LOCK:
        return sum(entry["who"] == who and (marker is None or entry["marker"] == marker)
                   for entry in MODEL_REQUESTS)


def send_count(who):
    with LOCK:
        return sum(entry["who"] == who for entry in SENDS)


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

    def dispatch(self, method):
        try:
            length = int(self.headers.get("Content-Length", "0"))
            assert length <= 2 * 1024 * 1024, "unbounded fixture request"
            raw = self.rfile.read(length)
            body = json.loads(raw) if raw else {}
            path = urlsplit(self.path).path
            if path == "/v1/chat/completions":
                assert method == "POST"
                self.model(body)
            elif path == "/open-apis/im/v1/messages":
                assert method == "POST"
                self.platform(body)
            elif path == "/open-apis/auth/v3/tenant_access_token/internal":
                assert method == "POST"
                self.token(body)
            else:
                self.handshake(method)
        except (BrokenPipeError, ConnectionResetError):
            pass  # A killed gateway can disconnect after the local side effect.
        except Exception as error:
            frame = traceback.extract_tb(error.__traceback__)[-1]
            with LOCK:
                ERRORS.append(type(error).__name__ + ": " + Path(frame.filename).name + ":" + str(frame.lineno))
            try:
                self.reply(500, {"error": "local fixture failure"})
            except (BrokenPipeError, ConnectionResetError):
                pass

    def identity(self):
        assert len(self.headers.get_all("Authorization", [])) == 1, "duplicate platform authorization"
        who = next(who for who, token in BOT_TOKENS.items()
                   if self.headers.get("Authorization") == "Bearer " + token)
        return who, IDENTITIES[who]

    def handshake(self, method):
        who, identity = self.identity()
        url = urlsplit(self.path)
        assert not url.query
        with LOCK:
            HANDSHAKES.append({"who": who, "method": method, "path": url.path})
            fault = STARTUP_FAULT["case"] if STARTUP_FAULT["who"] == who else None
        assert method == "GET"
        if url.path == "/open-apis/bot/v3/info":
            result = {"code": 0, "msg": "ok", "bot": {"open_id": identity["bot"], "activate_status": 2}}
            if fault == "bot_id":
                result["bot"]["open_id"] = "ou_foreign_bot"
            elif fault == "bot_inactive":
                result["bot"]["activate_status"] = 1
        elif url.path == "/open-apis/tenant/v2/tenant/query":
            result = {"code": 0, "msg": "ok", "data": {"tenant": {"tenant_key": identity["tenant"]}}}
            if fault == "tenant":
                result["data"]["tenant"]["tenant_key"] = "other_tenant"
        elif url.path == "/open-apis/im/v1/chats/" + identity["chat"]:
            # p2p does not promise group-only response fields or a chat_id echo.
            data = {"chat_mode": "p2p"}
            if who == "alice":
                data.update(tenant_key=identity["tenant"], external=False)
            result = {"code": 0, "msg": "ok", "data": data}
            if fault == "group":
                data.update(chat_mode="group", chat_type="private")
            elif fault == "external":
                data["external"] = True
            elif fault == "chat_tenant":
                data["tenant_key"] = "other_tenant"
            elif fault == "missing_mode":
                data.pop("chat_mode")
        else:
            raise AssertionError("unexpected platform API")
        self.reply(200, result)

    def token(self, body):
        assert not self.headers.get_all("Authorization", []), "auth credentials leaked into bearer auth"
        who = next(who for who, identity in IDENTITIES.items() if body.get("app_id") == identity["app"])
        assert body == {"app_id": IDENTITIES[who]["app"], "app_secret": APP_SECRETS[who]}
        with LOCK:
            AUTH_CALLS.append({"who": who, "time": time.monotonic()})
            gated = AUTH_GATED[who]
        if gated:
            AUTH_GATE[0].set()
            assert AUTH_GATE[1].wait(60), "fixture token gate expired"
        self.reply(200, {"code": 0, "msg": "ok", "expire": 7200, "tenant_access_token": BOT_TOKENS[who]})

    def model(self, body):
        who = next(who for who, secret in MODEL_SECRETS.items()
                   if self.headers.get("Authorization") == "Bearer " + secret)
        assert body["model"] == "fixture-channel", "channel did not select configured route"
        assert sorted(tool["function"]["name"] for tool in body["tools"]) == ["datetime_now", "json_query"]
        assert body["parallel_tool_calls"] is False
        prompt = next(message["content"] for message in reversed(body["messages"])
                      if message["role"] == "user")
        marker = re.search(r"CASE:([A-Z_]+)", prompt).group(1)
        last = body["messages"][-1]
        key = self.headers.get("Idempotency-Key")
        with LOCK:
            assert key and key not in MODEL_KEYS, "model request identity was replayed"
            MODEL_KEYS.add(key)
            MODEL_REQUESTS.append({"who": who, "marker": marker, "last_role": last["role"]})
        if last["role"] != "tool":
            if marker in GATES:
                GATES[marker][0].set()
                assert GATES[marker][1].wait(60), "fixture model gate expired"
            call = {"id": "call_" + uuid.uuid4().hex, "type": "function",
                    "function": {"name": "datetime_now", "arguments": "{}"}}
            message, reason = {"role": "assistant", "content": None, "tool_calls": [call]}, "tool_calls"
        else:
            previous = body["messages"][-2]
            assert previous["role"] == "assistant" and previous["tool_calls"][0]["id"] == last["tool_call_id"]
            assert "error" not in json.loads(last["content"]), "real tool execution failed"
            content = who + " CASE:" + marker + " verified reply"
            if marker.startswith("BASIC_"):
                content += " literal <@UFAKE> & <https://example.invalid/x>"
            if marker == "UNKNOWN":
                content += " 🧭界" * 1000  # Multiple real UTF-16-bounded platform sends.
            message, reason = {"role": "assistant", "content": content}, "stop"
        self.reply(200, {"choices": [{"message": message, "finish_reason": reason}]})

    def platform(self, body):
        who, identity = self.identity()
        assert parse_qs(urlsplit(self.path).query) == {"receive_id_type": ["chat_id"]}
        assert body["receive_id"] == identity["chat"], "cross-tenant platform destination"
        assert body["msg_type"] == "text" and "reply_in_thread" not in body
        text = json.loads(body["content"])["text"]
        assert text.strip() and "<" not in text and ">" not in text, "Feishu inline mention controls were not escaped"
        assert len(text.encode("utf-16-le")) // 2 <= 2000
        assert str(uuid.UUID(body["uuid"])) == body["uuid"]
        with LOCK:
            mode = MODES[who]
            receipt = "om_localreceipt_" + str(len(SENDS) + 1)
            SENDS.append({"who": who, "time_ms": int(time.time() * 1000), "mode": mode,
                          "text": text, "receipt": receipt, "uuid": body["uuid"]})
            if mode == "rate":
                RATE_COUNTS[who] += 1
                attempt = RATE_COUNTS[who]
        if mode == "unknown":
            self.reply(503, {"code": 1})  # Locally accepted effect, ambiguous receipt.
        elif mode == "rate":
            self.reply(429, {"code": 99991400}, {"x-ogw-ratelimit-reset": "5" if attempt == 1 else "1"})
        elif mode == "invalid_token":
            self.reply(400, {"code": 99991663})  # A documented rejection, never refresh and replay this message.
        else:
            if mode == "send_crash":
                SEND_GATE[0].set()
                assert SEND_GATE[1].wait(60), "fixture send gate expired"
            data = {"message_id": receipt, "chat_id": identity["chat"], "msg_type": "text"}
            if mode == "wrong_receipt":
                data["chat_id"] = IDENTITIES["bob" if who == "alice" else "alice"]["chat"]
            self.reply(200, {"code": 0, "data": data})


def rows(path, query, parameters=()):
    # Match the model_calls fixture: mode=rw only opens an existing database
    # and permits WAL bookkeeping after a checkpoint on older SQLite builds.
    # query_only forbids SQL mutation; never mark a live worker DB immutable.
    connection = sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, timeout=2)
    try:
        connection.execute("PRAGMA query_only=ON")
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(query, parameters)]
    finally:
        connection.close()


def main():
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), LocalHandler)
    fixture.daemon_threads = True
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="jiaclaw-gateway-feishu-") as temp:
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
                                        env=ENV, text=True, capture_output=True, timeout=15)
                if ok:
                    assert result.returncode == 0, (arguments[0], "CLI failed", result.returncode)
                    parsed = json.loads(result.stdout)
                    if isinstance(parsed, dict) and "token" in parsed:
                        ISSUED_TOKENS.append(parsed["token"])
                    return parsed
                assert result.returncode != 0, (arguments[0], "unexpected CLI success")
                return result

            def launch(arguments, url, name):
                log = root / (name + ".log")
                with log.open("ab") as output:
                    process = subprocess.Popen([str(BINARY), *arguments], env=ENV, stdout=output, stderr=output)
                PROCESSES.append(process)
                def healthy():
                    assert process.poll() is None, name + " startup failed; inspect local private log"
                    try:
                        return http(url, "/health")[0] == 200
                    except (OSError, urllib.error.URLError):
                        return False
                wait(healthy, name + " startup")
                return process

            def start_gateway():
                return launch(["gateway", "serve", "--config", str(cfg)], gateway_url, "gateway")

            def expect_start_failure():
                result = subprocess.run([str(BINARY), "gateway", "serve", "--config", str(cfg)],
                                        env=ENV, text=True, capture_output=True, timeout=15)
                assert result.returncode != 0, "invalid gateway configuration started"
                private_values = [*BOT_TOKENS.values(), *APP_SECRETS.values(), *VERIFICATION_TOKENS.values(), *ENCRYPT_KEYS.values(), *MODEL_SECRETS.values(),
                                  *ISSUED_TOKENS, *(entry["token"] for entry in backend.values())]
                assert all(value not in result.stdout + result.stderr for value in private_values), "credential in startup error"

            def channel_db(who):
                return root / "registry/feishu" / (bindings[who]["id"] + ".sqlite3")

            def events(who):
                return rows(channel_db(who), "SELECT * FROM channel_events ORDER BY seq")

            def event(who, update):
                return next((item for item in events(who) if item["event_id"] == "om_" + str(update)), None)

            def deliveries(who, update):
                value = event(who, update)
                return [] if value is None else rows(channel_db(who), "SELECT * FROM channel_outbox WHERE event_id=? ORDER BY ordinal", (value["id"],))

            def delivered(who, update):
                def complete():
                    found = deliveries(who, update)
                    return found if found and all(item["state"] == "delivered" for item in found) else None
                result = wait(complete, who + " delivered " + str(update), 25)
                try:
                    wait(lambda: hold(who) is None, who + " settled hold")
                except AssertionError:
                    # Outbox and registry commits are separate transactions.
                    # Keep the original failure and expose only bounded state
                    # metadata, never queue text, credentials or private notes.
                    settlement_diagnostics(who, update)
                    raise
                return result

            def settlement_diagnostics(who, update):
                diagnostic = {"tenant": who, "update_id": update}
                try:
                    current = hold(who)
                    diagnostic["hold"] = None if current is None else {
                        key: current.get(key) for key in
                        ("request_id", "state", "admitted_ms", "updated_ms")}
                    if current is not None:
                        reason = current.get("reason")
                        diagnostic["hold"]["reason"] = reason if reason in {
                            "request_in_flight", "backend_outcome_unknown", "gateway_restarted"
                        } else "unrecognized_reason"
                    known_states = {"pending", "submitting", "delivered", "retry_wait", "unknown",
                                    "permanent_failed", "expired", "cancelled"}
                    known_errors = {"binding_mismatch", "rate_limited", "transport_error", "http_server_error",
                                    "http_client_error", "unexpected_http_status", "response_too_large",
                                    "response_read_error", "invalid_response", "invalid_receipt",
                                    "feishu_error", "feishu_rejected", "invalid_rate_limit"}
                    diagnostic["deliveries"] = [{
                        **{key: item.get(key) for key in ("id", "ordinal", "attempts", "started_ms", "finished_ms")},
                        "state": item["state"] if item.get("state") in known_states else "unrecognized_state",
                        "error": None if item.get("error") is None else
                            (item["error"] if item["error"] in known_errors else "unrecognized_error")
                    } for item in deliveries(who, update)[:16]]
                    diagnostic["finish_errors"] = finish_log_metadata(
                        root / "gateway.log", current.get("request_id") if current else None,
                        users[who]["user_id"])
                    # The administrator CLI validates metadata and omits note
                    # extraction by default. Failures reveal only an exit code.
                    audit = subprocess.run([str(BINARY), "gateway", "audit-list", "--config", str(cfg),
                                            "--user", users[who]["user_id"], "--limit", "100"],
                                           env=ENV, text=True, capture_output=True, timeout=15)
                    diagnostic["audit_exit_code"] = audit.returncode
                    if audit.returncode == 0 and len(audit.stdout.encode()) <= 512 * 1024:
                        page = json.loads(audit.stdout)
                        assert page["notes_included"] is False
                        request_id = current.get("request_id") if current else None
                        matches = [item for item in page["events"] if item.get("request_id") == request_id
                                   and request_id is not None]
                        known_actions = {"feishu_execute", "feishu_send", "write_completed",
                                         "write_needs_review", "write_recovered_for_review", "write_review_cleared"}
                        diagnostic["audit"] = [{
                            **{key: item.get(key) for key in ("seq", "user_id", "key_id", "request_id", "created_ms")},
                            "action": item["action"] if item.get("action") in known_actions else "unrecognized_action"
                        } for item in matches[:8]]
                        diagnostic["audit_has_more"] = page["has_more"]
                except Exception as error:
                    diagnostic["diagnostic_error"] = type(error).__name__
                print("SETTLEMENT DIAGNOSTIC " + json.dumps(diagnostic, sort_keys=True), flush=True)

            def hold(who):
                records = rows(Path(settings["registry_path"]), "SELECT * FROM write_holds WHERE user_id=?", (users[who]["user_id"],))
                return records[0] if records else None

            def pending_review(who):
                return wait(lambda: (item if item and item["state"] == "needs_review" else None)
                            if (item := hold(who)) else None, who + " review hold")

            def payload(who, update, marker):
                identity = IDENTITIES[who]
                return {"schema": "2.0", "header": {"event_id": "ev_" + uuid.uuid4().hex,
                        "event_type": "im.message.receive_v1", "app_id": identity["app"],
                        "tenant_key": identity["tenant"], "token": VERIFICATION_TOKENS[who],
                        "create_time": str(int(time.time() * 1000))},
                        "event": {"sender": {"sender_type": "user", "sender_id": {"open_id": identity["human"]},
                                             "tenant_key": identity["tenant"]},
                                  "message": {"message_id": "om_" + str(update), "chat_id": identity["chat"],
                                              "chat_type": "p2p", "message_type": "text",
                                              "content": json.dumps({"text": "CASE:" + marker})}}}

            def encoded(value):
                return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()

            def encrypted(who, value):
                iv = os.urandom(16)
                result = subprocess.run(["openssl", "enc", "-aes-256-cbc", "-K", hashlib.sha256(ENCRYPT_KEYS[who].encode()).hexdigest(),
                                         "-iv", iv.hex()], input=encoded(value), capture_output=True, timeout=5)
                assert result.returncode == 0, "independent callback encryption failed"
                return encoded({"encrypt": base64.b64encode(iv + result.stdout).decode()})

            def signed(who, raw, timestamp=None):
                timestamp = str(int(time.time())) if timestamp is None else str(timestamp)
                nonce = uuid.uuid4().hex
                digest = hashlib.sha256(timestamp.encode() + nonce.encode() + ENCRYPT_KEYS[who].encode() + raw).hexdigest()
                return {"X-Lark-Request-Timestamp": timestamp, "X-Lark-Request-Nonce": nonce,
                        "X-Lark-Signature": digest, "Content-Type": "application/json"}

            def webhook(who, value, expected=200, headers=None, query="", raw=None,
                        expected_code=None, return_status=False):
                raw = encrypted(who, value) if raw is None else raw
                supplied = signed(who, raw) if headers is None else headers
                request = urllib.request.Request(gateway_url + "/hooks/feishu/" + bindings[who]["id"] + query,
                                                 method="POST", data=raw, headers=supplied)
                start = time.monotonic()
                try:
                    response = CLIENT.open(request, timeout=4)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    body = response.read(64 * 1024 + 1)
                    assert len(body) <= 64 * 1024, "unbounded webhook response"
                    parsed = json.loads(body) if body else None
                    code = parsed.get("status") if isinstance(parsed, dict) else None
                    known_codes = {None, "accepted", "ignored", "invalid_callback", "body_timeout",
                                   "busy", "queue_full", "admission_failed", "storage_unavailable",
                                   "ingress_deadline", "binding_disabled", "unknown_binding", "disabled",
                                   "json_required", "query_not_allowed", "invalid_body", "event_not_authorized",
                                   "fingerprint_conflict"}
                    safe_code = code if type(code) in (str, type(None)) and code in known_codes else "unrecognized_code"
                    elapsed = time.monotonic() - start
                    diagnostic = {"tenant": who, "http_status": response.status, "code": safe_code,
                                  "elapsed_ms": round(elapsed * 1000)}
                    allowed = (expected,) if isinstance(expected, int) else expected
                    assert response.status in allowed, diagnostic
                    assert expected_code is None or code == expected_code, diagnostic
                    assert elapsed < 1, {**diagnostic, "error": "ingress_budget_exceeded"}
                    return (response.status, parsed) if return_status else parsed

            def inspect(who, kind, *arguments):
                return cli("feishu-inspect", "--binding", bindings[who]["id"], "--kind", kind, *arguments)["result"][kind]

            def clear(who, ok=True):
                return cli("review-clear", "--user", users[who]["user_id"], "--confirm-backend-idle",
                           "--note", "Local fixture external result and backend idle verified; no replay", ok=ok)

            def bearer(who):
                return {"Authorization": "Bearer " + users[who]["token"]}

            def backend_request(who, request_id, update, expected):
                status, value = http(backend[who]["url"], "/internal/channels/feishu/requests/" + request_id,
                                     headers={"Authorization": "Bearer " + backend[who]["token"]})
                assert status == 200, "original backend request metadata unavailable"
                assert value == {"protocol": 4, "backend_id": who, "binding_id": bindings[who]["id"],
                                 "request_id": request_id, "event_id": "om_" + str(update),
                                 "session_id": "feishu:" + bindings[who]["id"], "status": expected}, "unexpected request metadata or private fields"
                return value

            for who in ("alice", "bob"):
                directory = root / who
                workspace = directory / "workspace"
                workspace.mkdir(parents=True)
                backend_token = "private-backend-" + uuid.uuid4().hex
                url = "http://127.0.0.1:" + str(port())
                config = {"agent": {"name": who, "description": "Tenant Feishu fixture", "workspace_path": str(workspace),
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
                                "db": directory / "state/sessions.sqlite3", "signing": ENCRYPT_KEYS[who]}
                settings["backends"].append({"id": who, "url": url,
                                             "token_file": secret(who + "-backend.secret", backend_token)})
                backend[who]["process"] = launch(["serve", "--config", str(path)], url, who)
            save()
            for who in backend:
                users[who] = cli("user-add", "--backend", who)
                identity = IDENTITIES[who]
                if who == "bob":
                    # A dedicated app remains reserved across workspaces, not
                    # just for one (tenant, app) installation pair.
                    cli("feishu-bind", "--user", users[who]["user_id"],
                        "--tenant-key", identity["tenant"], "--app-id", IDENTITIES["alice"]["app"],
                        "--bot-open-id", identity["bot"], "--human-open-id", identity["human"], "--chat-id", identity["chat"], ok=False)
                bindings[who] = cli("feishu-bind", "--user", users[who]["user_id"],
                                    "--tenant-key", identity["tenant"], "--app-id", identity["app"],
                                    "--bot-open-id", identity["bot"], "--human-open-id", identity["human"], "--chat-id", identity["chat"])
            gateway = start_gateway()
            webhook("alice", payload("alice", 1, "DEFAULT_OFF"), expected=404)
            for path in ("/internal/channels/feishu/status", "/internal/channels/feishu-binding", "/internal/channels/feishu/execute",
                         "/internal/gateway/channel/status", "/internal/gateway/channel/chat",
                         "/api/channels/events", "/hooks/inbound"):
                assert http(gateway_url, path, headers=bearer("alice"))[0] == 404, path
            assert http(backend["bob"]["url"], "/internal/channels/feishu/status",
                        headers={"Authorization": "Bearer " + backend["bob"]["token"]})[0] == 404
            stop(gateway)
            for who in backend:
                assert inspect(who, "events") == [] and inspect(who, "operations") == []
                cli("feishu-cancel", "--binding", bindings[who]["id"], "--event", str(uuid.uuid4()), ok=False)
            assert not (root / "registry/feishu").exists(), "offline inspection adopted or created empty queue state"
            settings["feishu"] = [{"binding_id": bindings[who]["id"],
                                     "app_secret_file": secret(who + "-app.secret", APP_SECRETS[who]),
                                     "encrypt_key_file": secret(who + "-encrypt.secret", ENCRYPT_KEYS[who]),
                                     "verification_token_file": secret(who + "-verification.secret", VERIFICATION_TOKENS[who]),
                                     "api_base": base + "/open-apis", "allow_loopback": True} for who in backend]
            save()
            expect_start_failure()  # Bob's real backend has not opted into the contract.
            stop(backend["bob"]["process"])
            backend["bob"]["config"]["http"]["gateway_channel_chat"] = True
            backend["bob"]["path"].write_text(json.dumps(backend["bob"]["config"]))
            backend["bob"]["process"] = launch(["serve", "--config", str(backend["bob"]["path"])], backend["bob"]["url"], "bob")
            gateway = start_gateway()
            stop(gateway)
            app_path = Path(settings["feishu"][0]["app_secret_file"])
            app_path.chmod(0o640)
            try:
                expect_start_failure()
            finally:
                app_path.chmod(0o600)
            real = app_path.with_suffix(".real")
            app_path.rename(real)
            app_path.symlink_to(real)
            try:
                expect_start_failure()
            finally:
                app_path.unlink()
                real.rename(app_path)
            alias = app_path.with_suffix(".alias")
            os.link(app_path, alias)
            try:
                expect_start_failure()
            finally:
                alias.unlink()
            original_path = settings["feishu"][0]["app_secret_file"]
            settings["feishu"][0]["app_secret_file"] = settings["feishu"][0]["encrypt_key_file"]
            save()
            expect_start_failure()
            settings["feishu"][0]["app_secret_file"] = original_path
            save()
            gateway = start_gateway()
            for fault in ("bot_id", "bot_inactive", "tenant", "group", "external", "chat_tenant", "missing_mode"):
                stop(gateway)
                with LOCK:
                    STARTUP_FAULT.update(who="alice", case=fault)
                expect_start_failure()
                with LOCK:
                    STARTUP_FAULT.update(who=None, case=None)
                gateway = start_gateway()
            for who in backend:
                challenge = {"type": "url_verification", "token": VERIFICATION_TOKENS[who],
                             "challenge": "bounded-local-challenge-" + who}
                assert webhook(who, challenge, headers={"Content-Type": "application/json"}) == {"challenge": challenge["challenge"]}
                assert webhook(who, challenge, headers={"Content-Type": "application/json"}, raw=encoded(challenge)) == {"challenge": challenge["challenge"]}
                for invalid_challenge in ("", "x" * 1025):
                    webhook(who, challenge | {"challenge": invalid_challenge}, expected=401)
                webhook(who, challenge | {"token": VERIFICATION_TOKENS["bob" if who == "alice" else "alice"]}, expected=401)
                data = payload(who, 2, "REJECTED")
                raw = encoded(data)
                webhook(who, None, expected=401, raw=raw, headers={"Content-Type": "application/json"})
                webhook(who, None, expected=415, raw=raw, headers=signed(who, raw) | {"Content-Type": "text/plain"})
                wrong = signed(who, raw) | {"X-Lark-Signature": "0" * 64}
                webhook(who, None, expected=401, raw=raw, headers=wrong)
                def unchanged_state():
                    return (rows(Path(settings["registry_path"]), "SELECT seq,action FROM audit_events ORDER BY seq"),
                            hold(who), events(who), model_count(who), send_count(who))
                before_invalid_mac = unchanged_state()
                registry_lock = sqlite3.connect(Path(settings["registry_path"]).as_uri() + "?mode=rw", uri=True, timeout=2)
                try:
                    registry_lock.execute("BEGIN IMMEDIATE")
                    webhook(who, None, expected=401, raw=raw, headers=wrong)
                finally:
                    registry_lock.rollback()
                    registry_lock.close()
                assert unchanged_state() == before_invalid_mac, "invalid MAC changed audit/hold/queue/model/send"
                for timestamp in (int(time.time()) - 360, int(time.time()) + 360):
                    webhook(who, None, expected=401, raw=raw, headers=signed(who, raw, timestamp))
                webhook(who, None, expected=401, raw=raw.replace(b"REJECTED", b"TAMPERED"), headers=signed(who, raw))
                webhook(who, data, expected=400, query="?backend=other")
                other = IDENTITIES["bob" if who == "alice" else "alice"]
                cases = [(("header", "tenant_key"), other["tenant"], 401),
                         (("header", "app_id"), other["app"], 401),
                         (("header", "token"), VERIFICATION_TOKENS["bob" if who == "alice" else "alice"], 401),
                         (("event", "sender", "tenant_key"), other["tenant"], 401),
                         (("event", "sender", "sender_id", "open_id"), other["human"], 400),
                         (("event", "message", "chat_id"), other["chat"], 400),
                         (("event", "message", "chat_type"), "group", 400),
                         (("event", "message", "message_id"), "invalid-message", 401)]
                for path, value, expected in cases:
                    bad = payload(who, 2, "REJECTED")
                    target = bad
                    for key in path[:-1]:
                        target = target[key]
                    target[path[-1]] = value
                    webhook(who, bad, expected=expected)
                before_large_text = unchanged_state()
                for size in (16385, 32768):
                    too_long = payload(who, 2, "OVERSIZED")
                    prefix = "CASE:OVERSIZED "
                    too_long["event"]["message"]["content"] = json.dumps({"text": prefix + "x" * (size - len(prefix))})
                    webhook(who, too_long, expected=400)
                    assert unchanged_state() == before_large_text, "oversized signed prompt changed audit/hold/queue/model/send"
                for path, value in [(("event", "sender", "sender_type"), "app"),
                                    (("event", "message", "message_type"), "image")]:
                    ignored = payload(who, 2, "REJECTED")
                    ignored["event"][path[1]][path[2]] = value
                    webhook(who, ignored)
                for original, duplicate in [(b'"sender_type":"user"', b'"sender_type":"user","sender_type":"app"'),
                                            (b'"message_id":"om_2"', b'"message_id":"om_2","message_id":"om_other"'),
                                            (b'"chat_id":', b'"chat_id":"oc_other","chat_id":')]:
                    duplicate_raw = raw.replace(original, duplicate)
                    assert duplicate_raw != raw
                    webhook(who, None, expected=401, raw=duplicate_raw)
                webhook(who, None, expected=401, raw=encoded([1,2,3]))
                oversized = encoded(data | {"padding": "x" * (129 * 1024)})
                webhook(who, None, expected=413, raw=oversized)
                oversized_plain = data | {"padding": "x" * (65 * 1024)}
                webhook(who, oversized_plain, expected=401)
                for name in ("X-Lark-Signature", "X-Lark-Request-Timestamp", "X-Lark-Request-Nonce"):
                    connection = HTTPConnection(settings["bind"], timeout=4)
                    connection.putrequest("POST", "/hooks/feishu/" + bindings[who]["id"])
                    for key, value in signed(who, raw).items():
                        connection.putheader(key, value)
                        if key == name:
                            connection.putheader(key, value)
                    connection.putheader("Content-Length", str(len(raw)))
                    connection.endheaders(raw)
                    response = connection.getresponse()
                    assert response.status == 401, "duplicate Feishu auth header accepted"
                    response.read()
                    connection.close()
                connection = HTTPConnection(settings["bind"], timeout=4)
                connection.putrequest("POST", "/hooks/feishu/" + bindings[who]["id"])
                for key, value in signed(who, raw).items():
                    connection.putheader(key, value)
                connection.putheader("Content-Length", str(len(raw)))
                start = time.monotonic()
                connection.endheaders(raw[:1])
                time.sleep(.05)
                webhook(who, challenge, expected=429)
                response = connection.getresponse()
                assert response.status == 408 and time.monotonic() - start < 1, "Feishu body deadline was not enforced"
                response.read()
                connection.close()
                assert not events(who) and model_count(who) == 0 and send_count(who) == 0
                methods = {(item["method"], item["path"]) for item in HANDSHAKES if item["who"] == who}
                assert {("GET", "/open-apis/bot/v3/info"), ("GET", "/open-apis/tenant/v2/tenant/query"),
                        ("GET", "/open-apis/im/v1/chats/" + IDENTITIES[who]["chat"])} <= methods
            print("PASS 1: default-off/protocol opt-in, real startup identity gates, encrypted/plain challenge, original MAC, duplicate fields and 900ms callback resource budget", flush=True)

            for who in backend:
                data = payload(who, 100, "BASIC_" + who.upper())
                webhook(who, data)
                repeated = payload(who, 100, "BASIC_" + who.upper())  # Same business message, different delivery event ID.
                webhook(who, repeated, raw=encoded(repeated))
            for who in backend:
                done = delivered(who, 100)
                assert len(done) == 1 and done[0]["text"].startswith(who + " CASE:BASIC_")
                assert model_count(who) == 2 and len(events(who)) == 1
                with LOCK:
                    sent = next(item for item in SENDS if item["who"] == who)
                assert done[0]["receipt"] == sent["receipt"], "Feishu message_id receipt was not preserved"
                assert "＜@UFAKE＞ & ＜https://example.invalid/x＞" in sent["text"], "literal mention/link escaping missing"
                webhook(who, payload(who, 100, "FINGERPRINT_CHANGED"), expected=409)
                assert model_count(who) == 2, "changed fingerprint replayed an existing event"
                owner = rows(channel_db(who), "SELECT * FROM gateway_feishu_owner")[0]
                assert owner["protocol"] == 4 and owner["backend_id"] == who
                assert owner["user_id"] == users[who]["user_id"] and owner["binding_id"] == bindings[who]["id"]
                assert not rows(channel_db(who), "SELECT * FROM sessions"), "channel DB copied private backend history"
                saved = rows(backend[who]["db"], "SELECT id,messages FROM sessions")
                assert len(saved) == 1 and saved[0]["id"] == "feishu:" + bindings[who]["id"]
                assert "BASIC_" + who.upper() in saved[0]["messages"]
                assert "BASIC_" + ("BOB" if who == "alice" else "ALICE") not in saved[0]["messages"]
                cli("feishu-inspect", "--binding", bindings[who]["id"], "--kind", "events", ok=False)
            stop(gateway)
            # Swap complete, closed private databases; no SQL is fabricated. The
            # permanent owner contract must reject each valid but foreign file.
            alice_db, bob_db = channel_db("alice"), channel_db("bob")
            for path in (alice_db, bob_db):
                assert not Path(str(path) + "-wal").exists(), "closed private queue retained uncheckpointed WAL"
            temporary = alice_db.with_suffix(".ownership-swap")
            alice_db.rename(temporary)
            bob_db.rename(alice_db)
            temporary.rename(bob_db)
            try:
                expect_start_failure()
                cli("feishu-inspect", "--binding", bindings["alice"]["id"], "--kind", "events", ok=False)
            finally:
                alice_db.rename(temporary)
                bob_db.rename(alice_db)
                temporary.rename(bob_db)
            gateway = start_gateway()
            read_key = cli("key-add", "--user", users["alice"]["user_id"], "--read-only")
            cli("key-revoke", "--key", users["alice"]["key_id"])
            users["alice"].update(read_key)
            assert http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))[0] == 403
            webhook("alice", payload("alice", 112, "READ_ONLY_HTTP_KEY"))
            delivered("alice", 112)
            users["alice"].update(cli("key-add", "--user", users["alice"]["user_id"]))
            print("PASS 2: two real native backends, p2p-only original callbacks, escaped literal output/receipts, private owners and independent read-only HTTP authority", flush=True)

            before = send_count("alice")
            webhook("alice", payload("alice", 101, "GATE_DISABLE"))
            wait(GATES["GATE_DISABLE"][0].is_set, "model gate before user disable")
            status, _ = http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))
            assert status in (409, 429), "channel execution did not share foreground capacity"
            webhook("alice", payload("alice", 111, "QUEUED_UNDER_HOLD"))
            assert event("alice", 111)["status"] == "received" and model_count("alice", "QUEUED_UNDER_HOLD") == 0
            cli("user-disable", "--user", users["alice"]["user_id"])
            webhook("alice", payload("alice", 102, "DISABLED"), expected=403)
            GATES["GATE_DISABLE"][1].set()
            wait(lambda: event("alice", 101)["status"] == "completed", "previously admitted model completion")
            observe_quiet(lambda: send_count("alice") == before, "disabled user sent queued reply")
            cli("user-enable", "--user", users["alice"]["user_id"])
            delivered("alice", 101)
            delivered("alice", 111)
            print("PASS 3: shared admission and user-disable gating between model and send", flush=True)

            with LOCK:
                MODES["alice"] = "unknown"
            before = send_count("alice")
            webhook("alice", payload("alice", 103, "UNKNOWN"))
            wait(lambda: any(item["state"] == "unknown" for item in deliveries("alice", 103)), "unknown first platform send")
            first_hold = pending_review("alice")
            current = deliveries("alice", 103)
            assert len(current) > 1 and current[0]["state"] == "unknown"
            assert all(item["state"] == "pending" for item in current[1:])
            calls = model_count("alice")
            observe_quiet(lambda: send_count("alice") == before + 1 and model_count("alice") == calls,
                          "unknown outcome replayed model or send")
            assert http(gateway_url, "/api/sessions", "POST", {}, bearer("alice"))[0] == 409
            stop(gateway)
            assert inspect("alice", "deliveries", "--event", event("alice", 103)["id"])[0]["state"] == "unknown"
            operations = inspect("alice", "operations")
            assert any(item["request_id"] == first_hold["request_id"] and item["kind"] == "delivery" for item in operations)
            clear("alice", ok=False)
            gateway = start_gateway()
            observe_quiet(lambda: send_count("alice") == before + 1, "gateway restart replayed unknown send", 1.3)
            stop(gateway)
            cli("feishu-resolve", "--binding", bindings["alice"]["id"], "--delivery", current[0]["id"],
                "--receipt", "fixture platform accepted first message; operator checked")
            clear("alice")
            with LOCK:
                MODES["alice"] = "ok"
            gateway = start_gateway()
            final = delivered("alice", 103)
            assert model_count("alice") == calls and send_count("alice") == before + len(final)
            print("PASS 4: unknown send blocks later fragments; offline evidence resolves without replay", flush=True)

            with LOCK:
                MODES["alice"], RATE_COUNTS["alice"] = "rate", 0
            webhook("alice", payload("alice", 104, "RATE"))
            limited = wait(lambda: next((item for item in deliveries("alice", 104)
                                         if item["state"] == "retry_wait" and item["attempts"] == 1), None), "first bounded rate limit")
            with LOCK:
                first_send = next(item for item in reversed(SENDS) if item["who"] == "alice")
            assert limited["next_attempt_ms"] >= first_send["time_ms"] + 4900, "retry delay was not persisted as an absolute deadline"
            stop(gateway)
            gateway = start_gateway()
            while int(time.time() * 1000) < limited["next_attempt_ms"] - 100:
                with LOCK:
                    assert RATE_COUNTS["alice"] == 1, "restart forgot platform cooldown"
                time.sleep(.05)
            failed = wait(lambda: next((item for item in deliveries("alice", 104)
                                       if item["state"] == "permanent_failed" and item["attempts"] == 5), None), "fifth rate limit stops and holds", 30)
            with LOCK:
                attempts = [item for item in SENDS if item["who"] == "alice" and item["mode"] == "rate"]
            assert len(attempts) == 5 and {item["uuid"] for item in attempts} == {failed["id"]}, "rate retry changed its durable delivery UUID"
            assert len({item["text"] for item in attempts}) == 1, "rate retry changed its original content"
            pending_review("alice")
            observe_quiet(lambda: RATE_COUNTS["alice"] == 5, "sixth platform attempt exceeded budget")
            stop(gateway)
            clear("alice", ok=False)
            cli("feishu-cancel", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
            cli("feishu-purge", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
            assert event("alice", 104) is None
            clear("alice")
            with LOCK:
                MODES["alice"] = "ok"
            gateway = start_gateway()
            calls = model_count("alice")
            webhook("alice", payload("alice", 104, "RATE"))
            observe_quiet(lambda: model_count("alice") == calls, "purged event lost its dedup tombstone", 1.3)
            print("PASS 5: persistent cooldown, five-attempt hold, offline cancel/purge and dedup", flush=True)

            webhook("alice", payload("alice", 105, "GATE_CRASH"))
            wait(GATES["GATE_CRASH"][0].is_set, "submitted model before gateway SIGKILL")
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
            wait(lambda: model_count("alice", "GATE_CRASH") == 2, "detached backend completes tool loop")
            wait(lambda: any("GATE_CRASH verified reply" in row["messages"] for row in rows(backend["alice"]["db"], "SELECT messages FROM sessions")), "backend session committed after gateway death")
            backend_request("alice", active_hold["request_id"], 105, "completed")
            observe_quiet(lambda: model_count("alice", "GATE_CRASH") == 2 and send_count("alice") == before,
                          "recovered event replayed its model or synthesized an outbox", 1.3)
            stop(gateway)
            record = next(item for item in inspect("alice", "events") if item["id"] == event("alice", 105)["id"])
            assert record["status"] == "needs_review"
            assert any(item["request_id"] == active_hold["request_id"] and item["kind"] == "event"
                       for item in inspect("alice", "operations"))
            clear("alice", ok=False)
            cli("feishu-cancel", "--binding", bindings["alice"]["id"], "--event", record["id"])
            clear("alice")
            gateway = start_gateway()
            webhook("alice", payload("alice", 107, "AFTER_REVIEW"))
            delivered("alice", 107)
            assert model_count("alice", "GATE_CRASH") == 2
            print("PASS 6: SIGKILL gateway, live backend completion, durable review and explicit new turn", flush=True)

            with LOCK:
                MODES["alice"] = "send_crash"
            before = send_count("alice")
            webhook("alice", payload("alice", 113, "SEND_CRASH"))
            wait(SEND_GATE[0].is_set, "real platform POST accepted before SIGKILL")
            send_hold = hold("alice")
            assert send_hold and send_hold["state"] == "in_flight"
            submitting = deliveries("alice", 113)
            assert len(submitting) == 1 and submitting[0]["state"] == "submitting"
            stop(gateway, kill=True)
            SEND_GATE[1].set()
            with LOCK:
                MODES["alice"] = "ok"
            gateway = start_gateway()
            assert pending_review("alice")["request_id"] == send_hold["request_id"]
            unknown = deliveries("alice", 113)
            assert unknown[0]["state"] == "unknown"
            calls = model_count("alice", "SEND_CRASH")
            observe_quiet(lambda: send_count("alice") == before + 1 and model_count("alice", "SEND_CRASH") == calls,
                          "recovered in-flight platform POST was replayed")
            stop(gateway)
            assert any(item["request_id"] == send_hold["request_id"] and item["kind"] == "delivery"
                       for item in inspect("alice", "operations"))
            cli("feishu-resolve", "--binding", bindings["alice"]["id"], "--delivery", unknown[0]["id"],
                "--receipt", "local Feishu accepted POST; operator checked timestamp and conversation")
            clear("alice")
            gateway = start_gateway()
            webhook("alice", payload("alice", 114, "AFTER_SEND_REVIEW"))
            delivered("alice", 114)
            assert model_count("alice", "SEND_CRASH") == calls
            print("PASS 7: SIGKILL after actual Feishu POST, original-request recovery and manual evidence without POST replay", flush=True)

            with LOCK:
                MODES["alice"] = "invalid_token"
                token_requests = len(AUTH_CALLS)
            before = send_count("alice")
            webhook("alice", payload("alice", 115, "TOKEN_REJECTED"))
            failed = wait(lambda: next((item for item in deliveries("alice", 115)
                                       if item["state"] == "permanent_failed"), None), "documented token rejection is terminal")
            assert failed["attempts"] == 1 and failed["error"] == "feishu_rejected"
            pending_review("alice")
            observe_quiet(lambda: send_count("alice") == before + 1 and len(AUTH_CALLS) == token_requests,
                          "token rejection refreshed credentials or resent the original message")
            stop(gateway)
            clear("alice", ok=False)
            cli("feishu-cancel", "--binding", bindings["alice"]["id"], "--event", event("alice", 115)["id"])
            cli("feishu-purge", "--binding", bindings["alice"]["id"], "--event", event("alice", 115)["id"])
            clear("alice")
            with LOCK:
                MODES["alice"] = "ok"
            # Real startup auth uses the normal 7200-second token contract. The
            # runtime's in-memory refresh/backoff cancellation is tested in Rust;
            # a restart loses that cache, so this process test does not fake expiry.
            original_state = {who: (events(who), hold(who), model_count(who), send_count(who)) for who in backend}
            for case in ("cancel", "timeout"):
                AUTH_GATE[0].clear()
                AUTH_GATE[1].clear()
                with LOCK:
                    AUTH_GATED["alice"] = True
                    auth_before = len(AUTH_CALLS)
                with (root / ("startup-auth-" + case + ".log")).open("ab") as output:
                    pending_start = subprocess.Popen([str(BINARY), "gateway", "serve", "--config", str(cfg)],
                                                     env=ENV, stdout=output, stderr=output)
                PROCESSES.append(pending_start)
                wait(AUTH_GATE[0].is_set, "startup auth HTTP entered for " + case)
                started = time.monotonic()
                if case == "cancel":
                    stop(pending_start)
                    assert time.monotonic() - started < 3, "startup auth survived process cancellation"
                else:
                    pending_start.wait(timeout=14)
                    assert 8 <= time.monotonic() - started < 14, "startup auth did not enforce its existing HTTP budget"
                assert pending_start.returncode != 0 and len(AUTH_CALLS) == auth_before + 1
                with LOCK:
                    AUTH_GATED["alice"] = False
                AUTH_GATE[1].set()
                observe_quiet(lambda: len(AUTH_CALLS) == auth_before + 1,
                              "cancelled startup detached or retried its token HTTP", 1.3)
                for who in backend:
                    assert (events(who), hold(who), model_count(who), send_count(who)) == original_state[who], "failed startup admitted a model or message operation"
            gateway = start_gateway()
            print("PASS 8: documented token rejection stops without refresh/replay; real startup auth cancellation/HTTP deadline leave no admitted effects", flush=True)

            with LOCK:
                MODES["alice"] = "wrong_receipt"
            before = send_count("alice")
            webhook("alice", payload("alice", 116, "WRONG_RECEIPT"))
            wait(lambda: deliveries("alice", 116) and deliveries("alice", 116)[0]["state"] == "unknown", "foreign chat receipt refused")
            pending_review("alice")
            stop(gateway)
            found = inspect("alice", "deliveries", "--event", event("alice", 116)["id"])
            assert len(found) == 1 and found[0]["receipt"] is None
            clear("alice", ok=False)
            with LOCK:
                accepted = next(item for item in reversed(SENDS) if item["who"] == "alice")
                MODES["alice"] = "ok"
            cli("feishu-resolve", "--binding", bindings["alice"]["id"], "--delivery", found[0]["id"], "--receipt", accepted["receipt"])
            clear("alice")
            gateway = start_gateway()
            observe_quiet(lambda: send_count("alice") == before + 1, "wrong receipt was retried after manual resolution")
            print("PASS 9: exact-chat success receipt pin fails closed; local accepted POST is reconciled without replay", flush=True)

            before = send_count("bob")
            webhook("bob", payload("bob", 108, "GATE_REVOKE"))
            wait(GATES["GATE_REVOKE"][0].is_set, "model admission before binding revoke")
            cli("feishu-revoke", "--binding", bindings["bob"]["id"])
            GATES["GATE_REVOKE"][1].set()
            wait(lambda: event("bob", 108)["status"] == "completed", "already admitted revoked-binding event")
            webhook("bob", payload("bob", 109, "REVOKED"), expected=403)
            observe_quiet(lambda: send_count("bob") == before, "revoked binding sent queued message")
            cli("user-enable", "--user", users["bob"]["user_id"])
            webhook("bob", payload("bob", 109, "REVOKED"), expected=403)
            identity = IDENTITIES["bob"]
            cli("feishu-bind", "--user", users["bob"]["user_id"],
                "--tenant-key", identity["tenant"], "--app-id", "cli_OTHERAPP", "--bot-open-id", identity["bot"],
                "--human-open-id", identity["human"],
                "--chat-id", identity["chat"], ok=False)
            stop(gateway)
            expect_start_failure()
            settings["feishu"] = [entry for entry in settings["feishu"] if entry["binding_id"] == bindings["alice"]["id"]]
            save()
            gateway = start_gateway()
            webhook("bob", payload("bob", 109, "REVOKED"), expected=404)
            webhook("alice", payload("alice", 110, "ALICE_STILL_LIVE"))
            delivered("alice", 110)
            stop(gateway)
            cli("feishu-cancel", "--binding", bindings["bob"]["id"], "--event", event("bob", 108)["id"])
            print("PASS 10: irreversible binding revoke blocks future ingress/send and cannot be reassigned", flush=True)
            gateway = start_gateway()
            with LOCK:
                MODES["alice"] = "unknown"
            webhook("alice", payload("alice", 117, "CAPACITY_HOLD"))
            original_hold = pending_review("alice")
            before_model, before_send = model_count("alice"), send_count("alice")
            initial_queue = events("alice")
            count = len(initial_queue)

            def busy_effects_unchanged():
                return (events("alice") == initial_queue and event("alice", 4001) is None
                        and hold("alice") == original_hold and model_count("alice") == before_model
                        and send_count("alice") == before_send)

            busy = payload("alice", 4001, "STORAGE_BUSY")
            queue_lock = sqlite3.connect(channel_db("alice").as_uri() + "?mode=rw", uri=True, timeout=2)
            try:
                queue_lock.execute("BEGIN IMMEDIATE")
                # A deadline response does not prove the blocking SQL operation
                # has finished. Keep the writer lock until process exit drains
                # every task, for either SQL failure or timeout; never release it early
                # and accidentally admit this request after its 503 response.
                _, busy_reply = webhook("alice", busy, expected=503, raw=encoded(busy), return_status=True)
                assert busy_reply in ({"status": "admission_failed"}, {"status": "ingress_deadline"}), "unexpected SQL-lock response code"
                assert busy_effects_unchanged(), "SQL-lock response changed original queue, hold or effects"
                stop(gateway)
                assert gateway.poll() is not None, "gateway writers were not drained under the retained lock"
                assert busy_effects_unchanged(), "gateway exit changed original queue, hold or effects"
                print("SQL_BUSY DIAGNOSTIC " + json.dumps({"code": busy_reply["status"],
                                                         "process_exited": True, "queue_count": count,
                                                         "effects_unchanged": True}, sort_keys=True), flush=True)
            finally:
                queue_lock.rollback()
                queue_lock.close()
            gateway = start_gateway()
            assert busy_effects_unchanged(), "drained SQL-lock request was admitted or changed effects after restart"
            baseline_ids = {item["event_id"] for item in events("alice")}
            count = len(baseline_ids)  # Count after the drain barrier, never before unknown work.
            admitted_ids = set()
            busy_retries = 0

            def queue_admission(update, release_slot=None):
                nonlocal busy_retries
                data = payload("alice", update, "QUEUED_CAPACITY")
                raw = encoded(data)
                original_headers = signed("alice", raw)
                original_event = "om_" + str(update)
                expected_count = count + len(admitted_ids)
                deadline = time.monotonic() + 2.5
                for attempt in range(1, 9):
                    assert time.monotonic() < deadline, "queue admission retry budget exhausted"
                    status, reply = webhook("alice", None, expected=(200, 429), raw=raw,
                                            headers=original_headers, return_status=True)
                    observed = rows(channel_db("alice"),
                                    "SELECT count(*) AS count,sum(event_id=?1) AS matches FROM channel_events",
                                    (original_event,))[0]
                    unchanged = (model_count("alice") == before_model and send_count("alice") == before_send
                                 and hold("alice") == original_hold)
                    diagnostic = {"tenant": "alice", "update_id": update, "attempt": attempt,
                                  "http_status": status, "queue_count": observed["count"],
                                  "event_matches": observed["matches"], "expected_count": expected_count,
                                  "effects_unchanged": unchanged}
                    assert time.monotonic() < deadline, {**diagnostic, "error": "retry_budget_exceeded"}
                    if status == 200:
                        assert reply == {"status": "accepted"}, {**diagnostic, "error": "unexpected_ack_code"}
                        assert unchanged and observed == {"count": expected_count + 1, "matches": 1}, diagnostic
                        admitted_ids.add(original_event)
                        return
                    # Only the semaphore's explicit pre-admission busy result is
                    # retryable here. Full queues, SQL errors and deadlines fail.
                    assert reply == {"status": "busy"}, {**diagnostic, "error": "non_transient_429"}
                    assert unchanged and observed == {"count": expected_count, "matches": 0}, diagnostic
                    busy_retries += 1
                    print("INGRESS DIAGNOSTIC " + json.dumps({**diagnostic, "code": "busy",
                                                             "busy_retries": busy_retries}, sort_keys=True), flush=True)
                    assert busy_retries <= 32 and attempt < 8 and time.monotonic() < deadline, diagnostic
                    if release_slot is not None:
                        release_slot()
                        release_slot = None
                    time.sleep(min(.01 * 2 ** (attempt - 1), .1))
                raise AssertionError("bounded queue admission retries exhausted")

            # Occupy the actual per-binding body-reader slot, then release it
            # after the first busy response. This exercises the retry branch
            # without changing production state, quotas or the original signature.
            challenge = {"type": "url_verification", "token": VERIFICATION_TOKENS["alice"],
                         "challenge": "bounded-queue-slot-barrier"}
            challenge_raw = encoded(challenge)
            connection = HTTPConnection(settings["bind"], timeout=4)
            connection.putrequest("POST", "/hooks/feishu/" + bindings["alice"]["id"])
            connection.putheader("Content-Type", "application/json")
            connection.putheader("Content-Length", str(len(challenge_raw)))
            slot_start = time.monotonic()
            connection.endheaders(challenge_raw[:1])
            released = False

            def release_slot():
                nonlocal released
                connection.send(challenge_raw[1:])
                response = connection.getresponse()
                body = response.read(64 * 1024 + 1)
                assert response.status == 200 and len(body) <= 64 * 1024
                assert json.loads(body) == {"challenge": challenge["challenge"]}
                assert time.monotonic() - slot_start < 1
                released = True
                connection.close()

            try:
                readiness_deadline = time.monotonic() + .25
                while True:
                    status, reply = webhook("alice", None, expected=(200, 429), raw=challenge_raw,
                                            headers={"Content-Type": "application/json"}, return_status=True)
                    if status == 429:
                        assert reply == {"status": "busy"}
                        break
                    assert reply == {"challenge": challenge["challenge"]}
                    assert time.monotonic() < readiness_deadline, "body-reader slot was never occupied"
                    time.sleep(.002)
                queue_admission(2000, release_slot)
                assert released and busy_retries >= 1, "actual busy admission branch was not exercised"
            finally:
                connection.close()
            for update in range(2001, 2000 + 1000 - count):
                queue_admission(update)
            current = events("alice")
            assert len(current) == 1000 and {item["event_id"] for item in current} == baseline_ids | admitted_ids
            assert len(admitted_ids) == 1000 - count
            overflow = payload("alice", 4000, "CAPACITY_OVERFLOW")
            webhook("alice", overflow, expected=429, expected_code="queue_full", raw=encoded(overflow))
            assert model_count("alice") == before_model and send_count("alice") == before_send
            assert len(events("alice")) == 1000 and event("alice", 4000) is None
            cli("feishu-revoke", "--binding", bindings["alice"]["id"])
            webhook("alice", overflow, expected=403)
            stop(gateway)
            settings["feishu"] = []
            save()
            gateway = start_gateway()
            webhook("alice", overflow, expected=404)
            stop(gateway)
            retained = inspect("alice", "events", "--limit", "100")
            assert len(retained) == 100 and len(inspect("alice", "events", "--limit", "100", "--offset", "100")) == 100
            clear("alice", ok=False)
            unresolved = event("alice", 117)
            cli("feishu-cancel", "--binding", bindings["alice"]["id"], "--event", unresolved["id"])
            cli("feishu-purge", "--binding", bindings["alice"]["id"], "--event", unresolved["id"])
            clear("alice")
            assert any(item["status"] == "received" for item in events("alice")), "future unclaimed queue was discarded"
            inspect_text = json.dumps(inspect("alice", "operations"))
            assert all(secret_value not in inspect_text for secret_value in [*APP_SECRETS.values(),*ENCRYPT_KEYS.values(),*VERIFICATION_TOKENS.values(),*BOT_TOKENS.values()])
            print("PASS 11: actual thousand-event queue limit, busy SQL 503, revoked/runtime-off retained state, bounded offline pages and review guard preserve unclaimed queue", flush=True)
            for process in PROCESSES:
                stop(process)
            for secret_value in [*BOT_TOKENS.values(), *APP_SECRETS.values(), *VERIFICATION_TOKENS.values(), *MODEL_SECRETS.values(),
                                 *(entry["token"] for entry in backend.values()),
                                 *ENCRYPT_KEYS.values(), *ISSUED_TOKENS]:
                for logfile in root.glob("*.log"):
                    assert secret_value not in logfile.read_text(), "secret exposed in process log"
            for database in root.rglob("*.sqlite3*"):
                if database.is_file():
                    raw_db = database.read_bytes()
                    assert all(value.encode() not in raw_db for value in [*APP_SECRETS.values(),*ENCRYPT_KEYS.values(),*VERIFICATION_TOKENS.values(),*BOT_TOKENS.values()]), "platform credential persisted in SQLite state"
            check_errors()
    finally:
        for _, release in GATES.values():
            release.set()
        SEND_GATE[1].set()
        AUTH_GATE[1].set()
        for process in PROCESSES:
            stop(process, kill=True)
        fixture.shutdown()
        fixture.server_close()


if __name__ == "__main__":
    main()
