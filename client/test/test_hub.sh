#!/usr/bin/env bash
# Offline checks for client/hub.sh: secret stripping and front matter parsing
# must match the shared fixture (the Rust crate checks the same fixture), and
# anything that still looks like a secret must be held back.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
HUB="$HERE/../hub.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
export HUB_HOME="$T/home" HUB_URL="http://127.0.0.1:9" HUB_KEY="ah_offline"
n=0
ok() { n=$((n + 1)); echo "ok $n - $*"; }
fail() { echo "not ok - $*"; exit 1; }

"$HUB" sync "$HERE/fixtures/agent" --dry-run 2>/dev/null > "$T/payload.json"
diff <(jq -S . "$HERE/fixtures/expected_sync.json") <(jq -S . "$T/payload.json") > "$T/diff" \
  && ok "dry-run payload matches fixtures/expected_sync.json" || { cat "$T/diff"; fail "payload differs"; }

for s in ghp_FAKE sk-proj hunter2 hooks.slack.com s3cretpass AKIAIOSFODNN7EXAMPLE ah_FAKE xoxb- abc123def456 FAKEfakeFAKEfake0000abcd; do
  grep -qF "$s" "$T/payload.json" && fail "$s leaked"
done
ok "no fixture secret survives stripping"

mkdir -p "$T/agent/memories"
printf -- '-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n' > "$T/agent/memories/bad.md"
printf 'fine\n' > "$T/agent/memories/good.md"
"$HUB" sync "$T/agent" --dry-run > "$T/p2.json" 2> "$T/err"
grep -q 'skipping memories `bad`' "$T/err" && ok "unterminated private key is skipped with a warning" || fail "bad memory not skipped"
[ "$(jq '.memories | map(.name)' -c "$T/p2.json")" = '["good"]' ] && ok "clean item still sent" || fail "good memory missing"

if "$HUB" share skill bad "$T/agent/memories/bad.md" 2> "$T/err"; then fail "share accepted a secret"; fi
grep -q "still looks like it holds a secret" "$T/err" && ok "share refuses residual secrets before any request" || fail "wrong share error"

out="$("$HUB" inbox 2>&1 || true)"
grep -q "cannot reach" <<<"$out" && ok "unreachable server is a clear error" || fail "bad network error: $out"

HUB_KEY="" HUB_HOME="$T/empty" "$HUB" inbox 2> "$T/err" && fail "ran without a key"
grep -q "no agent key" "$T/err" && ok "missing key is a clear error" || fail "bad missing-key error"

echo "1..$n"
