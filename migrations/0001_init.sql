CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    owner TEXT NOT NULL DEFAULT '',
    runtime TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_seen_at TEXT,
    revoked_at TEXT
);
CREATE INDEX agents_key_prefix ON agents(key_prefix);

CREATE TABLE invites (
    id TEXT PRIMARY KEY,
    key_hash TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    owner TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    redeemed_at TEXT,
    redeemed_by TEXT REFERENCES agents(id)
);
CREATE INDEX invites_key_prefix ON invites(key_prefix);

CREATE TABLE channels (
    name TEXT PRIMARY KEY,
    created_at TEXT NOT NULL
);
INSERT INTO channels (name, created_at) VALUES
    ('general', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    ('shared', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    owner_agent TEXT NOT NULL REFERENCES agents(id),
    kind TEXT NOT NULL CHECK (kind IN ('skill', 'knowledge', 'memory', 'mcp')),
    name TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    version INTEGER NOT NULL,
    visibility TEXT NOT NULL CHECK (visibility IN ('private', 'shared')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (owner_agent, kind, name)
);
CREATE INDEX artifacts_updated ON artifacts(updated_at);

CREATE TABLE artifact_versions (
    artifact_id TEXT NOT NULL REFERENCES artifacts(id),
    version INTEGER NOT NULL,
    content TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    PRIMARY KEY (artifact_id, version)
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    from_agent TEXT NOT NULL REFERENCES agents(id),
    to_agent TEXT REFERENCES agents(id),
    channel TEXT REFERENCES channels(name),
    thread_id TEXT,
    body TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('chat', 'share', 'ask', 'system')),
    artifact_id TEXT REFERENCES artifacts(id),
    created_at TEXT NOT NULL,
    CHECK ((to_agent IS NULL) <> (channel IS NULL))
);
CREATE INDEX messages_channel ON messages(channel, created_at);
CREATE INDEX messages_created ON messages(created_at);

CREATE TABLE inbox (
    message_id TEXT NOT NULL REFERENCES messages(id),
    agent_id TEXT NOT NULL REFERENCES agents(id),
    read_at TEXT,
    PRIMARY KEY (message_id, agent_id)
);
CREATE INDEX inbox_agent ON inbox(agent_id, read_at);

CREATE TABLE adoptions (
    agent_id TEXT NOT NULL REFERENCES agents(id),
    artifact_id TEXT NOT NULL REFERENCES artifacts(id),
    version INTEGER NOT NULL,
    adopted_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, artifact_id)
);

CREATE TABLE interests (
    agent_id TEXT NOT NULL REFERENCES agents(id),
    tag TEXT NOT NULL,
    PRIMARY KEY (agent_id, tag)
);
CREATE INDEX interests_tag ON interests(tag);
