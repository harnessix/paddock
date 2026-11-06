#!/usr/bin/env bash
# The firewall of the macOS guest (rules, mode ask, the panel, the
# connection log), checked on a Mac.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image (PADDOCK_MACVM_GOLDEN names it, as macvm-network.sh does).
# It starts one scratch session of the profile `fw` in a temporary data
# folder in network mode ask, and runs `curl` in the guest through the
# bench op `paddock macvm bench spawn-child`. Each check prints one line:
# `PASS name`, `FAIL name: reason` or `SKIP name: reason`. The script
# exits non-zero on a FAIL.
#
# Usage:
#   packaging/macos/checks/macvm-firewall.sh all
#   packaging/macos/checks/macvm-firewall.sh ask        # one check by name
#
# What proves each check (the lines of this script and the files it reads):
#   ask           `curl https://example.com` in the guest blocks; within 1 s
#                 `paddock approvals --json` lists one question with
#                 `"capability":"connect"` and `"host":"example.com"`;
#                 `paddock approvals answer ID --allow-session` lets curl
#                 finish with exit 0; logs/connections.jsonl of the profile
#                 has a line with `"verdict":"ask"` and then one with
#                 `"verdict":"allow"` and `"rule":"session-...`.
#   always        a second host (`https://www.iana.org`) asks; `--allow-always`
#                 writes ~/.config/paddock/firewall/fw.toml (XDG_CONFIG_HOME
#                 of this script) with mode 0600 and one `[[rule]]` for
#                 www.iana.org; the next curl to it is silent (no new
#                 question within 2 s, exit 0) and its log line names the
#                 rule id (`"rule":"rule-...`).
#   deny-timeout  a third host (`https://www.example.org`) asks and nobody
#                 answers: after 60 s curl exits non-zero, the log has a
#                 `"verdict":"deny"` line with `"rule":"timeout"`, and the
#                 question is still in `paddock approvals --json`.
#   panel         with the marker running (`paddock windows --mark`), within
#                 1 s of a question `paddock windows --json` has a row with
#                 `"kind": "approval"`.
#   restart       `kill` of the marker (pgrep -f 'paddock mark') while the
#                 deny-timeout question waits; within 60 s a marker runs
#                 again (the session helper starts it while a connection
#                 waits) and `paddock windows --json` lists the panel of the
#                 same question id.
#   seed          the first run of chatgpt-desktop (macvm-chatgpt.sh) has no
#                 `"verdict":"ask"` line in its connections.jsonl before the
#                 first chat message. This script runs macvm-chatgpt.sh login
#                 when PADDOCK_FW_CHATGPT=1 and reads the log; else SKIP.
#   system        `nscurl https://www.example.net` in the guest (a process
#                 of the guest system, not the app; the bench session counts
#                 only `curl` as the app) makes no question within 5 s; the
#                 log has a `"verdict":"deny"` line with `"rule":"system"`
#                 for www.example.net; `paddock app status fw --json` counts
#                 it under questions.system. The line also lists the hosts
#                 that the guest's own services tried, denied without a
#                 question.
#   queue         five curls to five new hosts at once: within 3 s the store
#                 has exactly one question of the session, with
#                 `"more":4`; `paddock app status fw --json` lists five open
#                 questions, one with `"shown":true`; `paddock windows
#                 --json` has one approval row for the session.
#   rate          the answers of the first three come up one after another;
#                 the fourth does not come up within 20 s of the third
#                 answer (three in this minute), and does within 70 s of
#                 the first.
#   guard         22 curls to new hosts: one notice (`"notice":true`) and
#                 no other question of the session; `paddock app status fw`
#                 says `asking paused`; one approval row in `paddock windows
#                 --json`; `paddock approvals answer ID --deny` on the
#                 notice leaves no question of the session and `none open`.
#   stays         the deny-timeout question keeps its panel after its curl
#                 timed out (the row of `paddock windows --json` with the
#                 same question id is there 5 s after the timeout).
#   look          only with PADDOCK_FW_LOOK=1: a question panel shows; the
#                 script saves it with `screencapture -l` into the output
#                 folder and asks whether the title, the detail and the
#                 three buttons are readable. Switch the appearance (System
#                 Settings, Appearance) when it asks, for Light and Dark.
#
# This is the one check script that shows firewall panels. The rest run
# their sessions with --network none or egress, so no question waits for
# a person. Run it with nothing else open; do not click the panels, the
# script answers them from the terminal.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-fw.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/fw/default"
log="$profile/logs/connections.jsonl"
rules="$XDG_CONFIG_HOME/paddock/firewall/fw.toml"
name=fw

cleanup() {
  paddock app stop "$name" >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $root/profiles.noindex" | xargs -r kill -9 2>/dev/null
  pgrep -f "paddock mark" | xargs -r kill 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

cat <<'TELL'
macvm-firewall.sh shows firewall panels of a scratch session named fw, at
the top right of the screen, for about 10 minutes:
  - one panel at a time, for hosts like example.com and q1.paddock-check.example;
  - a panel with "and 4 more waiting";
  - one notice "fw made many new connections";
  - with PADDOCK_FW_LOOK=1, a question about how the panel looks.
Do not click the panels: the script answers them from the terminal.
TELL

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return 1
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return 1
  fi
  if ! cargo test --release --lib -- macvm::firewall macvm::connlog macvm::approvals macvm::gate macvm::ask_queue macvm::ask_status macvm::owner mark::panels broker::approval >"$tmp/test.log" 2>&1; then
    fail build "cargo test: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return 1
  fi
  pass build
}

has_bench() { paddock macvm bench --help 2>/dev/null | grep -q spawn-child; }

# Run CMD in the guest in the background; its exit code lands in FILE.
guest_bg() {
  local file=$1 cmd=$2
  (paddock macvm bench spawn-child --session "$name" -- /bin/sh -c "$cmd" >/dev/null 2>&1; echo $? >"$file") &
}

# The id of the one connect question for HOST, or nothing.
question_id() {
  paddock approvals --json 2>/dev/null | python3 -c '
import json, sys
host = sys.argv[1]
for p in json.load(sys.stdin).get("pending", []):
    c = p.get("connect") or {}
    if p.get("capability") == "connect" and c.get("host") == host:
        print(p["id"]); break
' "$1"
}

# Wait up to SECS for a question for HOST; prints its id.
wait_question() {
  local host=$1 secs=$2 i id
  for i in $(seq 1 $((secs * 10))); do
    id=$(question_id "$host")
    if [ -n "$id" ]; then echo "$id"; return 0; fi
    sleep 0.1
  done
  return 1
}

start_session() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ]; then
    skip session "PADDOCK_MACVM_GOLDEN names no sealed golden"; return 1
  fi
  if ! has_bench; then
    skip session "needs the bench op paddock macvm bench spawn-child"; return 1
  fi
  mkdir -p "$root"
  cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" 2>/dev/null || cp -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex"
  (paddock app run "$name" --backend macvm --network ask --bench >"$tmp/run.log" 2>&1 &)
  for _ in $(seq 1 120); do
    grep -q '"kind": *"hello"\|"kind":"hello"' "$profile/logs/events.jsonl" 2>/dev/null && return 0
    sleep 1
  done
  fail session "no hello within 120 s: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"
  return 1
}

check_ask() {
  local id code
  guest_bg "$tmp/ask.exit" 'curl -sS -o /dev/null --max-time 90 https://example.com'
  if ! id=$(wait_question example.com 1); then
    id=$(wait_question example.com 10) && fail ask "the question took more than 1 s" || fail ask "no connect question for example.com within 10 s"
    return
  fi
  paddock approvals answer "$id" --allow-session >"$tmp/answer.log" 2>&1 || { fail ask "answer: $(cat "$tmp/answer.log")"; return; }
  for _ in $(seq 1 100); do [ -f "$tmp/ask.exit" ] && break; sleep 0.1; done
  code=$(cat "$tmp/ask.exit" 2>/dev/null)
  if [ "$code" != 0 ]; then fail ask "curl exited $code after --allow-session"; return; fi
  if grep -q '"host":"example.com".*"verdict":"ask"' "$log" && grep -q '"host":"example.com".*"verdict":"allow".*"rule":"session-' "$log"; then
    pass ask
  else
    fail ask "the log lacks the ask line or the allow line with rule session-: $(grep example.com "$log" | tr '\n' ' ' | cut -c1-300)"
  fi
}

check_always() {
  local id code mode
  guest_bg "$tmp/always.exit" 'curl -sS -o /dev/null --max-time 90 https://www.iana.org'
  id=$(wait_question www.iana.org 10) || { fail always "no question for www.iana.org"; return; }
  paddock approvals answer "$id" --allow-always >"$tmp/answer.log" 2>&1 || { fail always "answer: $(cat "$tmp/answer.log")"; return; }
  for _ in $(seq 1 100); do [ -f "$tmp/always.exit" ] && break; sleep 0.1; done
  [ -f "$rules" ] || { fail always "no rules file at $rules"; return; }
  mode=$(stat -f '%Lp' "$rules")
  [ "$mode" = 600 ] || { fail always "the rules file has mode $mode, not 600"; return; }
  grep -q 'host = "www.iana.org"' "$rules" || { fail always "the rules file names no www.iana.org rule: $(cat "$rules" | tr '\n' ' ' | cut -c1-300)"; return; }
  rm -f "$tmp/always2.exit"
  guest_bg "$tmp/always2.exit" 'curl -sS -o /dev/null --max-time 30 https://www.iana.org'
  if wait_question www.iana.org 2 >/dev/null; then fail always "the second curl asked again"; return; fi
  for _ in $(seq 1 300); do [ -f "$tmp/always2.exit" ] && break; sleep 0.1; done
  code=$(cat "$tmp/always2.exit" 2>/dev/null)
  [ "$code" = 0 ] || { fail always "the second curl exited $code"; return; }
  if grep -q '"host":"www.iana.org".*"verdict":"allow".*"rule":"rule-' "$log"; then
    pass always
  else
    fail always "no allow line with the rule id: $(grep iana "$log" | tr '\n' ' ' | cut -c1-300)"
  fi
}

check_deny_timeout() {
  local id code start end
  start=$(date +%s)
  guest_bg "$tmp/timeout.exit" 'curl -sS -o /dev/null --max-time 90 https://www.example.org'
  # The fourth question of the first minute: the rate limit (three per
  # minute) may hold it back until a minute after the first.
  id=$(wait_question www.example.org 75) || { fail deny-timeout "no question for www.example.org within 75 s"; return; }
  echo "$id" >"$tmp/timeout.id"
  for _ in $(seq 1 900); do [ -f "$tmp/timeout.exit" ] && break; sleep 0.1; done
  end=$(date +%s)
  echo "$end" >"$tmp/timeout.end"
  code=$(cat "$tmp/timeout.exit" 2>/dev/null)
  [ "$code" != 0 ] || { fail deny-timeout "curl exited 0 with no answer"; return; }
  [ $((end - start)) -ge 55 ] && [ $((end - start)) -le 75 ] || { fail deny-timeout "curl ended after $((end - start)) s, not about 60"; return; }
  grep -q '"host":"www.example.org".*"verdict":"deny".*"rule":"timeout"' "$log" || { fail deny-timeout "no deny line with rule timeout"; return; }
  if [ "$(question_id www.example.org)" = "$id" ]; then
    pass deny-timeout
  else
    fail deny-timeout "the question is gone after the timeout"
  fi
}

check_panel() {
  paddock windows --mark >/dev/null 2>&1
  guest_bg "$tmp/panel.exit" 'curl -sS -o /dev/null --max-time 90 https://www.wikipedia.org'
  local id
  id=$(wait_question www.wikipedia.org 10) || { fail panel "no question for www.wikipedia.org"; return; }
  sleep 1
  if paddock windows --json | grep -q '"kind": *"approval"'; then
    pass panel
  else
    fail panel "paddock windows --json lists no approval window: $(paddock windows --json | tr '\n' ' ' | cut -c1-300)"
  fi
  paddock approvals answer "$id" --deny >/dev/null 2>&1
}

check_restart() {
  local id pid
  id=$(cat "$tmp/timeout.id" 2>/dev/null)
  [ -n "$id" ] || { guest_bg "$tmp/restart.exit" 'curl -sS -o /dev/null --max-time 90 https://www.example.org'; id=$(wait_question www.example.org 10) || { fail restart "no question"; return; }; }
  # A connection must wait, so the helper brings the marker back.
  guest_bg "$tmp/restart2.exit" 'curl -sS -o /dev/null --max-time 90 https://www.example.org'
  sleep 2
  pid=$(pgrep -f "paddock mark" | head -1)
  [ -n "$pid" ] || { fail restart "no marker runs"; return; }
  kill "$pid"
  for _ in $(seq 1 60); do
    if pgrep -f "paddock mark" >/dev/null && paddock windows --json | grep -q "\"sandbox\": *\"macvm-$name\"" && paddock windows --json | grep -q '"kind": *"approval"'; then
      pass restart; paddock approvals answer "$id" --deny >/dev/null 2>&1; return
    fi
    sleep 1
  done
  fail restart "no marker with the panel within 60 s of the kill"
}

# The connect questions of the session fw, as JSON lines.
session_questions() {
  paddock approvals --json 2>/dev/null | python3 -c '
import json, sys
for p in json.load(sys.stdin).get("pending", []):
    if p.get("capability") == "connect" and p.get("sandbox") == "macvm-fw":
        print(json.dumps(p))'
}

# The approval rows of the session fw in paddock windows --json.
approval_rows() {
  paddock windows --json 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
rows = d if isinstance(d, list) else d.get("windows", [])
print(sum(1 for r in rows if r.get("kind") == "approval" and r.get("sandbox") == "macvm-fw"))'
}

# A field of `paddock app status fw --json` under questions.
status_q() {
  paddock app status "$name" --json 2>/dev/null | python3 -c '
import json, sys
q = json.load(sys.stdin).get("questions", {})
print(eval(sys.argv[1], {}, {"q": q}))' "$1"
}

# Answer every question of the session with --deny until none is left
# (the rate limit brings them up one by one), at most SECS seconds.
drain() {
  local secs=$1 end id
  end=$(( $(date +%s) + secs ))
  while [ "$(date +%s)" -lt "$end" ]; do
    [ "$(status_q 'len(q.get("open", []))')" = 0 ] && return 0
    id=$(session_questions | head -1 | python3 -c 'import json,sys; l=sys.stdin.read(); print(json.loads(l)["id"] if l.strip() else "")')
    [ -n "$id" ] && paddock approvals answer "$id" --deny >/dev/null 2>&1
    sleep 1
  done
  return 1
}

check_stays() {
  local id end
  id=$(cat "$tmp/timeout.id" 2>/dev/null)
  end=$(cat "$tmp/timeout.end" 2>/dev/null)
  [ -n "$id" ] && [ -n "$end" ] || { skip stays "needs deny-timeout first"; return; }
  pgrep -f "paddock mark" >/dev/null || { skip stays "no marker runs"; return; }
  while [ $(( $(date +%s) - end )) -lt 5 ]; do sleep 1; done
  if paddock windows --json | grep -q "\"question\": *\"$id\""; then
    pass stays
  else
    fail stays "the panel of $id is gone 5 s after its curl timed out"
  fi
}

check_system() {
  local before hosts
  guest_bg "$tmp/nscurl.exit" 'test -x /usr/bin/nscurl || exit 99; /usr/bin/nscurl https://www.example.net >/dev/null 2>&1'
  sleep 5
  if [ "$(cat "$tmp/nscurl.exit" 2>/dev/null)" = 99 ]; then skip system "no /usr/bin/nscurl in the guest"; return; fi
  if session_questions | grep -q '"host": *"www.example.net"'; then
    fail system "a process of the guest system asked"; return
  fi
  grep -q '"host":"www.example.net".*"verdict":"deny".*"rule":"system"' "$log" \
    || { fail system "no deny line with rule system for www.example.net: $(grep example.net "$log" | tr '\n' ' ' | cut -c1-300)"; return; }
  [ "$(status_q '[h["host"] for h in q.get("system", [])].count("www.example.net")')" = 1 ] \
    || { fail system "paddock app status fw --json does not count www.example.net under questions.system"; return; }
  if grep '"verdict":"ask"' "$log" | grep -qv '"app":"com.apple.curl"'; then
    fail system "a question came from another process than curl: $(grep '"verdict":"ask"' "$log" | grep -v com.apple.curl | head -3 | tr '\n' ' ' | cut -c1-300)"; return
  fi
  hosts=$(status_q '", ".join("%s x%d" % (h["host"], h["count"]) for h in q.get("system", []))')
  pass "system (guest system hosts denied without a question: $hosts)"
}

check_queue_rate() {
  local i one more first t id n
  # A fresh minute for the rate limit, and nothing open.
  drain 150 || { fail queue "the questions of the checks before did not clear"; return; }
  sleep 61
  for i in 1 2 3 4 5; do
    guest_bg "$tmp/q$i.exit" "curl -sS -o /dev/null --max-time 90 https://q$i.paddock-check.example"
  done
  sleep 3
  n=$(session_questions | wc -l | tr -d ' ')
  [ "$n" = 1 ] || { fail queue "$n questions of the session in the store, not one"; return; }
  more=$(session_questions | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["connect"].get("more", 0))')
  [ "$more" = 4 ] || { fail queue "the question says $more more, not 4"; return; }
  [ "$(status_q 'len(q.get("open", []))')" = 5 ] || { fail queue "paddock app status lists $(status_q 'len(q.get("open", []))') open questions, not 5"; return; }
  [ "$(status_q 'sum(1 for o in q.get("open", []) if o.get("shown"))')" = 1 ] || { fail queue "not exactly one question on the screen in the status"; return; }
  if pgrep -f "paddock mark" >/dev/null; then
    [ "$(approval_rows)" = 1 ] || { fail queue "$(approval_rows) approval panels of the session, not one"; return; }
  fi
  pass queue
  first=$(date +%s)
  for i in 1 2 3; do
    t=$(( $(date +%s) + 10 ))
    id=""
    while [ -z "$id" ] && [ "$(date +%s)" -lt "$t" ]; do
      id=$(session_questions | head -1 | python3 -c 'import json,sys; l=sys.stdin.read(); print(json.loads(l)["id"] if l.strip() else "")')
      [ -z "$id" ] && sleep 0.2
    done
    [ -n "$id" ] || { fail rate "question $i did not come up within 10 s of the answer before"; return; }
    paddock approvals answer "$id" --deny >/dev/null 2>&1
  done
  sleep 20
  [ "$(session_questions | wc -l | tr -d ' ')" = 0 ] || { fail rate "a fourth question came up within the minute"; return; }
  while [ $(( $(date +%s) - first )) -lt 70 ]; do
    [ "$(session_questions | wc -l | tr -d ' ')" = 1 ] && { pass "rate (the fourth came up $(( $(date +%s) - first )) s after the first)"; return; }
    sleep 1
  done
  fail rate "the fourth question never came up within 70 s"
}

check_guard() {
  local i n notice
  for i in $(seq 1 22); do
    guest_bg "$tmp/g$i.exit" "curl -sS -o /dev/null --max-time 90 https://g$i.paddock-check.example"
  done
  sleep 5
  n=$(session_questions | wc -l | tr -d ' ')
  notice=$(session_questions | python3 -c '
import json, sys
ls = [json.loads(l) for l in sys.stdin if l.strip()]
print(ls[0]["id"] if len(ls) == 1 and ls[0]["connect"].get("notice") else "")')
  [ -n "$notice" ] || { fail guard "not one notice: $n cards of the session"; return; }
  paddock app status "$name" | grep -q 'asking paused' || { fail guard "paddock app status says nothing of the pause"; return; }
  if pgrep -f "paddock mark" >/dev/null; then
    [ "$(approval_rows)" = 1 ] || { fail guard "$(approval_rows) approval panels, not one"; return; }
  fi
  paddock approvals answer "$notice" --deny >"$tmp/notice.log" 2>&1 || { fail guard "answer: $(cat "$tmp/notice.log")"; return; }
  sleep 3
  [ "$(session_questions | wc -l | tr -d ' ')" = 0 ] || { fail guard "questions are left after deny all"; return; }
  paddock app status "$name" | grep -q 'none open' || { fail guard "paddock app status does not say none open"; return; }
  pass guard
}

check_look() {
  if [ "${PADDOCK_FW_LOOK:-0}" != 1 ]; then
    skip look "set PADDOCK_FW_LOOK=1 to look at the panel in Light and Dark"; return
  fi
  local keep="$HOME/paddock-fw-look-$$" mode wid answer
  mkdir -p "$keep"
  guest_bg "$tmp/look.exit" 'curl -sS -o /dev/null --max-time 90 https://look.paddock-check.example'
  wait_question look.paddock-check.example 75 >/dev/null || { fail look "no question"; return; }
  sleep 2
  wid=$(paddock windows --json | python3 -c '
import json, sys
d = json.load(sys.stdin)
rows = d if isinstance(d, list) else d.get("windows", [])
print(next((r.get("window", r.get("id", "")) for r in rows if r.get("kind") == "approval"), ""))')
  for mode in Light Dark; do
    echo "look: set the appearance to $mode (System Settings, Appearance), then press Enter"
    read -r _
    [ -n "$wid" ] && screencapture -x -o -l "$wid" "$keep/panel-$mode.png"
    echo "look: in $mode, can you read the title, the detail and all three buttons (Deny, Allow once, Always allow)? [y/n]"
    read -r answer
    [ "$answer" = y ] || { fail look "not readable in $mode (picture: $keep/panel-$mode.png)"; return; }
  done
  pass "look (pictures in $keep)"
  paddock approvals answer "$(question_id look.paddock-check.example)" --deny >/dev/null 2>&1
}

check_seed() {
  if [ "${PADDOCK_FW_CHATGPT:-0}" != 1 ]; then
    skip seed "set PADDOCK_FW_CHATGPT=1 to run macvm-chatgpt.sh login and read its connection log"; return
  fi
  local chat="$root/profiles.noindex/chatgpt-desktop/default/logs/connections.jsonl"
  packaging/macos/checks/macvm-chatgpt.sh login >"$tmp/chatgpt.log" 2>&1
  [ -f "$chat" ] || { fail seed "no connection log of chatgpt-desktop: $(tail -3 "$tmp/chatgpt.log" | tr '\n' ' ')"; return; }
  if grep -q '"verdict":"ask"' "$chat"; then
    fail seed "the first login asked: $(grep '"verdict":"ask"' "$chat" | head -3 | tr '\n' ' ' | cut -c1-300)"
  else
    pass "seed (hosts: $(python3 -c 'import json,sys; print(sorted({json.loads(l)["host"] for l in open(sys.argv[1]) if l.strip()}))' "$chat"))"
  fi
}

build || exit 1
if [ "$what" = build ]; then exit $fails; fi
if [ "$what" = seed ]; then check_seed; exit $fails; fi
start_session || exit $fails
case "$what" in
  all) check_ask; check_always; check_panel; check_deny_timeout; check_stays; check_restart
       check_system; check_queue_rate; check_guard; check_look; check_seed ;;
  ask|always|panel|restart|system|guard|look) "check_$what" ;;
  queue|rate) check_queue_rate ;;
  deny-timeout) check_deny_timeout ;;
  stays) check_deny_timeout; check_stays ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
exit $fails
