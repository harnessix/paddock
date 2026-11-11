#!/usr/bin/env bash
# The Mac side of the security checklist: one line per item that only a
# Mac can see, `ok NN` or `FAIL NN: reason`.
# Items 5, 10, 11 and 16 have a script each; the 17 other items are Linux
# tests (`cargo test macvm::security`). The script ends with `paddock
# doctor`, whose `macvm` block says the state of items 5, 11, 12 and 16.
#
# Usage, on an Apple silicon Mac after `cargo build --release`:
#   PADDOCK_MACVM_GOLDEN=~/.local/share/paddock/macvm/golden.noindex \
#     packaging/macos/checks/security.sh
set -uo pipefail
here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here" || exit 1
fails=0
for check in check-05-golden-flags check-10-kill9 check-11-tcc check-16-slots; do
  if ! "$here/packaging/macos/checks/$check.sh"; then
    fails=$((fails + 1))
  fi
done
echo
echo "paddock doctor (the macvm block):"
PATH="$here/target/release:$PATH" paddock doctor 2>&1 | sed -n '/^macvm:/,/^[a-z]*:$/p' | sed '$d'
echo
if [ "$fails" = 0 ]; then
  echo "security: every Mac item ok"
else
  echo "security: $fails item(s) FAIL"
fi
exit "$fails"
