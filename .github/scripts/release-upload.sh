#!/usr/bin/env bash
# Attach files to the GitHub Release of a tag, and never replace one.
#
# Without a release for TAG, it creates the release with NOTES and the
# files. With a release, it checks every file first: a file whose name is
# not an asset yet gets uploaded. A file with the name of an asset must have
# the same sha256 as that asset, and then it stays as it is. A file with
# another sha256 stops the script before it uploads anything.
#
# Usage: release-upload.sh TAG NOTES FILE...
# It needs gh with GH_TOKEN, and sha256sum or shasum.
set -euo pipefail

if [ "$#" -lt 3 ]; then
  echo "release-upload: usage: release-upload.sh TAG NOTES FILE..." >&2
  exit 2
fi
tag=$1
notes=$2
shift 2

sum() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d ' ' -f 1
}

for f in "$@"; do
  [ -f "$f" ] || { echo "release-upload: $f is not a file" >&2; exit 1; }
  case "$(basename "$f")" in
    *[*?[]*) echo "release-upload: $f has a glob character in its name" >&2; exit 1 ;;
  esac
done

if ! gh release view "$tag" >/dev/null 2>&1; then
  gh release create "$tag" --verify-tag --title "paddock ${tag#v}" --notes-file "$notes" "$@"
  echo "release-upload: created $tag with $# files"
  exit 0
fi

assets=$(gh release view "$tag" --json assets --jq '.assets[].name')
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
new=()
bad=0
for f in "$@"; do
  name=$(basename "$f")
  if ! grep -qxF -- "$name" <<<"$assets"; then
    new+=("$f")
    continue
  fi
  mkdir "$tmp/$name.d"
  gh release download "$tag" --pattern "$name" --dir "$tmp/$name.d"
  have=$(sum "$tmp/$name.d/$name")
  want=$(sum "$f")
  if [ "$have" = "$want" ]; then
    echo "release-upload: $name is already on $tag with the same sha256"
  else
    echo "::error::$name is already on $tag with sha256 $have, the new file has $want. Delete the asset by hand if the new file is right."
    bad=1
  fi
done
[ "$bad" = 0 ] || exit 1

if [ "${#new[@]}" -gt 0 ]; then
  # No --clobber: gh refuses an asset that appeared since the check.
  gh release upload "$tag" "${new[@]}"
fi
echo "release-upload: uploaded ${#new[@]} new files to $tag"
