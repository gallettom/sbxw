#!/usr/bin/env python3
"""The Kanban with plans merged through their CI branch (invented):
basic.json with the columns' words of today's daemon ("Merged to main"),
and cards carrying the four CI marks as the daemon sends them, each with
its label, tone and help line (crates/core/src/tickets.rs, `Mark`):

  In test   T-6  CI testing        its plan's CI branch is built or tested
            T-13 CI failed         a conflict or failed tests on it
            T-14 CI rebuild waits  main or a thread's branch moved since
  Ready     T-7, T-8 Merged to CI  the tests passed on the CI branch

Run from anywhere; writes ci.json beside itself. The words below are the
daemon's, copied: when tickets.rs changes them, change them here.
"""
import copy, json, os

here = os.path.dirname(os.path.abspath(__file__))
base = json.load(open(os.path.join(here, "basic.json")))
d = copy.deepcopy(base)

MARKS = {
    "ci_testing": ("CI testing", "blue", "sbxw is building its plan's CI branch from main with the threads' branches, or running the plan's tests on it"),
    "ci_stale": ("CI rebuild waits", "amber", "Main or a thread's branch moved since its plan's CI branch passed: sbxw rebuilds it and tests it again before Merge is offered"),
    "ci_failed": ("CI failed", "red", "Its plan's CI branch conflicted or failed its tests: nothing reached main, and the lead has the threads fix it"),
    "merged_ci": ("Merged to CI", "green", "Merged to CI: the tests passed on its plan's CI branch, and Merge fast-forwards main to exactly that tested commit"),
}
COLUMN_HELP = {
    "ready": "A merge is proposed, or Merged to CI, and waits for your click",
    "merged": "Merged to main: in the main checkout's branch",
}


def mark(mid):
    label, tone, help_ = MARKS[mid]
    return {"id": mid, "label": label, "tone": tone, "help": help_}


def with_mark(card, mid):
    # After the priority badge, as the daemon orders them.
    marks = [m for m in card["marks"] if m["id"] not in MARKS and m["id"] != "failed"]
    at = 1 if marks and marks[0]["id"] == "priority" else 0
    marks.insert(at, mark(mid))
    card["marks"] = marks
    return card


def new_card(proto, n, title, thread, mid, at):
    c = copy.deepcopy(proto)
    c.update({"key": f"t:web-app:{n}", "n": n, "num": f"T-{n}", "title": title, "title_long": title,
              "title_short": title, "thread": thread, "plan": "p-ci", "updated_ms": at, "created_ms": at - 3_600_000})
    c["can"]["reorder_why"] = f"T-{n} follows its thread: only tickets no thread works on move, inside Planned"
    return with_mark(c, mid)


for key, b in d["daemon"]["tickets_boards"].items():
    if not key.startswith("web-app") and not key.startswith("all"):
        for col in b["columns"]:
            if col["id"] in COLUMN_HELP:
                col["help"] = COLUMN_HELP[col["id"]]
            if col["id"] == "merged":
                col["title"] = "Merged to main"
        b["counts_line"] = b["counts_line"].replace(" merged", " merged to main")
        continue
    cols = {c["id"]: c for c in b["columns"]}
    test, ready = cols["test"], cols["ready"]
    proto = next(c for c in test["cards"] if c["key"] == "t:web-app:6")
    now = proto["updated_ms"]
    for c in test["cards"]:
        if c["key"] == "t:web-app:6":
            with_mark(c, "ci_testing")
    test["cards"] += [
        new_card(proto, 13, "Cache the avatars for an hour on the members page", "avatar-cache", "ci_failed", now - 600_000),
        new_card(proto, 14, "Show the invoice total with its currency", "invoice-total", "ci_stale", now - 1_200_000),
    ]
    for c in ready["cards"]:
        with_mark(c, "merged_ci")
    test["count"] = len(test["cards"])
    for col in b["columns"]:
        if col["id"] in COLUMN_HELP:
            col["help"] = COLUMN_HELP[col["id"]]
        if col["id"] == "merged":
            col["title"] = "Merged to main"
    b["counts_line"] = b["counts_line"].replace("1 in test", f"{test['count']} in test").replace(" merged", " merged to main")

# The panels of the CI cards: the ticket's view (its card as the board has it).
views = d["daemon"]["tickets_views"]
board = d["daemon"]["tickets_boards"]["web-app"]
cards = {c["key"]: c for col in board["columns"] for c in col["cards"]}
for n in (6, 7, 13, 14):
    c = cards[f"t:web-app:{n}"]
    v = copy.deepcopy(views["web-app/2"])
    v.update({"card": c, "n": n, "num": c["num"], "title": c["title"], "link": {"plan": c["plan"], "thread": c["thread"]},
              "description": "Invented: a ticket whose plan merges through its CI branch.", "updated_ms": c["updated_ms"]})
    views[f"web-app/{n}"] = v

d["about"] = ("The Kanban with plans merged through their CI branch (written by ci.py from basic.json): the columns' words of today's daemon "
              "(Merged to main), T-6 CI testing, T-13 CI failed and T-14 CI rebuild waits in In test, T-7 and T-8 Merged to CI in Ready to merge, "
              "each mark with the daemon's label and help line.")
json.dump(d, open(os.path.join(here, "ci.json"), "w"), indent=2, ensure_ascii=False)
print("ci.json")
