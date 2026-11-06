#!/usr/bin/env python3
"""Keep Localizable.xcstrings in step with the code.

  strings.py --check   fail on a missing key, an unused key, or a literal
                       that bypasses the catalog
  strings.py --write   write the catalog from the code (keeps translations)

A key is the English text. The script finds keys in these places:
  * the first string literal of the SwiftUI calls in App/ that take a
    LocalizedStringKey: Text("..."), Button("..."), Label("..."), ...
  * every UIText("...") in App/ and PaddockKit/Sources/
  * every literal on a line that ends with "// localized"

The lint rules: no Text(verbatim: "...") with a literal, and no
interpolation inside a localized literal (use UIText with %@ instead).
AppTests/LocalizationTests.swift runs the same check on the Mac.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CATALOG = os.path.join(ROOT, "App", "Resources", "Localizable.xcstrings")
LITERAL = r'"((?:[^"\\]|\\.)*)"'
CALLS = ["Text", "Button", "Label", "Toggle", "Picker", "Section", "LabeledContent", "TextField",
         "Menu", "TableColumn", "ContentUnavailableView", "Window", "CommandMenu", "Link",
         "String(localized:"]
CALL_RE = re.compile(r'(?<![\w.])(?:' + "|".join(re.escape(c) if c.endswith(":") else re.escape(c) + r'\(' for c in CALLS)
                     + r')\s*' + LITERAL)
UITEXT_RE = re.compile(r'UIText\(\s*(?:key:\s*)?' + LITERAL)
ANY_RE = re.compile(LITERAL)
VERBATIM_RE = re.compile(r'Text\(verbatim:\s*"')


def unescape(text):
    return text.replace('\\"', '"').replace("\\\\", "\\").replace("\\n", "\n")


def swift_files(*dirs):
    for d in dirs:
        for base, _, files in os.walk(os.path.join(ROOT, d)):
            for name in sorted(files):
                if name.endswith(".swift"):
                    yield os.path.join(base, name)


def extract():
    keys = {}
    problems = []
    for path in swift_files("App", os.path.join("PaddockKit", "Sources")):
        in_app = os.sep + "App" + os.sep in path
        rel = os.path.relpath(path, ROOT)
        with open(path, encoding="utf-8") as f:
            for number, line in enumerate(f, 1):
                code = line.split("//")[0] if not line.rstrip().endswith("// localized") else line
                found = []
                for m in UITEXT_RE.finditer(code):
                    found.append(m.group(1))
                if in_app:
                    for m in CALL_RE.finditer(code):
                        found.append(m.group(1))
                    if line.rstrip().endswith("// localized"):
                        found += [m.group(1) for m in ANY_RE.finditer(line.split("// localized")[0])]
                    if VERBATIM_RE.search(code):
                        problems.append(f"{rel}:{number}: Text(verbatim:) with a literal; put the text in the catalog")
                for raw in found:
                    if "\\(" in raw:
                        problems.append(f"{rel}:{number}: interpolation in a localized literal; use UIText with %@")
                        continue
                    keys.setdefault(unescape(raw), f"{rel}:{number}")
    return keys, problems


def load():
    if not os.path.exists(CATALOG):
        return {"sourceLanguage": "en", "strings": {}, "version": "1.0"}
    with open(CATALOG, encoding="utf-8") as f:
        return json.load(f)


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "--check"
    keys, problems = extract()
    catalog = load()
    if mode == "--write":
        old = catalog.get("strings", {})
        strings = {}
        for key in sorted(keys):
            entry = old.get(key) or {}
            entry["extractionState"] = "manual"
            loc = entry.setdefault("localizations", {})
            loc["en"] = {"stringUnit": {"state": "translated", "value": key}}
            strings[key] = entry
        catalog["strings"] = strings
        os.makedirs(os.path.dirname(CATALOG), exist_ok=True)
        with open(CATALOG, "w", encoding="utf-8") as f:
            json.dump(catalog, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"strings: wrote {len(strings)} keys")
    have = set(catalog.get("strings", {}))
    missing = sorted(set(keys) - have)
    unused = sorted(have - set(keys))
    for key in missing:
        problems.append(f"{keys[key]}: missing in the catalog: {key!r}")
    for key in unused:
        problems.append(f"unused key in the catalog: {key!r}")
    for p in problems:
        print("strings: " + p)
    if problems:
        sys.exit(1)
    print(f"strings: ok, {len(have)} keys")


if __name__ == "__main__":
    main()
