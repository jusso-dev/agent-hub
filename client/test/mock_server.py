#!/usr/bin/env python3
"""Minimal in-memory mock of the agent-hub v0.1 HTTP API (see SPEC.md).

It exists so client/hub.sh and client/mcp can be tested without building the
real server. Response shapes follow the "Server notes" section of SPEC.md.
The real server is the source of truth; client/demo.sh runs against it by
default and against this mock with --mock.

Usage: HUB_ADMIN_KEY=admin python3 mock_server.py [--bind 127.0.0.1:8787]
"""
import json
import os
import re
import secrets
import sys
import threading
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

ADMIN_KEY = os.environ.get("HUB_ADMIN_KEY")
LOCK = threading.Lock()
MAX_BODY = 256 * 1024

agents = {}       # id -> dict
keys = {}         # agent_key -> agent id
invites = {}      # invite_key -> dict
messages = []     # list of dict
inbox = {}        # (message_id, agent_id) -> read_at
artifacts = {}    # id -> dict
adoptions = []    # dicts
interests = {}    # agent id -> set(tags)

# SPEC.md safety regexes, with two deliberate refinements the client relies on:
# `sk-` is anchored to a token shape, and ${ENV} placeholders are allowed.
SECRET_RE = re.compile(
    r"(\bsk-[A-Za-z0-9_-]{16,}|ghp_|gho_|\bxox[abposr]-|AKIA[0-9A-Z]{16}|-----BEGIN"
    r"|\"(password|token)\"\s*:\s*\"(?!\$\{[A-Za-z0-9_]+\}\")[^\"]+\")"
)


def now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def tags_of(v):
    # Server notes: tags may be an array or a comma string; lowercased.
    if v is None:
        return []
    if isinstance(v, str):
        v = v.split(",")
    return sorted({t.strip().lower() for t in v if t and t.strip()})


def public_agent(a):
    return {k: a[k] for k in ("id", "handle", "display_name", "owner", "runtime",
                              "description", "created_at", "last_seen_at")}


def handle_of(agent_id):
    a = agents.get(agent_id)
    return a["handle"] if a else None


def meta(art):
    out = {k: v for k, v in art.items() if k not in ("content", "versions")}
    out["owner"] = handle_of(art["owner_agent"])
    out["adopted_by"] = [handle_of(x["agent_id"]) for x in adoptions if x["artifact_id"] == art["id"]]
    return out


def deliver(from_id, body, kind, to_id=None, channel=None, thread_id=None, artifact_id=None):
    m = {
        "id": str(uuid.uuid4()), "from_agent": from_id, "from": handle_of(from_id),
        "to_agent": to_id, "to": handle_of(to_id) if to_id else None, "channel": channel,
        "thread_id": thread_id, "body": body, "kind": kind, "artifact_id": artifact_id,
        "created_at": now(),
    }
    messages.append(m)
    if to_id:
        inbox[(m["id"], to_id)] = None
    else:
        # Server notes: a channel post lands in the inbox of each @handle it mentions.
        mentioned = set(re.findall(r"@([a-z0-9][a-z0-9_-]{1,31})", body))
        for aid, a in agents.items():
            if aid != from_id and a["handle"] in mentioned:
                inbox[(m["id"], aid)] = None
    return m


def related_agents(art):
    tags = set(art["tags"])
    out = []
    for aid in agents:
        if aid == art["owner_agent"]:
            continue
        mine = set(interests.get(aid, set()))
        for other in artifacts.values():
            if other["owner_agent"] == aid:
                mine |= set(other["tags"])
        if tags & mine:
            out.append(aid)
    return out


def upsert(owner_id, kind, name, body, default_visibility):
    content = body.get("content", "")
    if not isinstance(content, str):
        content = json.dumps(content, indent=2)
    if kind == "mcp" and SECRET_RE.search(content):
        raise ApiError(422, "MCP config looks like it contains a secret. Replace secret values with ${ENV_VAR} placeholders.")
    tags = tags_of(body.get("tags"))
    summary = body.get("summary", "") or ""
    visibility = body.get("visibility") or default_visibility
    existing = next((a for a in artifacts.values()
                     if a["owner_agent"] == owner_id and a["kind"] == kind and a["name"] == name), None)
    if existing and existing["content"] == content and existing["summary"] == summary \
            and existing["tags"] == tags and existing["visibility"] == visibility:
        return existing, False
    ts = now()
    if existing:
        art = existing
        art.update(content=content, summary=summary, tags=tags, visibility=visibility,
                   version=art["version"] + 1, updated_at=ts)
    else:
        art = {"id": str(uuid.uuid4()), "owner_agent": owner_id, "kind": kind, "name": name,
               "summary": summary, "tags": tags, "content": content, "version": 1,
               "visibility": visibility, "created_at": ts, "updated_at": ts, "versions": []}
        artifacts[art["id"]] = art
    art["versions"].append({"version": art["version"], "summary": summary, "content": content, "created_at": ts})
    if visibility == "shared":
        owner = handle_of(owner_id)
        text = (f"@{owner} shared {kind} `{name}` v{art['version']}: {summary}. "
                f"GET /api/artifacts/{art['id']} to learn it, POST adopt if you use it.")
        for aid in related_agents(art):
            deliver(owner_id, text, "share", to_id=aid, artifact_id=art["id"])
        deliver(owner_id, text, "share", channel="shared", artifact_id=art["id"])
    return art, True


def suggestions_for(agent_id):
    mine = set(interests.get(agent_id, set()))
    for a in artifacts.values():
        if a["owner_agent"] == agent_id:
            mine |= set(a["tags"])
    adopted = {x["artifact_id"] for x in adoptions if x["agent_id"] == agent_id}
    out = [a for a in artifacts.values()
           if a["visibility"] == "shared" and a["owner_agent"] != agent_id
           and a["id"] not in adopted and set(a["tags"]) & mine]
    out.sort(key=lambda a: a["updated_at"], reverse=True)
    return [meta(a) for a in out[:10]]


class ApiError(Exception):
    def __init__(self, status, msg):
        super().__init__(msg)
        self.status = status
        self.msg = msg


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        if os.environ.get("MOCK_VERBOSE"):
            sys.stderr.write("mock: " + fmt % args + "\n")

    def send_json(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n > MAX_BODY:
            raise ApiError(413, "body too large")
        if n == 0:
            return {}
        try:
            return json.loads(self.rfile.read(n))
        except json.JSONDecodeError:
            raise ApiError(400, "invalid JSON")

    def auth(self):
        h = self.headers.get("Authorization", "")
        tok = h[7:] if h.startswith("Bearer ") else None
        if tok and ADMIN_KEY and tok == ADMIN_KEY:
            return "admin"
        if tok in keys:
            a = agents[keys[tok]]
            if a.get("revoked_at"):
                raise ApiError(401, "agent key revoked")
            a["last_seen_at"] = now()
            return a
        raise ApiError(401, "missing or invalid key")

    def agent(self):
        who = self.auth()
        if who == "admin":
            raise ApiError(403, "agent key required")
        return who

    def admin(self):
        if self.auth() != "admin":
            raise ApiError(403, "admin key required")

    def dispatch(self, method):
        url = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(url.query).items()}
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if parts[:1] != ["api"]:
            raise ApiError(404, "not found")
        p = parts[1:]
        with LOCK:
            return self.route(method, p, q)

    def route(self, m, p, q):
        if m == "GET" and p == ["health"]:
            return 200, {"ok": True}
        if m == "POST" and p == ["invites"]:
            self.admin()
            b = self.body()
            key = "inv_" + secrets.token_urlsafe(24)
            invites[key] = {"id": str(uuid.uuid4()), "note": b.get("note"), "owner": b.get("owner"),
                            "created_at": now(), "redeemed_at": None, "redeemed_by": None}
            return 201, {"invite_key": key, "invite": invites[key]}
        if m == "GET" and p == ["invites"]:
            self.admin()
            return 200, {"invites": list(invites.values())}
        if m == "DELETE" and len(p) == 2 and p[0] == "agents":
            self.admin()
            a = agents.get(p[1]) or next((x for x in agents.values() if x["handle"] == p[1]), None)
            if not a:
                raise ApiError(404, "no such agent")
            a["revoked_at"] = now()
            return 200, {"revoked": True, "id": a["id"]}
        if m == "POST" and p == ["onboard"]:
            b = self.body()
            inv = invites.get(b.get("invite_key", ""))
            if not inv:
                raise ApiError(401, "invalid invite key")
            if inv["redeemed_at"]:
                raise ApiError(409, "invite already used")
            handle = (b.get("handle") or "").lstrip("@")
            if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{1,31}", handle):
                raise ApiError(400, "handle must be 2-32 chars of a-z, 0-9, - or _")
            if any(a["handle"] == handle for a in agents.values()):
                raise ApiError(409, "handle taken")
            aid = str(uuid.uuid4())
            a = {"id": aid, "handle": handle, "display_name": b.get("display_name") or handle,
                 "owner": b.get("owner") or inv.get("owner"), "runtime": b.get("runtime") or "other",
                 "description": b.get("description") or "", "created_at": now(),
                 "last_seen_at": None, "revoked_at": None}
            agents[aid] = a
            key = "ah_" + secrets.token_urlsafe(32)
            keys[key] = aid
            inv["redeemed_at"] = now()
            inv["redeemed_by"] = aid
            return 201, {"agent": public_agent(a), "agent_key": key}

        me = self.agent()
        if p == ["me"] and m == "GET":
            return 200, {"agent": {**public_agent(me), "interests": sorted(interests.get(me["id"], set()))}}
        if p == ["me"] and m == "PATCH":
            b = self.body()
            if "description" in b:
                me["description"] = b["description"]
            if "interests" in b:
                interests[me["id"]] = set(tags_of(b["interests"]))
            return 200, {"agent": {**public_agent(me), "interests": sorted(interests.get(me["id"], set()))}}
        if p == ["agents"] and m == "GET":
            return 200, {"agents": [{**public_agent(a), "shared_artifacts": sum(
                1 for x in artifacts.values() if x["owner_agent"] == a["id"] and x["visibility"] == "shared")}
                for a in agents.values() if not a["revoked_at"]]}
        if p == ["messages"] and m == "POST":
            b = self.body()
            to = b.get("to", "")
            body = b.get("body", "")
            if not body:
                raise ApiError(400, "body required")
            kind = b.get("kind") or "chat"
            if to.startswith("@"):
                t = next((a for a in agents.values() if a["handle"] == to[1:]), None)
                if not t:
                    raise ApiError(404, f"no agent {to}")
                msg = deliver(me["id"], body, kind, to_id=t["id"], thread_id=b.get("thread_id"))
            elif to.startswith("#"):
                msg = deliver(me["id"], body, kind, channel=to[1:], thread_id=b.get("thread_id"))
            else:
                raise ApiError(400, "to must be @handle or #channel")
            return 200, {"message": msg}
        if p == ["inbox"] and m == "GET":
            unread = q.get("unread", "false") == "true"
            limit = int(q.get("limit", 50))
            rows = []
            for msg in reversed(messages):
                key = (msg["id"], me["id"])
                if key in inbox and (not unread or inbox[key] is None):
                    rows.append({**msg, "read_at": inbox[key]})
            unread_n = sum(1 for (mid, aid), r in inbox.items() if aid == me["id"] and r is None)
            return 200, {"items": rows[:limit], "unread": unread_n}
        if p == ["inbox", "read"] and m == "POST":
            ids = self.body().get("ids", [])
            n = 0
            for i in ids:
                if (i, me["id"]) in inbox and inbox[(i, me["id"])] is None:
                    inbox[(i, me["id"])] = now()
                    n += 1
            return 200, {"marked": n}
        if p == ["channels"] and m == "GET":
            names = sorted({x["channel"] for x in messages if x["channel"]} | {"general", "shared"})
            return 200, {"channels": [{"name": n} for n in names]}
        if len(p) == 3 and p[0] == "channels" and p[2] == "messages" and m == "GET":
            rows = [x for x in messages if x["channel"] == p[1]]
            return 200, {"channel": p[1], "messages": rows[-int(q.get("limit", 50)):]}
        if len(p) == 3 and p[0] == "dms" and p[2] == "messages" and m == "GET":
            handle = p[1].lstrip("@")
            other = next((a for a in agents.values() if a["handle"] == handle), None)
            if not other:
                raise ApiError(404, f"no agent @{handle}")
            limit = min(int(q.get("limit", 50)), 200)
            before = q.get("before")
            rows = []
            for msg in reversed(messages):  # newest first
                if msg["channel"]:
                    continue
                pair = {msg.get("from_agent"), msg.get("to_agent")}
                if pair != {me["id"], other["id"]}:
                    continue
                if before and (msg["created_at"] or "") >= before:
                    continue
                rows.append(msg)
                if len(rows) >= limit:
                    break
            return 200, {"with": handle, "messages": rows}
        if len(p) == 3 and p[0] == "artifacts" and m == "PUT":
            kind, name = p[1], p[2]
            if kind not in ("skill", "knowledge", "memory", "mcp"):
                raise ApiError(400, "kind must be skill, knowledge, memory or mcp")
            # Server notes: visibility defaults to shared, memory defaults to private.
            art, did = upsert(me["id"], kind, name, self.body(), "private" if kind == "memory" else "shared")
            return 200, {"artifact": meta(art), "changed": did}
        if p == ["artifacts"] and m == "GET":
            out = []
            for a in sorted(artifacts.values(), key=lambda a: a["updated_at"], reverse=True):
                if a["visibility"] != "shared" and a["owner_agent"] != me["id"]:
                    continue
                if q.get("kind") and a["kind"] != q["kind"]:
                    continue
                if q.get("tag") and q["tag"].lower() not in a["tags"]:
                    continue
                if q.get("owner") and handle_of(a["owner_agent"]) != q["owner"].lstrip("@"):
                    continue
                if q.get("q"):
                    needle = q["q"].lower()
                    if not any(needle in (a[f] or "").lower() for f in ("name", "summary", "content")):
                        continue
                out.append(meta(a))
            return 200, {"artifacts": out}
        if len(p) == 2 and p[0] == "artifacts" and m == "GET":
            a = artifacts.get(p[1])
            if not a or (a["visibility"] != "shared" and a["owner_agent"] != me["id"]):
                raise ApiError(404, "no such artifact")
            if "version" in q:
                try:
                    want = int(q["version"])
                except ValueError:
                    raise ApiError(400, "version must be an integer")
                ver = next((v for v in a["versions"] if v["version"] == want), None)
                if not ver:
                    raise ApiError(404, "no such version")
                return 200, {"artifact": meta(a), "version": ver}
            return 200, {"artifact": {**meta(a), "content": a["content"]}, "versions": list(reversed(a["versions"])),
                         "adoptions": [{"agent_id": x["agent_id"], "handle": handle_of(x["agent_id"]),
                                        "version": x["version"], "adopted_at": x["adopted_at"]}
                                       for x in adoptions if x["artifact_id"] == a["id"]]}
        if len(p) == 3 and p[0] == "artifacts" and p[2] == "adopt" and m == "POST":
            a = artifacts.get(p[1])
            if not a or a["visibility"] != "shared":
                raise ApiError(404, "no such artifact")
            adoptions.append({"agent_id": me["id"], "artifact_id": a["id"], "version": a["version"], "adopted_at": now()})
            deliver(me["id"], f"@{me['handle']} adopted your {a['kind']} `{a['name']}` v{a['version']}.",
                    "system", to_id=a["owner_agent"], artifact_id=a["id"])
            return 200, {"adoption": {"artifact_id": a["id"], "agent_id": me["id"], "handle": me["handle"],
                                      "version": a["version"], "adopted_at": now()}}
        if p == ["sync"] and m == "POST":
            b = self.body()
            changed, unchanged = [], 0
            groups = (("memories", "memory", "private"), ("skills", "skill", "shared"), ("mcps", "mcp", "shared"))
            # Validate everything first so a 422 leaves nothing half-applied.
            for field, kind, _ in groups:
                for item in b.get(field, []) or []:
                    if kind == "mcp":
                        c = item.get("content", "")
                        c = c if isinstance(c, str) else json.dumps(c)
                        if SECRET_RE.search(c):
                            raise ApiError(422, f"mcp `{item.get('name')}` looks like it contains a secret. Use ${{ENV_VAR}} placeholders.")
            for field, kind, vis in groups:
                for item in b.get(field, []) or []:
                    # Server notes: same defaults for sync; items may set visibility.
                    art, did = upsert(me["id"], kind, item["name"], item, vis)
                    if did:
                        changed.append({"id": art["id"], "kind": kind, "name": art["name"], "version": art["version"]})
                    else:
                        unchanged += 1
            return 200, {"changed": changed, "unchanged": unchanged, "suggestions": suggestions_for(me["id"])}
        if p == ["feed"] and m == "GET":
            since = q.get("since", "")
            items = [meta(a) for a in sorted(artifacts.values(), key=lambda a: a["updated_at"], reverse=True)
                     if a["visibility"] == "shared" and a["updated_at"] > since]
            msgs = [x for x in messages if x["channel"] and x["created_at"] > since]
            # Shape follows the v0.1 skeleton's stub: {since, now, agents, artifacts, messages, suggestions}.
            new_agents = [public_agent(a) for a in agents.values() if a["created_at"] > since]
            return 200, {"since": since or None, "now": now(), "agents": new_agents, "artifacts": items, "messages": msgs[-50:],
                         "suggestions": suggestions_for(me["id"])}
        raise ApiError(404, "not found")

    def handle_any(self, method):
        try:
            status, obj = self.dispatch(method)
            self.send_json(status, obj)
        except ApiError as e:
            self.send_json(e.status, {"error": e.msg})
        except Exception as e:  # pragma: no cover
            self.send_json(500, {"error": repr(e)})

    def do_GET(self):
        self.handle_any("GET")

    def do_POST(self):
        self.handle_any("POST")

    def do_PUT(self):
        self.handle_any("PUT")

    def do_PATCH(self):
        self.handle_any("PATCH")

    def do_DELETE(self):
        self.handle_any("DELETE")


def main():
    if not ADMIN_KEY:
        sys.exit("HUB_ADMIN_KEY is required")
    bind = os.environ.get("HUB_BIND", "127.0.0.1:8787")
    if "--bind" in sys.argv:
        bind = sys.argv[sys.argv.index("--bind") + 1]
    host, port = bind.rsplit(":", 1)
    srv = ThreadingHTTPServer((host, int(port)), Handler)
    print(f"mock agent-hub listening on http://{host}:{srv.server_port}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
