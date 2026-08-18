#!/usr/bin/env python3
"""Render a deterministic third-party NOTICE from `cargo about ... --format json`.

Why this exists instead of a cargo-about handlebars template:

cargo-about groups crates into license "sections" while harvesting license files,
and that grouping is *not* stable across machines. Crates that share a
byte-identical license text (e.g. two versions of the same crate, or the many
crates that ship the canonical MIT text) are sometimes emitted as one section and
sometimes split into several, depending on harvesting order on the host. That
made the checked-in NOTICE differ from the CI-generated one and broke the drift
check (see `make validate-notice`).

This renderer sidesteps that entirely: it re-groups crates by their exact
(license id, license text) and sorts everything, so the output depends only on the
license *content* — which is identical on every host — and never on cargo-about's
internal ordering. The result is byte-for-byte reproducible.
"""

import json
import sys

HEADER = """\
THIRD-PARTY SOFTWARE NOTICES AND INFORMATION
============================================

The Trident binaries statically link the third-party open-source Rust crates
listed below. Their required license notices are reproduced here in full.

This file is generated from Cargo.lock by cargo-about; do not edit it by hand.
Run `make update-notice` to regenerate it after changing dependencies.
"""

SEPARATOR = "-" * 80


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: render_notice.py <cargo-about-json>")

    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)

    # (license id, license name, license text) -> set of (crate name, version).
    # Grouping on the exact text merges any sections cargo-about happened to split.
    groups: dict[tuple[str, str, str], set[tuple[str, str]]] = {}
    for license in data.get("licenses", []):
        key = (license["id"], license["name"], license["text"].rstrip("\n"))
        used_by = groups.setdefault(key, set())
        for entry in license.get("used_by", []):
            crate = entry["crate"]
            used_by.add((crate["name"], crate["version"]))

    lines = [HEADER]
    # Ordering is fully determined by license content, never by input order.
    for (license_id, name, text) in sorted(groups):
        crates = sorted(groups[(license_id, name, text)])
        lines.append(SEPARATOR)
        lines.append(f"{name} ({license_id})")
        lines.append("")
        lines.append("Used by:")
        lines.extend(f"    - {crate} {version}" for crate, version in crates)
        lines.append("")
        lines.append(text)
        lines.append("")
    lines.append(SEPARATOR)

    sys.stdout.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
