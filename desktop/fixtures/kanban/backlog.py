#!/usr/bin/env python3
"""The Kanban with a ticket source (invented): basic.json plus a Backlog
column, first, of GitHub issues imported into web-app, and the source in
each state. Run from anywhere; writes beside itself:

  backlog.json         source on, last synced 4 min before the board's now
  source-off.json      the daemon has sources, none set (Backlog empty)
  source-syncing.json  a sync under way
  source-failed.json   the last sync failed, with the daemon's hint

basic.json stays the older daemon's board: no Backlog, no source route.
"""
import copy, json, os

here = os.path.dirname(os.path.abspath(__file__))
base = json.load(open(os.path.join(here, "basic.json")))
NOW = base["daemon"]["tickets_boards"]["web-app"]["now"]
REPO = "acme/web-app"


def imported(n, title, issue, column="backlog", upstream="open", can_plan=True, marks=None, thread=None):
    can = {"delete": False, "delete_why": "Imported from GitHub", "edit": False, "reorder": column == "planned", "reorder_why": "" if column == "planned" else "Only Planned has an order", "send": False, "send_why": "Not a planned ticket", "validate": False}
    c = {
        "age": "2h", "bytes": 0, "can": can, "column": column, "created_ms": NOW - 7_200_000, "files": 0,
        "key": f"t:web-app:{n}", "marks": marks or [], "n": n, "num": f"T-{n}", "plan": None, "priority": "none",
        "sandbox": "web-app", "source": "tracker", "thread": thread, "thread_state": "running" if thread else None,
        "thread_tone": "blue" if thread else "neutral", "title": title, "title_long": title,
        "title_short": title if len(title) <= 64 else title[:63] + "…", "updated_ms": NOW - 3_600_000, "validated": False,
        "origin": {"tracker": "github", "key": f"{REPO}#{issue}", "url": f"https://github.com/{REPO}/issues/{issue}", "upstream": upstream},
    }
    if column == "backlog":
        c["can_plan"] = can_plan
    return c


def view(card, description):
    return {"card": card, "n": card["n"], "num": card["num"], "title": card["title"], "description": description,
            "priority": "none", "files": [], "files_label": "", "bytes": 0, "created_ms": card["created_ms"],
            "history": [{"at": card["created_ms"], "when": "2h", "text": f"Imported from {card['origin']['key']}", "tone": "neutral"}]}


backlog = [
    imported(20, "Dark mode for the settings page", 12),
    imported(21, "Show the upload size limit before the upload starts, not after it fails", 15),
    imported(22, "Old export button still visible on the reports page", 9, upstream="closed", can_plan=False,
             marks=[{"id": "closed-upstream", "label": "closed upstream", "tone": "neutral", "help": "Closed on GitHub"}]),
]
progress = imported(23, "Keyboard shortcut to search the reports", 7, column="progress", thread="report-search")
progress["marks"] = []

fx = copy.deepcopy(base)
fx["about"] = ("The Kanban with a ticket source (invented, written by backlog.py from basic.json): web-app imports the "
               "issues of acme/web-app labelled sbxw; Backlog holds two open issues and one closed at source, and one "
               "imported ticket is in progress. docs has the older daemon's board and no source.")
d = fx["daemon"]
for key, b in d["tickets_boards"].items():
    if not (key.startswith("web-app") or key.startswith("all")):
        continue
    col = {"id": "backlog", "title": "Backlog", "tone": "neutral", "help": "Issues from the ticket source, until you move one to Planned",
           "empty": "No issue to import.", "count": len(backlog), "cards": copy.deepcopy(backlog), "older": 0}
    b["columns"].insert(0, col)
    for c in b["columns"]:
        if c["id"] == "progress":
            c["cards"].insert(0, copy.deepcopy(progress))
            c["count"] += 1
    b["counts_line"] = f"{len(backlog)} in backlog · " + b["counts_line"]
for c in backlog + [progress]:
    d["tickets_views"][f"web-app/{c['n']}"] = view(c, "Imported from GitHub: the issue's words, as data.\n\nThe settings page is white at night. [[Note]] <button on=x> is text.")


def source(state, configured=True, last=NOW - 240_000, error=None, hint=None):
    return {"configured": configured, "tracker": "github" if configured else None, "repo": REPO if configured else None,
            "label": "sbxw" if configured else None, "auto_close": False,
            "detected": {"tracker": "github", "repo": REPO},
            "status": {"state": state, "last_ok_ms": last if configured else None, "error": error, "hint": hint}}


def write(name, src, empty_backlog=False):
    f = copy.deepcopy(fx)
    f["daemon"]["tickets_sources"] = {"web-app": src}
    if empty_backlog:
        for key, b in f["daemon"]["tickets_boards"].items():
            for c in b["columns"]:
                if c["id"] == "backlog":
                    c["cards"], c["count"] = [], 0
                    c["empty"] = "No ticket source: connect one to import issues."
                if c["id"] == "progress":
                    c["cards"] = [x for x in c["cards"] if "origin" not in x]
                    c["count"] = len(c["cards"])
    with open(os.path.join(here, name), "w") as o:
        json.dump(f, o, indent=1, ensure_ascii=False)
        o.write("\n")


write("backlog.json", source("ok"))
write("source-off.json", source("off", configured=False), empty_backlog=True)
write("source-syncing.json", source("syncing"))
write("source-failed.json", source("failed", error="401 Unauthorized from the GitHub MCP server",
                                    hint="Sign in to GitHub again in Setup, MCP."))
