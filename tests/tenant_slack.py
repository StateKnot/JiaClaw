#!/usr/bin/env python3
"""Real two-tenant Slack gateway acceptance; localhost and synthetic secrets only.

Runs the production binary, native model/tool loop, private SQLite queues and CLI.
It does not qualify a real Slack installation or paid model provider. SQLite observations
are read only; one temporary write-lock transaction rolls back without changing data.
Closed real queue files are exchanged to verify owner rejection. No normal data, clock
or provider fault hook is fabricated.
"""
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import hmac
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
ENV.update(JIACLAW_LOG_LEVEL="off,jiaclaw::gateway::slack=error", JIACLAW_LOG_FORMAT="json",
           HTTP_PROXY="http://127.0.0.1:1",
           HTTPS_PROXY="http://127.0.0.1:1", ALL_PROXY="http://127.0.0.1:1", NO_PROXY="")
CLIENT = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOCK = threading.Lock()
ERRORS, MODEL_REQUESTS, SENDS = [], [], []
MODEL_KEYS = set()
MODEL_SECRETS = {who: "local-model-" + uuid.uuid4().hex for who in ("alice", "bob")}
IDENTITIES = {
    who: {"team": "TLOCAL" + who.upper(), "app": "ALOCAL" + who.upper(),
          "bot_user": ("W" if who == "alice" else "U") + "BOT" + who.upper(), "bot": "BBOT" + who.upper(),
          "sender": ("U" if who == "alice" else "W") + "HUMAN" + who.upper(), "conversation": "DPRIVATE" + who.upper()}
    for who in ("alice", "bob")}
BOT_TOKENS = {who: "xoxb-local-" + uuid.uuid4().hex for who in IDENTITIES}
SIGNING_SECRETS = {who: "signing_" + uuid.uuid4().hex for who in IDENTITIES}
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
    """Extract only the fixed Slack settlement event from a bounded log tail."""
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
        if (record.get("target") != "jiaclaw::gateway::slack" or record.get("level") != "ERROR"
                or not isinstance(fields, dict)
                or fields.get("message") != "Slack write hold requires administrator review"
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
            elif path == "/api/chat.postMessage":
                assert method == "POST"
                self.platform(body)
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
        query = parse_qs(url.query, strict_parsing=True) if url.query else {}
        with LOCK:
            HANDSHAKES.append({"who": who, "method": method, "path": url.path})
            fault = STARTUP_FAULT["case"] if STARTUP_FAULT["who"] == who else None
        if url.path == "/api/auth.test":
            assert method == "POST" and not query
            result = {"ok": True, "team_id": identity["team"], "user_id": identity["bot_user"],
                      "bot_id": identity["bot"], "is_enterprise_install": False}
            if who == "bob":
                result.pop("is_enterprise_install")  # Slack permits omitted false.
            if fault == "enterprise":
                result["is_enterprise_install"] = True
            elif fault == "enterprise_id":
                result["enterprise_id"] = "EFOREIGN"
        elif url.path == "/api/bots.info":
            assert method == "GET" and query == {"bot": [identity["bot"]]}
            result = {"ok": True, "bot": {"id": identity["bot"], "user_id": identity["bot_user"],
                                           "app_id": identity["app"], "deleted": False}}
            if fault == "bot_app":
                result["bot"]["app_id"] = "AFOREIGN"
        elif url.path == "/api/users.info":
            assert method == "GET" and query == {"user": [identity["sender"]]}
            result = {"ok": True, "user": {"id": identity["sender"], "team_id": identity["team"],
                                            "is_bot": False, "is_app_user": False, "deleted": False}}
            if fault == "bot_sender":
                result["user"]["is_bot"] = True
        elif url.path == "/api/conversations.info":
            assert method == "GET" and query == {"channel": [identity["conversation"]]}
            result = {"ok": True, "channel": {"id": identity["conversation"], "is_im": True,
                                               "user": identity["sender"], "is_user_deleted": False}}
            if fault == "shared_dm":
                result["channel"]["is_ext_shared"] = True
        else:
            raise AssertionError("unexpected platform API")
        self.reply(200, result)

    def model(self, body):
        who = next(who for who, secret in MODEL_SECRETS.items()
                   if self.headers.get("Authorization") == "Bearer " + secret)
        assert body["model"] == "fixture-channel", "channel did not select configured route"
        assert sorted(tool["function"]["name"] for tool in body["tools"]) == ["datetime_now", "json_query"]
        assert body["parallel_tool_calls"] is False
        prompt = next(message["content"] for message in reversed(body["messages"])
                      if message["role"] == "user")
        assert "IGNORED_RICH_TEXT" not in prompt, "ignored Slack blocks entered model prompt"
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
        assert body["channel"] == identity["conversation"], "cross-tenant platform destination"
        assert body["text"].strip() and "thread_ts" not in body
        for key, expected in {"mrkdwn": False, "parse": "none", "link_names": False,
                              "unfurl_links": False, "unfurl_media": False}.items():
            assert body[key] == expected, "Slack plain-text safety flag absent"
        assert "<" not in body["text"] and ">" not in body["text"], "Slack mention controls were not escaped"
        assert len(body["text"].encode("utf-16-le")) // 2 <= 2000
        with LOCK:
            mode = MODES[who]
            receipt = "1791400000." + format(len(SENDS) + 1, "06d")
            SENDS.append({"who": who, "time_ms": int(time.time() * 1000), "mode": mode,
                          "text": body["text"], "receipt": receipt})
            if mode == "rate":
                RATE_COUNTS[who] += 1
                attempt = RATE_COUNTS[who]
        if mode == "unknown":
            self.reply(503, {"ok": False})  # Locally accepted effect, ambiguous receipt.
        elif mode == "rate":
            self.reply(429, {"ok": False, "error": "ratelimited"},
                       {"Retry-After": "5" if attempt == 1 else "1"})
        else:
            if mode == "send_crash":
                SEND_GATE[0].set()
                assert SEND_GATE[1].wait(60), "fixture send gate expired"
            self.reply(200, {"ok": True, "channel": identity["conversation"], "ts": receipt})


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
        with tempfile.TemporaryDirectory(prefix="jiaclaw-gateway-slack-") as temp:
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
                private_values = [*BOT_TOKENS.values(), *SIGNING_SECRETS.values(), *MODEL_SECRETS.values(),
                                  *ISSUED_TOKENS, *(entry["token"] for entry in backend.values())]
                assert all(value not in result.stdout + result.stderr for value in private_values), "credential in startup error"

            def channel_db(who):
                return root / "registry/slack" / (bindings[who]["id"] + ".sqlite3")

            def events(who):
                return rows(channel_db(who), "SELECT * FROM channel_events ORDER BY seq")

            def event(who, update):
                return next((item for item in events(who) if item["event_id"] == "Ev" + str(update)), None)

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
                                    "slack_error", "slack_rejected", "invalid_rate_limit"}
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
                        known_actions = {"slack_execute", "slack_send", "write_completed",
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
                return {"type": "event_callback", "team_id": identity["team"], "api_app_id": identity["app"],
                        "event_id": "Ev" + str(update), "is_ext_shared_channel": False,
                        "authorizations": [{"team_id": identity["team"], "user_id": identity["bot_user"],
                                            "is_bot": True, **({} if who == "bob" else {"is_enterprise_install": False})}],
                        "event": {"type": "message", "user": identity["sender"],
                                  "channel": identity["conversation"], "channel_type": "im",
                                  "text": "CASE:" + marker, "ts": "1791400000." + format(update, "06d"),
                                  "blocks": [{"type": "rich_text", "elements": [{"type": "rich_text_section",
                                              "elements": [{"type": "text", "text": "IGNORED_RICH_TEXT"}]}]}]}}

            def encoded(value):
                return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()

            def signed(who, raw, timestamp=None):
                timestamp = str(int(time.time())) if timestamp is None else str(timestamp)
                digest = hmac.new(SIGNING_SECRETS[who].encode(), b"v0:" + timestamp.encode() + b":" + raw,
                                  hashlib.sha256).hexdigest()
                return {"X-Slack-Request-Timestamp": timestamp, "X-Slack-Signature": "v0=" + digest,
                        "Content-Type": "application/json"}

            def webhook(who, value, expected=200, headers=None, query="", raw=None):
                raw = encoded(value) if raw is None else raw
                supplied = signed(who, raw) if headers is None else headers
                request = urllib.request.Request(gateway_url + "/hooks/slack/" + bindings[who]["id"] + query,
                                                 method="POST", data=raw, headers=supplied)
                start = time.monotonic()
                try:
                    response = CLIENT.open(request, timeout=4)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    body = response.read(64 * 1024 + 1)
                    assert len(body) <= 64 * 1024, "unbounded webhook response"
                    assert response.status == expected, (who, response.status, expected)
                    assert time.monotonic() - start < 3, "Slack acknowledgement exceeded its 3-second budget"
                    return json.loads(body) if body else None

            def inspect(who, kind, *arguments):
                return cli("slack-inspect", "--binding", bindings[who]["id"], "--kind", kind, *arguments)["result"][kind]

            def clear(who, ok=True):
                return cli("review-clear", "--user", users[who]["user_id"], "--confirm-backend-idle",
                           "--note", "Local fixture external result and backend idle verified; no replay", ok=ok)

            def bearer(who):
                return {"Authorization": "Bearer " + users[who]["token"]}

            def backend_request(who, request_id, update, expected):
                status, value = http(backend[who]["url"], "/internal/channels/slack/requests/" + request_id,
                                     headers={"Authorization": "Bearer " + backend[who]["token"]})
                assert status == 200, "original backend request metadata unavailable"
                assert value == {"protocol": 2, "backend_id": who, "binding_id": bindings[who]["id"],
                                 "request_id": request_id, "event_id": "Ev" + str(update),
                                 "session_id": "slack:" + bindings[who]["id"], "status": expected}, "unexpected request metadata or private fields"
                return value

            for who in ("alice", "bob"):
                directory = root / who
                workspace = directory / "workspace"
                workspace.mkdir(parents=True)
                backend_token = "private-backend-" + uuid.uuid4().hex
                url = "http://127.0.0.1:" + str(port())
                config = {"agent": {"name": who, "description": "Tenant Slack fixture", "workspace_path": str(workspace),
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
                                "db": directory / "state/sessions.sqlite3", "signing": SIGNING_SECRETS[who]}
                settings["backends"].append({"id": who, "url": url,
                                             "token_file": secret(who + "-backend.secret", backend_token)})
                backend[who]["process"] = launch(["serve", "--config", str(path)], url, who)
            save()
            for who in backend:
                users[who] = cli("user-add", "--backend", who)
                identity = IDENTITIES[who]
                if who == "bob":
                    # A dedicated app remains reserved across workspaces, not
                    # just for one (team, app) installation pair.
                    cli("slack-bind", "--user", users[who]["user_id"],
                        "--team-id", identity["team"], "--app-id", IDENTITIES["alice"]["app"],
                        "--bot-user-id", identity["bot_user"], "--bot-id", identity["bot"],
                        "--sender-id", identity["sender"], "--conversation-id", identity["conversation"], ok=False)
                bindings[who] = cli("slack-bind", "--user", users[who]["user_id"],
                                    "--team-id", identity["team"], "--app-id", identity["app"],
                                    "--bot-user-id", identity["bot_user"], "--bot-id", identity["bot"],
                                    "--sender-id", identity["sender"], "--conversation-id", identity["conversation"])
            gateway = start_gateway()
            webhook("alice", payload("alice", 1, "DEFAULT_OFF"), expected=404)
            for path in ("/internal/channels/slack/status", "/internal/channels/slack-binding", "/internal/channels/slack/execute",
                         "/internal/gateway/channel/status", "/internal/gateway/channel/chat",
                         "/api/channels/events", "/hooks/inbound"):
                assert http(gateway_url, path, headers=bearer("alice"))[0] == 404, path
            assert http(backend["bob"]["url"], "/internal/channels/slack/status",
                        headers={"Authorization": "Bearer " + backend["bob"]["token"]})[0] == 404
            stop(gateway)
            settings["slack"] = [{"binding_id": bindings[who]["id"],
                                     "bot_token_file": secret(who + "-bot.secret", BOT_TOKENS[who]),
                                     "signing_secret_file": secret(who + "-signing.secret", SIGNING_SECRETS[who]),
                                     "api_base": base + "/api", "allow_loopback": True} for who in backend]
            save()
            expect_start_failure()  # Bob's real backend has not opted into the contract.
            stop(backend["bob"]["process"])
            backend["bob"]["config"]["http"]["gateway_channel_chat"] = True
            backend["bob"]["path"].write_text(json.dumps(backend["bob"]["config"]))
            backend["bob"]["process"] = launch(["serve", "--config", str(backend["bob"]["path"])], backend["bob"]["url"], "bob")
            gateway = start_gateway()
            for fault in ("enterprise", "enterprise_id", "bot_app", "bot_sender", "shared_dm"):
                stop(gateway)
                with LOCK:
                    STARTUP_FAULT.update(who="alice", case=fault)
                expect_start_failure()
                with LOCK:
                    STARTUP_FAULT.update(who=None, case=None)
                gateway = start_gateway()
            for who in backend:
                challenge = "bounded-local-challenge-" + who
                assert webhook(who, {"type": "url_verification", "challenge": challenge}) == {"challenge": challenge}
                for invalid_challenge in ("", "x" * 1025):
                    webhook(who, {"type": "url_verification", "challenge": invalid_challenge}, expected=400)
                webhook(who, payload(who, 2, "REJECTED"), expected=401, headers={})
                raw = encoded(payload(who, 2, "REJECTED"))
                webhook(who, None, expected=415, raw=raw, headers=signed(who, raw) | {"Content-Type": "text/plain"})
                wrong = signed(who, raw)
                wrong["X-Slack-Signature"] = "v0=" + "0" * 64
                webhook(who, payload(who, 2, "REJECTED"), expected=401, headers=wrong)
                # A malformed MAC must not spend registry write capacity. Hold
                # a temporary transaction without changing any production data;
                # invalid authentication must still return 401, never storage 503.
                def unchanged_state():
                    return (rows(Path(settings["registry_path"]), "SELECT seq,action FROM audit_events ORDER BY seq"),
                            hold(who), events(who), model_count(who), send_count(who))
                before_invalid_mac = unchanged_state()
                registry_lock = sqlite3.connect(Path(settings["registry_path"]).as_uri() + "?mode=rw",
                                               uri=True, timeout=2)
                try:
                    registry_lock.execute("BEGIN IMMEDIATE")
                    webhook(who, payload(who, 2, "REJECTED"), expected=401, headers=wrong)
                finally:
                    registry_lock.rollback()
                    registry_lock.close()
                assert unchanged_state() == before_invalid_mac, "invalid MAC changed audit/hold/queue/model/send"
                for timestamp in (int(time.time()) - 360, int(time.time()) + 360):
                    webhook(who, payload(who, 2, "REJECTED"), expected=401,
                            headers=signed(who, raw, timestamp))
                tampered = raw.replace(b"REJECTED", b"TAMPERED")
                webhook(who, None, expected=401, headers=signed(who, raw), raw=tampered)
                webhook(who, payload(who, 2, "REJECTED"), expected=400, query="?backend=other")
                other = IDENTITIES["bob" if who == "alice" else "alice"]
                cases = [(('team_id',), other["team"]), (('api_app_id',), other["app"]),
                         (('event', 'user'), other["sender"]), (('event', 'channel'), other["conversation"]),
                         (('event', 'channel'), "CPUBLIC"), (('event', 'channel_type'), "channel"),
                         (('event', 'thread_ts'), "1791400000.000001"), (('event', 'subtype'), "bot_message"),
                         (('event', 'bot_id'), "BFOREIGN"), (('event', 'files'), [{"url_private": "https://example.invalid/media"}]),
                         (('event', 'attachments'), [{}]), (('event', 'ts'), "1791400000.1"),
                         (('is_ext_shared_channel',), True), (('enterprise_id',), "EFOREIGN"),
                         (('event_id',), "evbad"), (('event_id',), "Ev" + "x" * 127)]
                for path, value in cases:
                    bad = payload(who, 2, "REJECTED")
                    target = bad
                    for key in path[:-1]:
                        target = target[key]
                    target[path[-1]] = value
                    webhook(who, bad, expected=400)
                for changes in ({"team_id": other["team"]}, {"user_id": other["bot_user"]},
                                {"is_bot": False}, {"is_enterprise_install": True}, {"enterprise_id": "EFOREIGN"}):
                    bad = payload(who, 2, "REJECTED")
                    bad["authorizations"][0].update(changes)
                    webhook(who, bad, expected=400)
                bad = payload(who, 2, "REJECTED")
                bad["authorizations"] *= 2
                webhook(who, bad, expected=400)
                duplicate_json = raw[:-1] + b',"event_id":"Ev2"}'
                webhook(who, None, expected=400, raw=duplicate_json)
                oversized = encoded(payload(who, 2, "REJECTED") | {"padding": "x" * 70000})
                webhook(who, None, expected=413, raw=oversized)
                for name in ("X-Slack-Signature", "X-Slack-Request-Timestamp"):
                    connection = HTTPConnection(settings["bind"], timeout=4)
                    connection.putrequest("POST", "/hooks/slack/" + bindings[who]["id"])
                    for key, value in signed(who, raw).items():
                        connection.putheader(key, value)
                        if key == name:
                            connection.putheader(key, value)
                    connection.putheader("Content-Length", str(len(raw)))
                    connection.endheaders(raw)
                    response = connection.getresponse()
                    assert response.status == 401, "duplicate Slack auth header accepted"
                    response.read()
                    connection.close()
                connection = HTTPConnection(settings["bind"], timeout=4)
                connection.putrequest("POST", "/hooks/slack/" + bindings[who]["id"])
                for key, value in signed(who, raw).items():
                    connection.putheader(key, value)
                connection.putheader("Content-Length", str(len(raw)))
                start = time.monotonic()
                connection.endheaders(raw[:1])
                response = connection.getresponse()
                assert response.status == 408 and time.monotonic() - start < 3, "Slack body deadline was not enforced"
                response.read()
                connection.close()
                assert not events(who) and model_count(who) == 0 and send_count(who) == 0
                methods = {(item["method"], item["path"]) for item in HANDSHAKES if item["who"] == who}
                assert {("POST", "/api/auth.test"), ("GET", "/api/bots.info"),
                        ("GET", "/api/users.info"), ("GET", "/api/conversations.info")} <= methods
            print("PASS 1: default-off + protocol opt-in, four private manifest checks, W IDs, signature/body deadlines and strict DM identity", flush=True)

            for who in backend:
                data = payload(who, 100, "BASIC_" + who.upper())
                webhook(who, data)
                webhook(who, data, headers=signed(who, encoded(data)) | {"X-Slack-Retry-Num": "1",
                        "X-Slack-Retry-Reason": "http_timeout"})  # Retry metadata does not widen identity.
            for who in backend:
                done = delivered(who, 100)
                assert len(done) == 1 and done[0]["text"].startswith(who + " CASE:BASIC_")
                assert model_count(who) == 2 and len(events(who)) == 1
                with LOCK:
                    sent = next(item for item in SENDS if item["who"] == who)
                assert done[0]["receipt"] == sent["receipt"], "Slack timestamp receipt was not preserved"
                assert "&lt;@UFAKE&gt; &amp; &lt;https://example.invalid/x&gt;" in sent["text"], "literal mention/link escaping missing"
                webhook(who, payload(who, 100, "FINGERPRINT_CHANGED"), expected=409)
                assert model_count(who) == 2, "changed fingerprint replayed an existing event"
                owner = rows(channel_db(who), "SELECT * FROM gateway_slack_owner")[0]
                assert owner["protocol"] == 2 and owner["backend_id"] == who
                assert owner["user_id"] == users[who]["user_id"] and owner["binding_id"] == bindings[who]["id"]
                assert not rows(channel_db(who), "SELECT * FROM sessions"), "channel DB copied private backend history"
                saved = rows(backend[who]["db"], "SELECT id,messages FROM sessions")
                assert len(saved) == 1 and saved[0]["id"] == "slack:" + bindings[who]["id"]
                assert "BASIC_" + who.upper() in saved[0]["messages"]
                assert "BASIC_" + ("BOB" if who == "alice" else "ALICE") not in saved[0]["messages"]
                cli("slack-inspect", "--binding", bindings[who]["id"], "--kind", "events", ok=False)
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
                cli("slack-inspect", "--binding", bindings["alice"]["id"], "--kind", "events", ok=False)
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
            print("PASS 2: two real native backends, text-only rich blocks, escaped literal output/receipts, private owners and independent read-only HTTP authority", flush=True)

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
            cli("slack-resolve", "--binding", bindings["alice"]["id"], "--delivery", current[0]["id"],
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
            pending_review("alice")
            observe_quiet(lambda: RATE_COUNTS["alice"] == 5, "sixth platform attempt exceeded budget")
            stop(gateway)
            clear("alice", ok=False)
            cli("slack-cancel", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
            cli("slack-purge", "--binding", bindings["alice"]["id"], "--event", failed["event_id"])
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
            cli("slack-cancel", "--binding", bindings["alice"]["id"], "--event", record["id"])
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
            cli("slack-resolve", "--binding", bindings["alice"]["id"], "--delivery", unknown[0]["id"],
                "--receipt", "local Slack accepted POST; operator checked timestamp and conversation")
            clear("alice")
            gateway = start_gateway()
            webhook("alice", payload("alice", 114, "AFTER_SEND_REVIEW"))
            delivered("alice", 114)
            assert model_count("alice", "SEND_CRASH") == calls
            print("PASS 7: SIGKILL after actual Slack POST, original-request recovery and manual evidence without POST replay", flush=True)

            before = send_count("bob")
            webhook("bob", payload("bob", 108, "GATE_REVOKE"))
            wait(GATES["GATE_REVOKE"][0].is_set, "model admission before binding revoke")
            cli("slack-revoke", "--binding", bindings["bob"]["id"])
            GATES["GATE_REVOKE"][1].set()
            wait(lambda: event("bob", 108)["status"] == "completed", "already admitted revoked-binding event")
            webhook("bob", payload("bob", 109, "REVOKED"), expected=403)
            observe_quiet(lambda: send_count("bob") == before, "revoked binding sent queued message")
            cli("user-enable", "--user", users["bob"]["user_id"])
            webhook("bob", payload("bob", 109, "REVOKED"), expected=403)
            identity = IDENTITIES["bob"]
            cli("slack-bind", "--user", users["bob"]["user_id"],
                "--team-id", identity["team"], "--app-id", "AOTHERAPP", "--bot-user-id", identity["bot_user"],
                "--bot-id", identity["bot"], "--sender-id", identity["sender"],
                "--conversation-id", identity["conversation"], ok=False)
            stop(gateway)
            expect_start_failure()
            settings["slack"] = [entry for entry in settings["slack"] if entry["binding_id"] == bindings["alice"]["id"]]
            save()
            gateway = start_gateway()
            webhook("bob", payload("bob", 109, "REVOKED"), expected=404)
            webhook("alice", payload("alice", 110, "ALICE_STILL_LIVE"))
            delivered("alice", 110)
            stop(gateway)
            cli("slack-cancel", "--binding", bindings["bob"]["id"], "--event", event("bob", 108)["id"])
            print("PASS 8: irreversible binding revoke blocks future ingress/send and cannot be reassigned", flush=True)
            for process in PROCESSES:
                stop(process)
            for secret_value in [*BOT_TOKENS.values(), *MODEL_SECRETS.values(),
                                 *(entry["token"] for entry in backend.values()),
                                 *SIGNING_SECRETS.values(), *ISSUED_TOKENS]:
                for logfile in root.glob("*.log"):
                    assert secret_value not in logfile.read_text(), "secret exposed in process log"
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
