#!/bin/sh
# RES-015 pilot run. Live model call. usage: scripts/pilot/run.sh <task-commit> <devcode|opencode> <evaluator-test>
set -u
good=$1; harness=$2; evaltest=$3
repo=$(git rev-parse --show-toplevel)
work=${PILOT_WORK:-${TMPDIR:-/tmp}/devcode-pilot}
export CARGO_TARGET_DIR=$work/target
model=nvidia/nemotron-3.5-super-vl-120b-a12b
budget=${PILOT_SECONDS:-900}
dir=$work/runs/$good-$harness
rm -rf "$dir"; mkdir -p "$dir"
# Fresh tree from the parent commit; no fix history in the workspace.
git -C "$repo" archive "$good^" | tar -x -C "$dir"
find "$dir" -type f -exec touch {} + # archive keeps old mtimes; cargo would reuse a stale build from another tree
(cd "$dir" && git init -q && git add -A && git -c user.name=pilot -c user.email=pilot@localhost commit -q -m baseline)
prompt=$(cat "$repo/scripts/pilot/tasks/$good.md")
start=$(date +%s)
case $harness in
  devcode)
    printf '{"provider":{"kind":"openai_compat","base_url":"https://integrate.api.nvidia.com/v1","api_key_env":"NVIDIA_API_KEY","connector":"nim","model":"%s","context_window":128000,"max_rpm":30},"task_token_cap":300000,"verification":{"test_command":"cargo test"}}' "$model" > "$dir/.devcode.json"
    (cd "$dir" && perl -e 'alarm shift; exec @ARGV' "$budget" "$repo/target/debug/devcode" --plain -y --trace "$prompt") >"$work/runs/$good-$harness.log" 2>&1 ;;
  opencode)
    (cd "$dir" && perl -e 'alarm shift; exec @ARGV' "$budget" opencode run --auto --format json -m "nvidia/$model" "$prompt") >"$work/runs/$good-$harness.log" 2>&1 ;;
esac
rc=$?
end=$(date +%s)
git -C "$repo" show "$good:tests/$evaltest.rs" > "$dir/tests/$evaltest.rs"
(cd "$dir" && cargo test --test "$evaltest" >"$work/runs/$good-$harness.eval.log" 2>&1); erc=$?
echo "task=$good harness=$harness harness_exit=$rc seconds=$((end-start)) evaluator_exit=$erc"
(cd "$dir" && git status --short | head -20)
