#!/usr/bin/env bash
# lead-review.sh: what a lead reads before proposing a merge. Read-only, no
# decision in it: it prints facts about a thread's branch and leaves the
# judgment to the lead.
#
#   scripts/lead-review.sh <thread> [--scope <path>...] [--main <ref>]
#   scripts/lead-review.sh --after-merge [--main <ref>]
#
# For branch sbxw/<thread>: its commits on top of main, the diff stat, whether
# it merges cleanly into main and beside every other live sbxw/* branch (one
# with commits of its own), the files it changed outside the given scope
# paths, whether its worktree is dirty, what it left there (.sbxw-artifacts/
# content, target* dirs, scratch worktrees), and the added lines that log.
#
# --after-merge: every live thread branch and its merge state against the
# current main, for the "main moved" round after a merge.
#
# Exit code 0 always, except 2 for a missing branch or a bad argument. Plain
# text, stable order (branches and files sorted), no build, no network, no
# write to the repository (git merge-tree --write-tree only writes objects).

set -u
export LC_ALL=C

usage() {
  sed -n '4,6p' "$0" | sed 's/^# //' >&2
  exit 2
}

main_ref=""
thread=""
after_merge=0
scopes=()
while [ $# -gt 0 ]; do
  case "$1" in
    --after-merge) after_merge=1 ;;
    --main) shift; [ $# -gt 0 ] || usage; main_ref="$1" ;;
    --scope)
      shift
      [ $# -gt 0 ] || usage
      # Every following word up to the next option is a scope path.
      while [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; do
        scopes+=("${1%/}")
        shift
      done
      continue
      ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *) [ -z "$thread" ] || usage; thread="$1" ;;
  esac
  shift
done

git rev-parse --git-dir >/dev/null 2>&1 || { echo "not in a git repository" >&2; exit 2; }

if [ -z "$main_ref" ]; then
  if git rev-parse -q --verify refs/heads/main >/dev/null; then main_ref=main
  elif git rev-parse -q --verify refs/heads/master >/dev/null; then main_ref=master
  else echo "no main branch: pass --main <ref>" >&2; exit 2
  fi
fi
git rev-parse -q --verify "$main_ref^{commit}" >/dev/null || { echo "no such ref: $main_ref" >&2; exit 2; }

# The conflicting files between two refs, or nothing when they merge cleanly.
# Prints "CLEAN" or "CONFLICT: a, b".
merge_state() {
  local out rc
  out=$(git merge-tree --write-tree --name-only --no-messages "$1" "$2" 2>/dev/null)
  rc=$?
  if [ $rc -eq 0 ]; then
    echo "CLEAN"
  elif [ $rc -eq 1 ]; then
    echo "CONFLICT: $(printf '%s\n' "$out" | tail -n +2 | sed '/^$/d' | sort -u | paste -sd, - | sed 's/,/, /g')"
  else
    echo "UNKNOWN (git merge-tree failed)"
  fi
}

# Live thread branches: sbxw/* with commits main lacks, sorted.
live_branches() {
  git for-each-ref --format='%(refname:short)' 'refs/heads/sbxw/*' | sort | while read -r b; do
    [ "$(git rev-list --count "$main_ref..$b" 2>/dev/null || echo 0)" -gt 0 ] && echo "$b"
  done
}

if [ $after_merge -eq 1 ]; then
  [ -z "$thread" ] || usage
  echo "== Live thread branches against $main_ref ($(git rev-parse --short "$main_ref")) =="
  any=0
  while read -r b; do
    [ -n "$b" ] || continue
    any=1
    ahead=$(git rev-list --count "$main_ref..$b")
    behind=$(git rev-list --count "$b..$main_ref")
    echo "$b: $ahead ahead, $behind behind: $(merge_state "$main_ref" "$b")"
  done < <(live_branches)
  [ $any -eq 1 ] || echo "(none)"
  exit 0
fi

[ -n "$thread" ] || usage
branch="sbxw/$thread"
git rev-parse -q --verify "refs/heads/$branch" >/dev/null || { echo "no branch $branch" >&2; exit 2; }

echo "== $branch against $main_ref =="
echo "tip: $(git log -1 --format='%h %s' "$branch")"
echo
echo "== Commits on top of $main_ref =="
log=$(git log --reverse --format='%h %s' "$main_ref..$branch")
[ -n "$log" ] && echo "$log" || echo "(none)"
echo
echo "== Diff stat ($main_ref...$branch) =="
stat=$(git diff --stat "$main_ref...$branch")
[ -n "$stat" ] && echo "$stat" || echo "(no change)"
echo
echo "== Merge state =="
echo "$main_ref: $(merge_state "$main_ref" "$branch")"
others=0
while read -r b; do
  [ -n "$b" ] && [ "$b" != "$branch" ] || continue
  others=1
  echo "$b: $(merge_state "$b" "$branch")"
done < <(live_branches)
[ $others -eq 1 ] || echo "(no other live sbxw/* branch)"
echo
echo "== Files outside the scope =="
if [ ${#scopes[@]} -eq 0 ]; then
  echo "(no --scope given)"
else
  outside=$(git diff --name-only "$main_ref...$branch" | sort | while read -r f; do
    in=0
    for s in "${scopes[@]}"; do
      if [ "$f" = "$s" ] || [ "${f#"$s"/}" != "$f" ]; then in=1; break; fi
    done
    [ $in -eq 1 ] || echo "$f"
  done)
  [ -n "$outside" ] && echo "$outside" || echo "(none)"
fi
echo

# The thread's worktree: the one registered on its branch.
wt=$(git worktree list --porcelain | awk -v want="refs/heads/$branch" '
  /^worktree /{p=substr($0,10)} /^branch /{if (substr($0,8)==want) print p}' | head -n 1)
echo "== Worktree =="
if [ -z "$wt" ] || [ ! -d "$wt" ]; then
  echo "(no worktree registered on $branch)"
else
  echo "path: $wt"
  dirty=$(git -C "$wt" status --porcelain 2>/dev/null)
  if [ -z "$dirty" ]; then
    echo "clean"
  else
    echo "DIRTY ($(printf '%s\n' "$dirty" | wc -l | tr -d ' ') entries):"
    printf '%s\n' "$dirty" | head -n 30
  fi
  echo
  echo "== Leftovers in the worktree =="
  left=0
  if [ -d "$wt/.sbxw-artifacts" ]; then
    files=$(cd "$wt" && find .sbxw-artifacts -type f 2>/dev/null | sort | head -n 30)
    if [ -n "$files" ]; then left=1; echo "$files"; fi
  fi
  targets=$(cd "$wt" && find . -maxdepth 3 -type d -name 'target*' -not -path './.git/*' -prune 2>/dev/null | sort)
  if [ -n "$targets" ]; then left=1; echo "$targets" | sed 's|^\./||; s|$|/ (target dir)|'; fi
  scratch=$(git worktree list --porcelain | awk -v base="$wt/" '/^worktree /{p=substr($0,10); if (index(p, base)==1) print p}' | sort)
  if [ -n "$scratch" ]; then left=1; echo "$scratch" | sed 's|$| (registered worktree)|'; fi
  [ $left -eq 1 ] || echo "(none)"
fi
echo

echo "== Added lines that log =="
logs=$(git diff -U0 "$main_ref...$branch" | awk '
  /^\+\+\+ / { f = substr($0, 5); sub(/^b\//, "", f); next }
  /^@@ / { split($3, a, ","); n = substr(a[1], 2) + 0; next }
  /^\+/ { if ($0 ~ /(log::|tracing::|console\.(log|error|warn|info|debug))/) print f ":" n ": " substr($0, 2); n++; next }
')
[ -n "$logs" ] && echo "$logs" || echo "(none)"
exit 0
