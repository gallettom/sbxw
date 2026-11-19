# Generates desktop/fixtures/board/basic.json: invented, neutral content.
import json, hashlib, sys
out = sys.argv[1]
DAY = 86_400_000
T0 = 1794_902_400_000  # 2026-11-17 08:00 UTC
seq = [0]
def h(s): return hashlib.sha1(s.encode()).hexdigest()
GROUP = {"team_note":"notes","board_decision":"notes","board_warning":"notes","board_handoff":"notes","conflict":"notes","board_question":"questions",
 "relay_question":"questions","relay_routed":"questions","relay_drafted":"questions","relay_answer":"questions","user_question":"questions",
 "user_answer":"questions","screenshot_asked":"questions","screenshot_given":"questions","relay_refused":"questions","commit":"commits","claim":"claims","release":"claims"}
def ev(at, kind, frm, text=None, to=None, data=None, prefix="l"):
    seq[0] += 1
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
run.reverse()
run_team = {"id": "p-run", "sandbox": "web-app", "summary": "Orders v2: the API routes, the web page that uses them, and their docs", "state": "running", "variant": "split",
  "lead": "Orders lead", "threads": [{"name": "api-routes", "state": "done", "model": "claude-sonnet-5-5"}, {"name": "web-page", "state": "blocked"}, {"name": "docs-update", "state": "working"}],
  "created_ms": m(0), "updated_ms": m(98), "last_ms": m(98)}
cl = [e for e in run if e["kind"] == "commit"]
def totals(cs):
    t = {}
    for c in cs:
        th = c["data"]["thread"]; x = t.setdefault(th, {"thread": th, "commits": 0, "files": 0, "added": 0, "removed": 0})
        x["commits"] += 1; x["files"] += c["data"]["files"]; x["added"] += c["data"]["added"]; x["removed"] += c["data"]["removed"]
    return list(t.values())
def clist(cs): return [{"hash": c["data"]["hash"], "short": c["data"]["short"], "subject": c["text"], "author": "Agent", "at": c["at"]//1000, "thread": c["data"]["thread"], "branch": c["data"]["branch"], "merged": c["data"]["merged"], "files": c["data"]["files"], "added": c["data"]["added"], "removed": c["data"]["removed"], "repo": ""} for c in cs]
standing = [{"id": e["id"].replace("l", "b"), "at": e["at"], "kind": "decision" if e["kind"] == "board_decision" else "warning", "by": e["from"], "text": e["text"], "files": e["data"]["files"], "guess": False} for e in run if e["kind"] in ("board_decision", "board_warning")]

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
 "about": "The Board (feat/board/): three teams, invented (generated by gen.py beside it). p-run is at work on web-app (a warning and a decision on the board, a round of questions answered, a question to the docs sandbox answered, a screenshot still waiting, two threads committing, one start that failed); p-old on docs is merged; p-new on api waits for approval. The stand-in answers /api/teams from board_teams and /api/teams/:id/timeline from board_timelines; commit details through the Commits feature's stand-in (commit_details).",
 "app": {"selected": "web-app", "panes": 2, "split": "columns", "open": ["web-app", "docs"], "focused": 1},
 "daemon": {
  "sandboxes": sandboxes, "plans": plans,
  "board_teams": [run_team, pro_team, mer_team],
  "board_timelines": {
    "p-run": {"team": run_team, "events": run, "standing": standing, "commits": {"available": True, "list": clist(sorted(cl, key=lambda c: -c["at"])), "totals": totals(cl), "merged": False}},
    "p-old": {"team": mer_team, "events": mer, "standing": [{"id": "b90", "at": mm(45), "kind": "decision", "by": "guide-search", "text": "The search index is built at deploy, never in the browser.", "files": [], "guess": False}], "commits": {"available": True, "list": clist(sorted(mcl, key=lambda c: -c["at"])), "totals": totals(mcl), "merged": True}},
    "p-new": {"team": pro_team, "events": pro, "standing": [], "commits": {"available": True, "list": [], "totals": [], "merged": False}},
  },
  "commits": {"web-app": [], "docs": []},
  "commit_details": details,
 }}
json.dump(fx, open(out, "w"), indent=1)
print("ok", len(run), len(mer))
