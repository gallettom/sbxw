#!/usr/bin/env bash
# The one place that knows this repository's test commands.
#
#   scripts/run-suite.sh crates|js|desktop|all
#
#   crates   cargo test -p sbxw-core -p sbxw-daemon -p sbxw-sbx -p sbxw
#   js       node --test tests/js/*.test.mjs
#   desktop  (in desktop/) CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1 --no-fail-fast
#            (--no-fail-fast: without it a failing lib test makes cargo skip
#            desktop/tests/*, which once hid failures)
#   all      the three, one after the other, none skipped because another failed
#
# SBXW_TEST_TARGET: the cargo target directory (crates and desktop keep a
# subdirectory each: two packages of one name must not share one). Unset, a
# git-ignored directory under this checkout (target/run-suite).
#
# tests/quarantine.txt lists tests to skip in this blocking run (see there for
# the format). They are printed as skipped; a bad line stops the run before
# anything starts. Exits non-zero when any suite fails; ends with one summary
# line per suite: `suite <name>: PASS|FAIL (<n> passed, <n> failed, <n>
# quarantined) <s>s`.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
target="${SBXW_TEST_TARGET:-$root/target/run-suite}"

usage() {
  echo "usage: scripts/run-suite.sh crates|js|desktop|all" >&2
  exit 2
}
[ $# -eq 1 ] || usage
case "$1" in
  crates | js | desktop) suites=("$1") ;;
  all) suites=(crates js desktop) ;;
  *) usage ;;
esac

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# Quarantined test names of a suite, one per line; a bad list ends the run.
quarantined() {
  node "$here/quarantine.mjs" "$1"
}

# Sum the counts a runner printed: "<passed> <failed>".
count() {
  case "$1" in
    js) awk '/^ℹ pass [0-9]+/ {p += $3} /^ℹ fail [0-9]+/ {f += $3} END {print p + 0, f + 0}' "$log" ;;
    *) awk '/^test result:/ {for (i = 1; i <= NF; i++) { if ($i == "passed;") p += $(i - 1); if ($i == "failed;") f += $(i - 1) }} END {print p + 0, f + 0}' "$log" ;;
  esac
}

run_one() {
  local suite="$1" out name
  names=()
  out="$(quarantined "$suite")" || return 2
  while IFS= read -r name; do
    [ -n "$name" ] && names+=("$name")
  done <<< "$out"

  echo "== suite $suite =="
  for name in ${names[@]+"${names[@]}"}; do echo "quarantined, skipped: $name"; done

  case "$suite" in
    crates | desktop)
      local skip=()
      for name in ${names[@]+"${names[@]}"}; do skip+=(--skip "$name"); done
      if [ "$suite" = crates ]; then
        (cd "$root" && CARGO_TARGET_DIR="$target/crates" cargo test -p sbxw-core -p sbxw-daemon -p sbxw-sbx -p sbxw -- ${skip[@]+"${skip[@]}"}) 2>&1 | tee "$log"
      else
        (cd "$root/desktop" && CARGO_TARGET_DIR="$target/desktop" CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1 --no-fail-fast -- ${skip[@]+"${skip[@]}"}) 2>&1 | tee "$log"
      fi
      ;;
    js)
      local flags=() pattern
      if [ "${#names[@]}" -gt 0 ]; then
        # --test-skip-pattern needs node 22.1 or later.
        node -e 'const [a, b] = process.versions.node.split(".").map(Number); process.exit(a > 22 || (a === 22 && b >= 1) ? 0 : 1)' \
          || { echo "run-suite: skipping quarantined js tests needs node 22.1 or later" >&2; return 2; }
        while IFS= read -r pattern; do
          [ -n "$pattern" ] && flags+=("--test-skip-pattern=$pattern")
        done < <(node "$here/quarantine.mjs" js --patterns)
      fi
      (cd "$root" && node --test ${flags[@]+"${flags[@]}"} tests/js/*.test.mjs) 2>&1 | tee "$log"
      ;;
  esac
}

overall=0
summary=()
names=()
for suite in "${suites[@]}"; do
  start=$SECONDS
  run_one "$suite"
  status=$?
  read -r passed failed < <(count "$suite")
  [ "$status" -ne 0 ] && overall=1
  if [ "$status" -eq 0 ]; then verdict=PASS; else verdict=FAIL; fi
  summary+=("suite $suite: $verdict ($passed passed, $failed failed, ${#names[@]} quarantined) $((SECONDS - start))s")
done

echo
printf '%s\n' "${summary[@]}"
exit "$overall"
