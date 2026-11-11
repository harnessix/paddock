#!/usr/bin/env bash
# The CLI of the Mac launcher, checked on a Mac: `paddock events --follow
# --json`, `paddock doctor --json`, `paddock app add --dry-run --json`,
# `paddock macvm setup --pause-download --json` and `paddock macvm status
# --json`. Each check prints `PASS name`, `FAIL name: reason` or `SKIP
# name: reason`; the script exits 0 when every check that ran passed.
#
# Checks use temporary configuration, state and runtime folders. The session
# check uses the real data folder for its sealed golden and a temporary profile.
#
# Usage:
#   packaging/macos/checks/launcher-cli.sh all
#   packaging/macos/checks/launcher-cli.sh events   # one check
#
# What proves each check:
#   doctor    the JSON has v = 1, a chip name, apple_silicon = true, a
#             macOS version, free space over 0 and the entitlement of
#             paddock-vm present.
#   events    a follower prints `stream started` first; `paddock app rules
#             chatgpt-desktop add` and `remove` arrive as two `grant` lines; the
#             socket of the follower is gone after its SIGTERM.
#   pause     with no build running, --pause-download answers
#             `no_build` with exit 1, and status --json has paused = false.
#   add       `app add /System/Applications/TextEdit.app --dry-run --json`
#             has v = 1, display_name TextEdit, a version and app_store =
#             false, and writes no configuration.
#   session   `paddock app add` makes the profile launcher-check-textedit;
#             a follower sees `session` lines starting, running and
#             stopped for it around `paddock app run` and `stop`; then
#             `paddock app remove --delete-data --yes --json` reports
#             freed bytes over 0.
set -uo pipefail
which=${1:-all}
paddock=${PADDOCK:-paddock}
tmp=$(mktemp -d /tmp/paddock-check.XXXXXX)
trap 'rm -rf "$tmp"' EXIT
status=0
pass() { echo "PASS $1"; }
fail() {
  echo "FAIL $1: $2"
  status=1
}
skip() { echo "SKIP $1: $2"; }
want() { [ "$which" = all ] || [ "$which" = "$1" ]; }
# `py EXPR FILE`: evaluate EXPR with `j` the JSON of FILE (one object) or
# `lines` the JSON lines of FILE.
py() {
  /usr/bin/python3 -c '
import json, sys
text = open(sys.argv[2]).read()
lines = [json.loads(l) for l in text.splitlines() if l.strip().startswith("{")]
j = lines[0] if lines else None
sys.exit(0 if eval(sys.argv[1]) else 1)' "$1" "$2"
}
iso() {
  env XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" \
    XDG_STATE_HOME="$tmp/state" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run" "$@"
}

if want doctor; then
  iso "$paddock" doctor --json >"$tmp/doctor.json" 2>"$tmp/doctor.err"
  if py 'j["v"] == 1 and j["chip"]["name"] and j["chip"]["apple_silicon"] and j["macos"]["version"] and j["disk"]["free"] > 0 and j["entitlement"]["present"] is True' "$tmp/doctor.json"; then
    pass doctor
  else
    fail doctor "$(head -c 400 "$tmp/doctor.json") $(tail -2 "$tmp/doctor.err")"
  fi
fi

if want events; then
  iso "$paddock" events --follow --json >"$tmp/events.out" 2>"$tmp/events.err" &
  follower=$!
  sleep 2
  iso "$paddock" app rules chatgpt-desktop add launcher-check.invalid --deny >"$tmp/rules.out" 2>&1
  id=$(sed -n "s/^Added rule \([^ ]*\) .*/\1/p" "$tmp/rules.out" | head -1)
  id=${id:-$(iso "$paddock" app rules chatgpt-desktop | awk '/launcher-check.invalid/ {print $1; exit}')}
  iso "$paddock" app rules chatgpt-desktop remove "$id" >>"$tmp/rules.out" 2>&1
  sleep 2
  kill -TERM "$follower"
  wait "$follower" 2>/dev/null
  socks=$(find "$tmp/run" -path '*/events/*.sock' -name "$follower.sock" 2>/dev/null)
  if ! py 'lines[0]["kind"] == "stream" and lines[0]["state"] == "started"' "$tmp/events.out"; then
    fail events "no stream started line: $(head -c 300 "$tmp/events.out") $(tail -2 "$tmp/events.err")"
  elif ! py '[l["action"] for l in lines if l["kind"] == "grant"] == ["add", "remove"]' "$tmp/events.out"; then
    fail events "the grant lines: $(cat "$tmp/events.out") / rules: $(cat "$tmp/rules.out")"
  elif [ -n "$socks" ]; then
    fail events "the socket stayed after SIGTERM: $socks"
  else
    pass events
  fi
fi

if want pause; then
  iso "$paddock" macvm setup --pause-download --json >"$tmp/pause.json" 2>&1
  code=$?
  iso "$paddock" macvm status --json >"$tmp/status.json" 2>&1
  if [ "$code" = 1 ] && py 'j["v"] == 1 and j["result"] == "no_build"' "$tmp/pause.json" &&
    /usr/bin/python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if j["v"] == 1 and j["paused"] is False and j["setup"]["running"] is False else 1)' "$tmp/status.json"; then
    pass pause
  else
    fail pause "exit $code: $(cat "$tmp/pause.json") / $(head -c 300 "$tmp/status.json")"
  fi
fi

if want add; then
  env XDG_CONFIG_HOME="$tmp/addconfig" XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" \
    "$paddock" app add /System/Applications/TextEdit.app --dry-run --json >"$tmp/add.json" 2>&1
  if py 'j["v"] == 1 and j["display_name"] == "TextEdit" and j["version"] and j["app_store"] is False and j["dry_run"] is True' "$tmp/add.json" &&
    [ ! -e "$tmp/addconfig/paddock/config.toml" ]; then
    pass add
  else
    fail add "$(cat "$tmp/add.json")"
  fi
fi

if want session; then
  export XDG_CONFIG_HOME="$tmp/session-config" XDG_STATE_HOME="$tmp/session-state" XDG_RUNTIME_DIR="$tmp/session-run"
  if ! "$paddock" macvm status --json 2>/dev/null | grep -q '"state": "sealed"'; then
    skip session "no sealed golden: run paddock macvm setup first"
  else
    app=launcher-check-textedit
    "$paddock" app add /System/Applications/TextEdit.app --name "$app" >"$tmp/run.out" 2>&1
    "$paddock" events --follow --json >"$tmp/session.out" 2>&1 &
    follower=$!
    sleep 2
    "$paddock" app run "$app" >>"$tmp/run.out" 2>&1 &
    sleep 60
    "$paddock" app stop "$app" >>"$tmp/run.out" 2>&1
    sleep 5
    kill -TERM "$follower"
    wait "$follower" 2>/dev/null
    "$paddock" app remove "$app" --delete-data --yes --json >"$tmp/remove.json" 2>&1
    if ! py 'set(["starting", "running", "stopped"]) <= set(l["state"] for l in lines if l["kind"] == "session" and l["app"] == "launcher-check-textedit")' "$tmp/session.out"; then
      fail session "$(grep '"session"' "$tmp/session.out" | head -20) / $(tail -3 "$tmp/run.out")"
    elif ! py 'j["v"] == 1 and j["freed"] > 0' "$tmp/remove.json"; then
      fail session "app remove: $(cat "$tmp/remove.json")"
    else
      pass session
    fi
  fi
fi

exit "$status"
