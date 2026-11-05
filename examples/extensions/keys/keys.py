"""An example claudio extension (protocol v1): the `keys` tag lens draws a
key combination as keycaps, and the `stamp` action pastes a timestamp.

claudio writes one JSON request per line on stdin and reads one JSON answer
per line on stdout. See docs/extensions.md."""

import json
import sys
import time

SYMBOLS = {"ctrl": "⌃", "alt": "⌥", "shift": "⇧", "cmd": "⌘", "enter": "⏎", "esc": "⎋", "tab": "⇥"}


def keycaps(source):
    """`Ctrl+C` as `⌃C` (inline) and as a row of boxed keys (block)."""
    keys = [k.strip() for k in source.split("+") if k.strip()]
    names = [SYMBOLS.get(k.lower(), k.upper() if len(k) == 1 else k) for k in keys]
    inline = "".join(names)
    widths = [len(n) + 2 for n in names]
    top = " ".join("┌" + "─" * w + "┐" for w in widths)
    mid = " ".join("│ " + n + " │" for n in names)
    bottom = " ".join("└" + "─" * w + "┘" for w in widths)
    return {"inline": inline, "block": [top, mid, bottom]}


def handle(request):
    params = request.get("params", {})
    if request.get("method") == "render":
        return keycaps(params.get("source", ""))
    if request.get("method") == "action" and params.get("action") == "stamp":
        label = params.get("input") or "now"
        stamp = time.strftime("%Y-%m-%d %H:%M")
        return {"kind": "paste", "text": f"{label}: {stamp}"}
    raise ValueError("unknown request")


def main():
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        answer = {"id": request.get("id")}
        try:
            answer["result"] = handle(request)
        except Exception as e:  # any failure is that request's error
            answer["error"] = str(e)
        print(json.dumps(answer, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
