#!/usr/bin/env python3
"""Stand-in 'agent' for the plumbing selftest: one chat request through the proxy.

Exercises container → internal network → proxy → upstream → records, without
a GPU or a real harness. Stdlib only.
"""

import argparse
import json
import sys
import urllib.error
import urllib.request


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--prompt", required=True)
    args = ap.parse_args()
    body = {
        "model": args.model,
        "messages": [
            {"role": "system", "content": "hcbench selftest"},
            {"role": "user", "content": args.prompt[:2000]},
        ],
        "stream": True,
    }
    req = urllib.request.Request(
        f"{args.base_url}/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            text = resp.read().decode("utf-8", errors="replace")
            status = resp.status
    except urllib.error.HTTPError as e:
        print(json.dumps({"ok": False, "status": e.code, "error": e.read().decode()[:500]}))
        return 1
    except urllib.error.URLError as e:
        print(json.dumps({"ok": False, "error": str(e)}))
        return 1
    print(json.dumps({"ok": status == 200, "status": status, "stream_bytes": len(text)}))
    return 0 if status == 200 else 1


if __name__ == "__main__":
    sys.exit(main())
