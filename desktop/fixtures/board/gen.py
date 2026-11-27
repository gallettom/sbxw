# Generates desktop/fixtures/board/basic.json, and ci.json with its views
# (ci-*.json) beside it: invented, neutral content. Run: python3 gen.py basic.json
import json, hashlib, sys
out = sys.argv[1]
DAY = 86_400_000
T0 = 1794_902_400_000  # 2026-11-17 08:00 UTC
seq = [0]
def h(s): return hashlib.sha1(s.encode()).hexdigest()
GROUP = {"direct_message":"messages","team_note":"notes","board_decision":"notes","board_warning":"notes","board_handoff":"notes","conflict":"notes","board_question":"questions",
 "relay_question":"questions","relay_routed":"questions","relay_drafted":"questions","relay_answer":"questions","user_question":"questions",
 "user_answer":"questions","screenshot_asked":"questions","screenshot_given":"questions","relay_refused":"questions","commit":"commits","claim":"claims","release":"claims"}
def ev(at, kind, frm, text=None, to=None, data=None, prefix=None):
    seq[0] += 1
    # The daemon's ids: a board entry is b<id>, a log entry l<seq>; a standing note keeps its event's id.
    prefix = prefix or ("b" if kind in ("board_decision", "board_warning", "board_question", "board_handoff") else "l")
    return {"id": f"{prefix}{seq[0]}", "at": at, "kind": kind, "group": GROUP.get(kind, "status"), "from": frm, "to": to, "text": text, "data": data or {}}
def commit(at, thread, subject, files, add, rem, merged=False):
    hh = h(thread + subject)
    return {"id": "c" + hh, "at": at, "kind": "commit", "group": "commits", "from": thread, "to": None, "text": subject,
            "data": {"hash": hh, "short": hh[:7], "author": "Agent", "branch": "sbxw/" + thread, "merged": merged, "files": files, "added": add, "removed": rem, "repo": "", "thread": thread}}

# ── A running team on web-app ──
R = T0 + DAY + 9*3600_000  # 2026-11-18 17:00
threads = [
  {"name": "api-routes", "goal": "Add the /v2/orders routes to the API. Keep the v1 routes untouched; write handler tests for each new route and the error shapes.", "scope": "api/src/routes/v2/", "model": "claude-sonnet-5-5"},
  {"name": "web-page", "goal": "Build the orders page on the v2 routes: the list, the filters and the empty state. Reuse the table component.", "scope": "web/src/pages/orders/", "model": None},
  {"name": "docs-update", "goal": "Document the v2 routes in the API guide, with one example request and answer each.", "scope": "docs/api/", "model": None},
]
m = lambda mins: R + mins*60_000
run = []
run.append(ev(m(0), "plan_proposed", "lead", "Orders v2: the API routes, the web page that uses them, and their docs, in three threads that do not touch the same files.", "user",
  {"variant": "split", "justification": "Three independent folders: each thread on its own branch, merged at the end.", "threads": threads}))
run.append(ev(m(4), "plan_approved", "user", None, "lead", {"variant": "split", "threads": [t["name"] for t in threads]}))
run.append(ev(m(5), "thread_failed", "sbxw", "the worktree folder .sbxw-worktrees/docs-update already exists and is not a git worktree", None, {"thread": "docs-update", "state": "failed"}))
run.append(ev(m(7), "thread_status", "api-routes", "Reading the v1 routes and their tests first.", "lead", {"thread": "api-routes", "state": "working"}))
run.append(ev(m(8), "thread_status", "web-page", "Starting with the list and its empty state.", "lead", {"thread": "web-page", "state": "working"}))
run.append(ev(m(11), "thread_status", "docs-update", "Started after the folder was cleared. Drafting the guide's outline.", "lead", {"thread": "docs-update", "state": "working"}))
for i, (s, f, a, r) in enumerate([("v2 orders: route skeleton and types", 3, 84, 2), ("v2 orders: list with cursor pagination", 2, 61, 9), ("v2 orders: error shapes", 2, 40, 4), ("v2 orders: handler tests", 1, 132, 0)]):
    run.append(commit((m(20 + i*9))//1000*1000, "api-routes", s, f, a, r))
run.append(ev(m(58), "board_warning", "api-routes", "The v2 order total is in cents, not in euros. [[Warning: breaking]] The page must divide by 100 before it formats the amount.", None, {"files": ["api/src/routes/v2/orders.rs"]}))
run.append(ev(m(61), "board_decision", "lead", "Keep the v1 routes until the page ships; no redirect from v1 to v2 in this plan.", None, {"files": []}))
run.append(ev(m(64), "user_question", "web-page", None, "user", {"relay": "r-11", "relay_created": m(64), "relay_kind": "ask", "state": "pending", "shots": 0, "live": True,
  "questions": [{"id": "Q1", "title": "Empty state", "text": "When a filter matches no order, show the empty state's illustration or only the sentence?", "recommendation": "Only the sentence: the page stays light.", "answer": None},
                {"id": "Q2", "title": "Page size", "text": "How many orders a page: 25 or 50?", "recommendation": "25, as the v1 list.", "answer": None}]}))
run.append(ev(m(70), "user_answer", "user", None, "web-page", {"relay": "r-11", "relay_created": m(64), "relay_kind": "ask", "state": "approved", "shots": 0, "live": True,
  "questions": [{"id": "Q1", "title": "Empty state", "text": "When a filter matches no order, show the empty state's illustration or only the sentence?", "recommendation": "Only the sentence: the page stays light.", "answer": "Only the sentence."},
                {"id": "Q2", "title": "Page size", "text": "How many orders a page: 25 or 50?", "recommendation": "25, as the v1 list.", "answer": "50, people scroll anyway."}]}))
run.append(ev(m(65), "direct_message", "lead", "Before you open the pull request, please read the handlers once more as a stranger would. Each route should check its input in one place, return the same error shape as the v1 routes, and say in its test name what it refuses. If a handler needs more than about forty lines, split it. The page depends on the error codes, so do not rename them without telling web-page first; I would rather keep a slightly awkward name than break the filters the day before the review.", "api-routes"))
run.append(ev(m(66), "direct_message", "lead", "Use the table component's own pagination; do not write a second one.", "web-page"))
run.append(ev(m(67), "direct_message", "lead", "And keep the filters in the URL, so a reload keeps them.", "web-page"))
run.append(ev(m(68), "direct_message", "web-page", "Understood. I will pass the page size through.", "lead"))
run.append(ev(m(71), "team_note", "lead", "The table component takes a page size now: pass 50, as you were told.", "web-page", {"kind": "info"}))
run.append(ev(m(72), "relay_question", "docs-update", "Where is the style guide for the API docs, and does it want one example per route?", None, {"relay": "r-12", "relay_created": m(72), "relay_kind": "question", "state": "pending", "live": True}))
run.append(ev(m(75), "relay_routed", "user", None, "docs", {"relay": "r-12", "relay_created": m(72), "relay_kind": "question", "state": "routed", "to_sandbox": "docs"}))
run.append(ev(m(79), "relay_drafted", "docs", "docs/STYLE.md, section API. One request and one answer per route, trimmed to the fields that matter.", "user", {"relay": "r-12", "relay_created": m(72), "relay_kind": "question", "state": "answered", "live": True}))
run.append(ev(m(81), "relay_answer", "user", "docs/STYLE.md, section API. One request and one answer per route, trimmed to the fields that matter.", "docs-update", {"relay": "r-12", "relay_created": m(72), "relay_kind": "question", "state": "approved", "live": True}))
for i, (s, f, a, r) in enumerate([("orders page: list and empty state", 4, 120, 6), ("orders page: amounts in euros from cents", 2, 18, 5)]):
    run.append(commit((m(84 + i*6))//1000*1000, "web-page", s, f, a, r))
run.append(ev(m(92), "thread_status", "web-page", "Waiting on a screenshot of the filters at mobile width before I lay them out.", "lead", {"thread": "web-page", "state": "blocked"}))
run.append(ev(m(93), "screenshot_asked", "web-page", "The orders page filters at 375 px wide, after the last change.", "user", {"relay": "r-13", "relay_created": m(93), "relay_kind": "screenshot", "state": "pending", "shots": 0, "live": True}))
run.append(ev(m(96), "thread_status", "api-routes", "All routes in with their tests; the totals are in cents as noted on the board.", "lead", {"thread": "api-routes", "state": "done"}))
run.append(commit((m(98))//1000*1000, "docs-update", "api guide: v2 orders routes", 1, 74, 0))
# Bare changes of state (no words): one quiet line, folded with a count.
run.append(ev(m(99), "thread_status", "docs-update", None, "lead", {"thread": "docs-update", "state": "blocked"}))
run.append(ev(m(100), "thread_status", "docs-update", "", "lead", {"thread": "docs-update", "state": "working"}))
run.append(ev(m(101), "thread_status", "docs-update", "  ", "lead", {"thread": "docs-update", "state": "working"}))
run.append(ev(m(102), "team_note", "lead", "Everyone: the merge goes in the order api-routes, web-page, docs-update. Say so if that breaks something of yours.", "team", {"kind": "decision"}))
run.append(ev(m(104), "thread_status", "docs-update", "The guide covers the v2 orders routes.\n\n1. One section per route, with a request and its answer.\n2. The totals are in cents: the guide says so next to `total_cents` and in the example.\n3. The pagination's `cursor` field is described once and linked from each list route.\n\nNot done: the changelog entry, which waits for the merge. Run `make docs` to see the pages.", "lead", {"thread": "docs-update", "state": "done"}))
# The plan's test queue: finished jobs (from the asker) and a thread sent to the queue.
# Their ids from a count of their own (l1001...), so every other id stays as it was.
qseq = [1000]
def qev(at, kind, frm, data, text=None):
    qseq[0] += 1
    return {"id": f"l{qseq[0]}", "at": at, "kind": kind, "group": "status", "from": frm, "to": None, "text": text, "data": data}
def job(at, asker, outcome, short, ms, reused=False, retried=False, reuse=None):
    return qev(at, "test_job", asker, {"asker": asker, "outcome": outcome, "reused": reused, "reuse": reuse, "retried": retried, "short": short, "duration_ms": ms, "job": f"j{qseq[0]+1}"}, f"tests {outcome}")
run.append(job(m(106), "api-routes", "passed", h("api-routes" + "v2 orders: handler tests")[:7], 184_000))
run.append(job(m(108), "web-page", "failed", h("web-page" + "orders page: amounts in euros from cents")[:7], 400_000, retried=True))
run.append(qev(m(109), "test_refused", "web-page", {"thread": "web-page"}))
run.append(job(m(110), "lead", "passed", h("api-routes" + "v2 orders: handler tests")[:7], 184_000, reused=True, reuse="stored"))
run.append(job(m(111), "docs-update", "timed_out", h("docs-update" + "api guide: v2 orders routes")[:7], 1_200_000))
run.append(job(m(112), "lead", "error", h("docs-update" + "api guide: v2 orders routes")[:7], 2_000))
run.reverse()
run_team = {"id": "p-run", "sandbox": "web-app", "summary": "Orders v2: the API routes, the web page that uses them, and their docs", "state": "running", "variant": "split",
  "lead": "orders-planner", "threads": [{"name": "api-routes", "state": "done", "model": "claude-sonnet-5-5"}, {"name": "web-page", "state": "blocked"}, {"name": "docs-update", "state": "done"}],
  "created_ms": m(0), "updated_ms": m(104), "last_ms": m(104)}
cl = [e for e in run if e["kind"] == "commit"]
def totals(cs):
    t = {}
    for c in cs:
        th = c["data"]["thread"]; x = t.setdefault(th, {"thread": th, "commits": 0, "files": 0, "added": 0, "removed": 0})
        x["commits"] += 1; x["files"] += c["data"]["files"]; x["added"] += c["data"]["added"]; x["removed"] += c["data"]["removed"]
    return list(t.values())
def clist(cs): return [{"hash": c["data"]["hash"], "short": c["data"]["short"], "subject": c["text"], "author": "Agent", "at": c["at"]//1000, "thread": c["data"]["thread"], "branch": c["data"]["branch"], "merged": c["data"]["merged"], "files": c["data"]["files"], "added": c["data"]["added"], "removed": c["data"]["removed"], "repo": ""} for c in cs]
def standing_of(evs):
    # As the daemon's team::standing_notes: board decisions and warnings, and team notes that say decision or warning; newest first.
    out = [{"id": e["id"], "at": e["at"], "kind": "decision" if e["kind"] == "board_decision" else "warning", "by": e["from"], "text": e["text"], "files": e["data"]["files"], "guess": False} for e in evs if e["kind"] in ("board_decision", "board_warning")]
    out += [{"id": e["id"], "at": e["at"], "kind": e["data"]["kind"], "by": e["from"], "to": e["to"], "text": e["text"], "files": [], "guess": False, "team_note": True} for e in evs if e["kind"] == "team_note" and e["data"].get("kind") in ("decision", "warning")]
    return sorted(out, key=lambda n: -n["at"])
standing = standing_of(run)

# ── A merged team on docs ──
M = T0 + 2*3600_000
mm = lambda mins: M + mins*60_000
mt = [{"name": "guide-intro", "goal": "Rewrite the guide's introduction for first-time readers.", "scope": "docs/guide/intro.md", "model": None},
      {"name": "guide-search", "goal": "Add a search box to the guide's site.", "scope": "site/search/", "model": None}]
mer = []
mer.append(ev(mm(0), "plan_proposed", "lead", "Guide refresh: a clearer introduction and a search box.", "user", {"variant": "split", "justification": "Two files that never meet.", "threads": mt}))
mer.append(ev(mm(2), "plan_approved", "user", None, "lead", {"variant": "split", "threads": ["guide-intro", "guide-search"]}))
for i, (th, s, f, a, r) in enumerate([("guide-intro", "intro: shorter first paragraph", 1, 12, 30), ("guide-intro", "intro: a three-step start", 1, 40, 8), ("guide-search", "search: index at build time", 3, 96, 0), ("guide-search", "search: the box and its results", 2, 70, 4)]):
    mer.append(commit((mm(10 + i*8))//1000*1000, th, s, f, a, r, merged=True))
mer.append(ev(mm(45), "board_decision", "guide-search", "The search index is built at deploy, never in the browser.", None, {"files": []}))
mer.append(ev(mm(50), "thread_status", "guide-intro", "Done: the introduction reads in one minute.", "lead", {"thread": "guide-intro", "state": "done"}))
mer.append(ev(mm(52), "thread_status", "guide-search", "Done, with a test of the index.", "lead", {"thread": "guide-search", "state": "done"}))
mer.append(ev(mm(55), "merge_proposed", "lead", "Both reviewed. The intro first: the search page links to it.", "user", {"order": ["guide-intro", "guide-search"], "distillate": 1}))
mer.append(ev(mm(60), "merge_started", "user", None, None, {"order": ["guide-intro", "guide-search"], "base": "main"}))
mer.append(ev(mm(60)+2000, "merge_result", "sbxw", None, None, {"thread": "guide-intro", "ok": True, "commit": h("m1")}))
mer.append(ev(mm(60)+4000, "merge_result", "sbxw", None, None, {"thread": "guide-search", "ok": True, "commit": h("m2")}))
mer.append(ev(mm(60)+5000, "merge_finished", "sbxw", None, None, {"state": "merged", "order": ["guide-intro", "guide-search"]}))
mer.sort(key=lambda e: e["at"]); mer.reverse()
mer_team = {"id": "p-old", "sandbox": "docs", "summary": "Guide refresh: a clearer introduction and a search box", "state": "merged", "variant": "split", "lead": None,
  "threads": [{"name": "guide-intro", "state": "done"}, {"name": "guide-search", "state": "done"}], "created_ms": mm(0), "updated_ms": mm(60)+5000, "last_ms": mm(60)+5000}
mcl = [e for e in mer if e["kind"] == "commit"]

# ── A proposed team on api ──
P = T0 + 2*DAY + 30*60_000
pro = [ev(P, "plan_proposed", "lead", "Rate limits: one thread for the middleware and its tests.", "user", {"variant": "split", "justification": "One thread.", "threads": [{"name": "rate-limit", "goal": "Add a per-token rate limit to the API.", "scope": "api/src/middleware/", "model": None}]})]
pro_team = {"id": "p-new", "sandbox": "api", "summary": "Rate limits: one thread for the middleware and its tests", "state": "proposed", "variant": "split", "lead": "API lead",
  "threads": [{"name": "rate-limit", "state": "starting"}], "created_ms": P, "updated_ms": P, "last_ms": P}

# ── A team whose words are long: unbroken paths, hashes, 200 characters without a space ──
L = T0 + 2*DAY + 3*3600_000
ll = lambda mins: L + mins*60_000
LT = [{"name": "storage-migration", "goal": "Move the uploads to the new bucket layout.", "scope": "storage/", "model": None}]
LONG_PATH = "storage/src/" + "/".join(["nested_folder_level_%d" % i for i in range(1, 7)]) + "/migrate_uploads_to_the_new_bucket_layout.rs"
LONG_WORD = "x" * 200
long_ev = [
  ev(ll(0), "plan_proposed", "lead", "Storage: move the uploads to the new bucket layout.", "user", {"variant": "split", "justification": "One thread.", "threads": LT}),
  ev(ll(1), "plan_approved", "user", None, "lead", {"variant": "split", "threads": ["storage-migration"]}),
  ev(ll(5), "thread_status", "storage-migration", "Started in " + LONG_PATH + " with the checksum " + h("long") + h("long2") + ".", "lead", {"thread": "storage-migration", "state": "working"}),
  ev(ll(9), "team_note", "lead", "A token without a space, as a log line can hold: " + LONG_WORD, "team", {"kind": "info"}),
  ev(ll(10), "direct_message", "lead", "Paste the failing object key here: " + LONG_WORD[:120], "storage-migration"),
  ev(ll(12), "board_warning", "storage-migration", "Do not touch " + LONG_PATH + " before the copy ends.", None, {"files": [LONG_PATH]}),
  ev(ll(15), "thread_status", "storage-migration", "The copy failed at " + LONG_PATH + ": checksum mismatch " + h("x") + h("y") + h("z"), "lead", {"thread": "storage-migration", "state": "failed"}),
]
long_ev.reverse()
long_team = {"id": "p-long", "sandbox": "docs", "summary": "Storage: move the uploads to the new bucket layout, with a checksum of every object and a dry run before the copy", "state": "running", "variant": "split",
  "lead": "a-coordinating-session-with-a-rather-long-name", "threads": [{"name": "storage-migration", "state": "failed"}], "created_ms": ll(0), "updated_ms": ll(15), "last_ms": ll(15)}

details = {}
for c in cl + mcl:
    d = c["data"]
    details[d["hash"]] = {"hash": d["hash"], "short": d["short"], "author": "Agent", "at": c["at"]//1000, "subject": c["text"], "body": "", "repo": "",
      "files": [{"path": f"src/part_{i}.rs", "added": max(1, d["added"]//d["files"]), "removed": d["removed"]//d["files"], "binary": False,
                 "patch": f"@@ -1,3 +1,4 @@\n fn part_{i}() {{\n-    old_value()\n+    new_value()\n+    // checked\n }}\n"} for i in range(d["files"])], "files_more": 0, "truncated": False}

sandboxes = [
  {"name": "web-app", "agent": "claude", "status": "running", "workspace": "/work/web", "created_at": "2026-11-10T09:12:00Z", "approvals": 0, "kits_waiting": 0},
  {"name": "docs", "agent": "claude", "status": "running", "workspace": "/work/docs", "created_at": "2026-11-10T09:12:00Z", "approvals": 0, "kits_waiting": 0},
  {"name": "api", "agent": "claude", "status": "running", "workspace": "/work/api", "created_at": "2026-11-10T09:12:00Z", "approvals": 0, "kits_waiting": 0},
]
plans = [
  {"id": "p-run", "sandbox": "web-app", "summary": run_team["summary"], "state": "approved", "closed": False, "threads": [], "created_ms": m(0), "updated_ms": m(98)},
  {"id": "p-old", "sandbox": "docs", "summary": mer_team["summary"], "state": "approved", "closed": False, "merge": {"state": "merged"}, "threads": [], "created_ms": mm(0), "updated_ms": mm(60)},
]
# p-new waits for approval: not in `plans`, or the app would open its dialog over the shots.
fx = {
 "about": "The Board (feat/board/): three teams, invented (generated by gen.py beside it). p-run is at work on web-app (a warning and a decision on the board, a round of questions answered, a question to the docs sandbox answered, a screenshot still waiting, two threads committing, one start that failed, the test queue's jobs of every outcome and a thread sent to the queue; its numbers: approved, not merged, tests run and reused); p-old on docs is merged (approved to merged, no test command); p-new on api waits for approval; p-long on docs has long unbroken words (a path, hashes, 200 characters without a space) and a failed report, and no numbers (an older daemon). The stand-in answers /api/teams from board_teams and /api/teams/:id/timeline from board_timelines; commit details through the Commits feature's stand-in (commit_details).",
 "app": {"selected": "web-app", "panes": 2, "split": "columns", "open": ["web-app", "docs"], "focused": 1},
 "daemon": {
  "sandboxes": sandboxes, "plans": plans,
  "board_teams": [run_team, pro_team, mer_team, long_team],
  "board_timelines": {
    "p-run": {"team": run_team, "events": run, "standing": standing, "commits": {"available": True, "list": clist(sorted(cl, key=lambda c: -c["at"])), "totals": totals(cl), "merged": False},
      "numbers": {"approved_ms": m(4), "merged_ms": None, "tests": {"run": 4, "reused": 1, "refused": 1}, "asked": 3}},
    "p-old": {"team": mer_team, "events": mer, "standing": standing_of(mer), "commits": {"available": True, "list": clist(sorted(mcl, key=lambda c: -c["at"])), "totals": totals(mcl), "merged": True},
      "numbers": {"approved_ms": mm(2), "merged_ms": mm(60) + 5000, "tests": None, "asked": 0}},
    "p-long": {"team": long_team, "events": long_ev, "standing": standing_of(long_ev), "commits": {"available": True, "list": [], "totals": [], "merged": False}},
    "p-new": {"team": pro_team, "events": pro, "standing": [], "commits": {"available": True, "list": [], "totals": [], "merged": False}},
  },
  "commits": {"web-app": [], "docs": []},
  "commit_details": details,
 }}
json.dump(fx, open(out, "w"), indent=1)
print("ok", len(run), len(mer))

# ── ci.json and its views: six teams merged through their CI branch ──
# One team per CI state (the daemon's merge.state and merge_status, its words
# as crates/core/src/plans/ci.rs writes them), and the timeline's CI lines:
# the ci_step events, the battery sbxw asked on the CI branch (test_job,
# asker sbxw), a rebuild because main moved, a merge finished to main.
import os
here = os.path.dirname(os.path.abspath(out))
C = T0 + 3*DAY
def ci_team(pid, sb, summary, names, state, at, last):
    return {"id": pid, "sandbox": sb, "summary": summary, "state": state, "variant": "split", "lead": None,
            "threads": [{"name": n, "state": "done"} for n in names], "created_ms": at, "updated_ms": last, "last_ms": last}
def story(pid, names, at, extra):
    br = "sbxw/ci-" + pid
    t = lambda mins: at + mins*60_000
    e = [ev(t(0), "plan_proposed", "lead", "Two threads, merged through the CI branch.", "user", {"variant": "split", "justification": "Two folders.", "threads": [{"name": n, "goal": "Its part.", "scope": n + "/", "model": None} for n in names]}),
         ev(t(1), "plan_approved", "user", None, "lead", {"variant": "split", "threads": names})]
    for i, n in enumerate(names):
        e.append(ev(t(20 + i), "thread_status", n, "Done, with its tests.", "lead", {"thread": n, "state": "done"}))
    e.append(ev(t(30), "merge_proposed", "lead", "Both reviewed; " + names[0] + " first.", "user", {"order": names, "distillate": 1}))
    for mins, kind, frm, text, data in extra(br, t):
        e.append(ev(t(mins) if isinstance(mins, int) else mins, kind, frm, text, None, data))
    e.sort(key=lambda x: x["at"]); e.reverse()
    return e
def built(br, tip, base, status="Testing the CI branch"):
    return ("ci_step", "sbxw", f"{br} built at {tip[:7]} on {base[:7]}", {"step": "built", "run": 1, "tip": tip, "base": base, "state": "ci_testing", "status": status})
def job(short, outcome, retried=False, reuse=None, ms=312_000):
    return ("test_job", "sbxw", f"Tests {outcome} at {short}", {"job": "j-" + short, "asker": "sbxw", "thread": "sbxw", "outcome": outcome, "reused": reuse is not None, "reuse": reuse, "retried": retried, "short": short, "duration_ms": ms})
def passed(br, tip):
    return ("ci_step", "sbxw", f"Merged to CI: tests passed on {br} at {tip[:7]}", {"step": "passed", "run": 1, "job": "j", "outcome": "passed", "reused": False, "state": "merged_ci", "status": "Merged to CI"})

teams_ci, timelines_ci, plans_ci = [], {}, []
def add(pid, sb, summary, names, at, merge_state, ci, status, extra, numbers, team_state="done"):
    e = story(pid, names, at, extra)
    last = e[0]["at"]
    team = ci_team(pid, sb, summary, names, team_state, at, last)
    teams_ci.append(team)
    timelines_ci[pid] = {"team": team, "events": e, "standing": [], "commits": {"available": True, "list": [], "totals": [], "merged": merge_state == "merged"}, "numbers": numbers}
    plans_ci.append({"id": pid, "sandbox": sb, "summary": summary, "state": "approved", "closed": False, "threads": [],
                     "merge": {"state": merge_state, "order": names, "ci": ci}, "merge_status": status, "created_ms": at, "updated_ms": last})

def nums(at, run, reused, merged=None): return {"approved_ms": at + 60_000, "merged_ms": merged, "tests": {"run": run, "reused": reused, "refused": 0}, "asked": 0}

# Testing, after a rebuild because main moved (web-app: the view opened first).
A = C; br = "sbxw/ci-p-ci-test"; tip1, base1, base2, tip2 = h("t1"), h("b1"), h("b2"), h("t2")
add("p-ci-test", "web-app", "Checkout: the cart page and its API client, merged through CI", ["cart-page", "api-client"], A, "ci_testing",
    {"branch": br, "run": 2, "builds": 2, "base_branch": "main", "base": base2, "tip": tip2, "rebuilt_because": f"main moved from {base1[:7]} to {base2[:7]}"},
    {"word": "Testing the CI branch", "detail": f"The plan's tests run on {br} at {tip2[:7]}", "can_merge": False, "why_not": f"main moved from {base1[:7]} to {base2[:7]}: the rebuilt {br} is being tested; Merge comes back when it passes", "ci": True},
    lambda br, t: [(31, *built(br, tip1, base1)), (37, *job(tip1[:7], "passed", retried=True, ms=402_000)), (38, *passed(br, tip1)),
                   (52, "ci_step", "sbxw", "main moved: the CI branch waits to be rebuilt", {"step": "stale", "run": 1, "main": base2, "because": None, "state": "ci_stale", "status": "Main moved: CI rebuild waits"}),
                   (53, "ci_step", "sbxw", f"main moved from {base1[:7]} to {base2[:7]}: rebuilding {br}", {"step": "rebuild", "run": 2, "main": base2, "state": "ci_building", "status": "Building the CI branch"}),
                   (54, *built(br, tip2, base2))],
    nums(A, 2, 0))
# Building (docs).
B = C + 2*3600_000; br = "sbxw/ci-p-ci-build"
add("p-ci-build", "docs", "Guide: the glossary and its search entries", ["glossary", "search-terms"], B, "ci_building",
    {"branch": br, "run": 1, "builds": 1},
    {"word": "Building the CI branch", "detail": f"Merging the threads into {br}", "can_merge": False, "why_not": f"sbxw is building {br} from main with the threads' branches; the tests run next, then Merge is offered", "ci": True},
    lambda br, t: [], nums(B, 0, 0))
# Merged to CI, the battery's result reused (api).
D = C + 3*3600_000; br = "sbxw/ci-p-ci-ok"; tip = h("ok"); base = h("okb")
add("p-ci-ok", "api", "Rate limits: the middleware and its settings", ["rate-limit", "limit-settings"], D, "merged_ci",
    {"branch": br, "run": 1, "builds": 1, "base_branch": "main", "base": base, "tip": tip, "tests": {"job": "j-ok", "outcome": "passed", "reused": True, "commit": tip, "attempts": []}},
    {"word": "Merged to CI", "detail": f"Tests passed on {br} at {tip[:7]}: Merge fast-forwards main to exactly that commit", "can_merge": True, "ci": True},
    lambda br, t: [(31, *built(br, tip, base)), (32, *job(tip[:7], "passed", reuse="stored", ms=280_000)), (32, *passed(br, tip))], nums(D, 0, 1))
# CI failed: a conflict while building (ops).
E = C + 4*3600_000; br = "sbxw/ci-p-ci-fail"
add("p-ci-fail", "ops", "Backups: the nightly job and its schema change", ["db-schema", "backup-job"], E, "ci_failed",
    {"branch": br, "run": 1, "builds": 1, "base_branch": "main", "base": h("fb"), "failure": {"kind": "conflict", "thread": "backup-job", "files": ["migrations/0042_backups.sql"], "text": "backup-job conflicts"}},
    {"word": "CI failed", "detail": "sbxw/backup-job conflicts in migrations/0042_backups.sql: nothing reached main, no tests ran", "can_merge": False, "why_not": "the lead has the threads fix it, then proposes the merge again, which rebuilds and retests the CI branch", "ci": True},
    lambda br, t: [(31, "ci_step", "sbxw", "sbxw/backup-job conflicts in migrations/0042_backups.sql", {"step": "failed", "run": 1, "kind": "conflict", "thread": "backup-job", "files": ["migrations/0042_backups.sql"], "state": "ci_failed", "status": "CI failed"})],
    nums(E, 0, 0))
# A rebuild waits: a thread's branch moved since the CI branch passed (site).
F = C + 5*3600_000; br = "sbxw/ci-p-ci-stale"; tip = h("st"); base = h("stb")
because = "sbxw/home-hero has 1 commit the CI branch did not test"
add("p-ci-stale", "site", "Home page: the hero and its newsletter form", ["home-hero", "newsletter"], F, "ci_stale",
    {"branch": br, "run": 1, "builds": 1, "base_branch": "main", "base": base, "tip": tip, "stale_because": because, "main_now": base},
    {"word": "CI rebuild waits", "detail": "Merged to CI without the newest commits of a thread", "can_merge": False, "why_not": f"{because}: sbxw rebuilds {br} with it and tests it again before Merge is offered. One plan rebuilds at a time, the oldest first, so this one waits its turn", "ci": True},
    lambda br, t: [(31, *built(br, tip, base)), (36, *job(tip[:7], "passed", ms=301_000)), (36, *passed(br, tip)),
                   (44, "ci_step", "sbxw", f"{because}: the CI branch waits to be rebuilt", {"step": "stale", "run": 1, "main": base, "because": because, "state": "ci_stale", "status": "CI rebuild waits"})],
    nums(F, 1, 0))
# Merged to main, from its CI branch (shop).
G = C + 6*3600_000; br = "sbxw/ci-p-ci-main"; tip = h("mn"); base = h("mnb")
add("p-ci-main", "shop", "Invoices: the PDF export and its mail", ["invoice-pdf", "invoice-mail"], G, "merged",
    {"branch": br, "run": 1, "builds": 1, "base_branch": "main", "base": base, "tip": tip},
    {"word": "Merged to main", "detail": f"main is now the tested tip of {br}", "can_merge": False, "ci": True},
    lambda br, t: [(31, *built(br, tip, base)), (35, *job(tip[:7], "passed", ms=240_000)), (35, *passed(br, tip)),
                   (50, "merge_started", "user", None, {"order": ["invoice-pdf", "invoice-mail"], "base": "main"}),
                   (51, "merge_finished", "sbxw", None, {"state": "merged", "order": ["invoice-pdf", "invoice-mail"], "status": "Merged to main", "ci": br})],
    nums(G, 1, 0, merged=G + 51*60_000), team_state="merged")

sb_ci = [{"name": n, "agent": "claude", "status": "running", "workspace": "/work/" + n, "created_at": "2026-11-10T09:12:00Z", "approvals": 0, "kits_waiting": 0} for n in ["web-app", "docs", "api", "ops", "site", "shop"]]
VIEWS = [("ci", "web-app", "testing after a rebuild because main moved"), ("ci-merged-ci", "api", "Merged to CI, its tests reused"), ("ci-failed", "ops", "CI failed on a conflict"),
         ("ci-stale", "site", "a CI rebuild waits for a thread's newer commit"), ("ci-main", "shop", "Merged to main from its CI branch"), ("ci-building", "docs", "building the CI branch")]
for name, sel, what in VIEWS:
    fx_ci = {
     "about": f"The Board's CI states (generated by gen.py): six teams merged through their CI branch, one per state (web-app testing after a rebuild because main moved, docs building, api Merged to CI with a reused result, ops CI failed on a conflict, site a rebuild waits, shop Merged to main). Opened on {sel}: {what}.",
     "app": {"selected": sel, "panes": 1, "split": "columns", "open": [sel], "focused": 0},
     "daemon": {"sandboxes": sb_ci, "plans": plans_ci, "board_teams": teams_ci, "board_timelines": timelines_ci, "commits": {n["name"]: [] for n in sb_ci}, "commit_details": {}},
    }
    json.dump(fx_ci, open(os.path.join(here, name + ".json"), "w"), indent=1)
print("ci", len(teams_ci))
