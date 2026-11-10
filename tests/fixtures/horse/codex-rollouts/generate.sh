#!/bin/sh
# Oracle generation from a disposable upstream checkout:
# git clone --filter=blob:none --no-checkout --depth 1 --branch rust-v0.167.0 https://github.com/openai/codex.git /tmp/codex-rollout-oracle
# git -C /tmp/codex-rollout-oracle fetch --depth 1 origin tag rust-v0.166.0
# tests/fixtures/horse/codex-rollouts/generate.sh /tmp/codex-rollout-oracle
#
# Input examples are parsed by the upstream canonical rollout parser and
# serialized as RolloutLine. The oracle adds an unknown field with a decimal
# after serialization and checks that upstream accepts it. Tests create torn
# tails separately because they are not complete RolloutLine values. The
# temporary workspace is narrowed to avoid downloading unrelated V8 sources.
# Run from any directory; the checkout must contain both requested tags.
set -eu
if [ "$#" -ne 1 ]; then
  echo "usage: $0 /path/to/temporary/codex-checkout" >&2
  exit 2
fi
checkout=$(cd "$1" && pwd)
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for version in 0.166.0 0.167.0; do
  git -C "$checkout" reset --hard "rust-v$version"
  # Restrict Cargo's workspace resolution to the crates used by this oracle.
  # Otherwise it downloads unrelated V8 sources before resolving history.
  python3 - "$checkout/codex-rs/Cargo.toml" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
a = s.index('members = [')
b = s.index('\n]', a) + 2
s = s[:a] + 'members = ["history", "rollout"]' + s[b:]
s = '\n'.join(x for x in s.split('\n') if not x.startswith('v8 = { git = '))
open(p, 'w').write(s)
PY
  mkdir -p "$checkout/codex-rs/rollout/examples"
  cp "$here/oracle.rs" "$checkout/codex-rs/rollout/examples/hx2_oracle.rs"
  python3 "$here/input.py" "$version" | (cd "$checkout/codex-rs" && CARGO_BUILD_JOBS=4 cargo +stable run -q -p codex-rollout --example hx2_oracle) > "$here/$version.jsonl"
done
