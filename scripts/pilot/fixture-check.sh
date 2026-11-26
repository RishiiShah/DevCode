#!/bin/sh
# RES-015 fixture check. No model calls.
# usage: scripts/pilot/fixture-check.sh <known-good-commit> <evaluator-test-name>...
# Fails unless the evaluator fails on the parent and passes on the known-good commit.
set -u
good=$1; shift
repo=$(git rev-parse --show-toplevel)
work=${PILOT_WORK:-${TMPDIR:-/tmp}/devcode-pilot}
export CARGO_TARGET_DIR=$work/target
mkdir -p "$work"
run() { # <label> <rev> ; evaluator files always taken from $good
  dir=$work/$1
  git -C "$repo" worktree remove --force "$dir" >/dev/null 2>&1
  git -C "$repo" worktree add --detach "$dir" "$2" >/dev/null 2>&1 || return 2
  for t in "$@"; do :; done
  for t in $TESTS; do
    git -C "$repo" show "$good:tests/$t.rs" > "$dir/tests/$t.rs" || return 2
  done
  (cd "$dir" && cargo test $(for t in $TESTS; do printf -- '--test %s ' "$t"; done) >"$work/$1.log" 2>&1)
  rc=$?
  git -C "$repo" worktree remove --force "$dir" >/dev/null 2>&1
  return $rc
}
TESTS="$*"
run parent "$good^"; p=$?
run good "$good"; g=$?
echo "parent exit=$p (want nonzero) good exit=$g (want 0)"
[ "$p" -ne 0 ] && [ "$g" -eq 0 ]
