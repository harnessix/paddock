#!/usr/bin/env bash
# Leak guard: stop private files and private data from entering this public repo.
#
# It fails when:
#   1. A tracked file is a private agent or design note
#      (docs/DESIGN.md, CLAUDE.md, CLAUDE.local.md, AGENTS.md, .claude/).
#   2. A tracked file contains a blocked internal name.
#   3. A tracked file contains a home path such as /home/<user>/.
#      Use /home/demo/ in examples.
#   4. A commit in the range (first argument, for example origin/main..HEAD)
#      has an author or committer email that is not a noreply address.
#   5. A tracked file is a document that is not on the public list below.
#      Design notes, plans and other internal documents stay out of this
#      repository. Only code, the user documentation and images are public.
#   6. A tracked file matches a pattern of the optional local file that
#      $LEAK_GUARD_PATTERNS names (default: info/leak-patterns in the git
#      folder of the repository): extended regular expressions, one per
#      line, matched as words and in any case. The file stays outside the
#      repository, so the patterns are never published. The released
#      sections of CHANGELOG.md are history and stay as they are.
#
# To fix a failure, remove the file or text, or rewrite the commit with a
# GitHub noreply email (Settings > Emails > Keep my email address private).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
self=.github/scripts/leak-guard.sh
status=0

bad_files=$(git ls-files | grep -E '^(docs/DESIGN\.md|(.*/)?CLAUDE(\.local)?\.md|(.*/)?AGENTS\.md|(.*/)?\.claude/)' || true)
if [ -n "$bad_files" ]; then
  echo "leak guard: these private files must not be tracked:"; echo "$bad_files"; status=1
fi

# The only documents this repository publishes. Anything else under docs/
# (except images in docs/assets/), and any other text document, fails.
public_docs='^(README\.md|mac/launcher/README\.md|CHANGELOG\.md|CONTRIBUTING\.md|SECURITY\.md|\.github/pull_request_template\.md|changes/[^/]+\.md|skills/[^/]+/SKILL\.md|skills/[^/]+/references/[^/]+\.md|docs/assets/[^/]+\.(png|gif|svg|jpg|jpeg|webp))$'
docs=$(git ls-files | grep -iE '\.(md|markdown|txt|rst|adoc|org)$|^docs/' | grep -vE "$public_docs" || true)
if [ -n "$docs" ]; then
  echo "leak guard: these documents must not be published (only code and the user docs):"; echo "$docs"; status=1
fi

# Internal names, built from parts so this file does not contain them.
blocked="r""e""o""l"
hits=$(git grep -nIi -e "$blocked" -- . ":!$self" || true)
if [ -n "$hits" ]; then
  echo "leak guard: blocked internal name found:"; echo "$hits"; status=1
fi

hits=$(git grep -noIE '/home/[A-Za-z0-9_.-]+' -- . ":!$self" | grep -vE ':/home/demo$' || true)
if [ -n "$hits" ]; then
  echo "leak guard: private home path found (use /home/demo/):"; echo "$hits"; status=1
fi

# The local patterns, from a file outside the repository.
patterns=${LEAK_GUARD_PATTERNS:-$(git rev-parse --git-common-dir 2>/dev/null)/info/leak-patterns}
if [ -s "$patterns" ]; then
  # The CHANGELOG.md lines from the first released section on are history.
  history=$(grep -n '^## ' CHANGELOG.md 2>/dev/null | grep -v '^[0-9]*:## Unreleased' | head -n 1 | cut -d: -f1 || true)
  hits=$( { git grep -nIwiE -f "$patterns" -- . || true; } |
          awk -F: -v h="${history:-0}" '!($1 == "CHANGELOG.md" && h > 0 && $2 >= h)' )
  if [ -n "$hits" ]; then
    echo "leak guard: text that matches a local leak pattern found (write the reason itself):"; echo "$hits"; status=1
  fi
fi

if [ -n "${1:-}" ]; then
  emails=$(git log --format='%ae%n%ce' "$1" | sort -u)
  for e in $emails; do
    case "$e" in
      *@users.noreply.github.com|noreply@github.com|noreply@anthropic.com) ;;
      # Only the masked address goes to the public log.
      *) echo "leak guard: a commit email is not a noreply address: ${e:0:1}***@${e#*@}"; status=1 ;;
    esac
  done
fi

[ "$status" = 0 ] && echo "leak guard: ok"
exit "$status"
