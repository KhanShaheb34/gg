#!/usr/bin/env python3
"""Splits each launch of a `scripts/perf.sh --logs DIR` series into phases.

    scripts/launch-phases.py DIR...

Each perf-<run>.log holds `name<TAB>ms since navigation start (-1: none)<TAB>unix ms`. The
navigation start's wall-clock time is a mark's unix ms minus its ms. Printed per launch (the
first of a series is left out of the medians, as perf.sh does), in ms:

  spawn->nav      `open` ran -> the page's navigation started (process, window, webview)
  shell           the login shell's run, and when it ended relative to navigation
  html            navigation -> the HTML's response end
  ->bundle        navigation -> the bundle's first line running (`bundle-start`): the
                  script's fetch and parse
  eval            bundle-start -> page-main: running the bundle's modules
  ->main          navigation -> the main script running (`page-main`)
  mount           page-main -> page-mounted
  state           state-asked -> state-answered
  open            open-asked -> open-answered (the launch repository's open_repo)
  ->rows-asked    open-answered -> the first page of rows asked for (tab and graph made)
  rows            rows-asked -> rows-answered (get_rows)
  paint           rows-answered -> first-rows (drawn)
  ->rows          open-answered -> first-rows (rows read and painted)
  nav->rows       navigation -> first-rows
"""
import statistics
import sys
from pathlib import Path


def read(path):
    marks = {}
    for line in path.read_text().splitlines():
        parts = line.split("\t")
        if len(parts) != 3:
            continue
        name, ms, epoch = parts
        marks.setdefault(name, (float(ms), float(epoch)))
    return marks


def phases(marks):
    if "first-rows" not in marks:
        return None
    ms, epoch = marks["first-rows"]
    nav = epoch - ms
    at = lambda n: marks[n][0] if n in marks else float("nan")
    out = {
        "spawn->nav": nav - marks["spawn"][1] if "spawn" in marks else float("nan"),
        "shell": (marks["login-shell-done"][1] - marks["login-shell-start"][1])
        if "login-shell-done" in marks
        else float("nan"),
        "shell-end@nav": marks["login-shell-done"][1] - nav if "login-shell-done" in marks else float("nan"),
        "html": at("nav-response-end"),
        "->bundle": at("bundle-start"),
        "eval": at("page-main") - at("bundle-start"),
        "->main": at("page-main"),
        "mount": at("page-mounted") - at("page-main"),
        "state": at("state-answered") - at("state-asked"),
        "open": at("open-answered") - at("open-asked"),
        "->rows-asked": at("rows-asked") - at("open-answered"),
        "rows": at("rows-answered") - at("rows-asked"),
        "paint": at("first-rows") - at("rows-answered"),
        "->rows": at("first-rows") - at("open-answered"),
        "nav->rows": ms,
    }
    return out


def main():
    for folder in sys.argv[1:]:
        logs = sorted(Path(folder).glob("perf-*.log"), key=lambda p: int(p.stem.split("-")[1]))
        rows = [(p.stem, phases(read(p))) for p in logs]
        rows = [(n, r) for n, r in rows if r is not None]
        if not rows:
            print(f"{folder}: no launches with first-rows")
            continue
        keys = list(rows[0][1])
        print(folder)
        print("  " + "run".ljust(9) + "".join(k.rjust(14) for k in keys))
        for name, r in rows:
            print("  " + name.ljust(9) + "".join(f"{r[k]:14.0f}" for k in keys))
        rest = [r for _, r in rows[1:]] or [r for _, r in rows]
        med = {k: statistics.median(r[k] for r in rest) for k in keys}
        print("  " + "median".ljust(9) + "".join(f"{med[k]:14.0f}" for k in keys))
        print()


if __name__ == "__main__":
    main()
