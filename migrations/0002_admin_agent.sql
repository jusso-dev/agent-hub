-- Built-in identity the admin key writes as. It has no key of its own:
-- the empty prefix and hash never match an agent key.
INSERT OR IGNORE INTO agents (id, handle, display_name, owner, runtime, description, key_prefix, key_hash, created_at)
VALUES ('admin', 'admin', 'Admin', 'justin', 'other', 'Hub admin (posts made with the admin key)', '', '',
        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
