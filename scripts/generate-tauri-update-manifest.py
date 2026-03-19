#!/usr/bin/env python3
"""Generate the Tauri updater manifest JSON from build artifacts.

Usage: python scripts/generate-tauri-update-manifest.py <version>

Reads the .sig file from the NSIS build output and writes update-manifest.json
to the current directory.
"""

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO = "iwismer/rusty-timer"
NSIS_DIR = Path("target/x86_64-pc-windows-msvc/release/bundle/nsis")


def main() -> None:
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <version>", file=sys.stderr)
        sys.exit(1)

    version = sys.argv[1]

    # Find the .sig file
    sig_files = list(NSIS_DIR.glob("*.exe.sig"))
    if len(sig_files) != 1:
        print(
            f"Expected exactly one .sig file in {NSIS_DIR}, found {len(sig_files)}",
            file=sys.stderr,
        )
        sys.exit(1)

    signature = sig_files[0].read_text().strip()

    # Find the installer .exe (not the .sig)
    exe_files = [f for f in NSIS_DIR.glob("*.exe") if not f.name.endswith(".sig")]
    if len(exe_files) != 1:
        print(
            f"Expected exactly one .exe file in {NSIS_DIR}, found {len(exe_files)}",
            file=sys.stderr,
        )
        sys.exit(1)

    exe_name = exe_files[0].name

    manifest = {
        "version": f"v{version}",
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "notes": f"Receiver UI v{version}",
        "platforms": {
            "windows-x86_64": {
                "url": f"https://github.com/{REPO}/releases/download/receiver-ui-v{version}/{exe_name}",
                "signature": signature,
            }
        },
    }

    output = Path("update-manifest.json")
    output.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Wrote {output} for v{version}")


if __name__ == "__main__":
    main()
