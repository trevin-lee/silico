#!/usr/bin/env bash
# A mutants run's verdict: the summary step of .github/workflows/mutants.yml, cut from it as
# CI runs it, against made-up shards' outcomes as their artifacts hold them; and .github/mutants-none.sh,
# which a shard runs once cargo-mutants has ended well, against a made-up tree. Each case
# says whether the run must pass or fail, and what the summary must say. Above all: a run
# whose cargo-mutants found nothing to mutate passes and says so (run 43831530760 read
# INCOMPLETE), while a shard that stopped before it listed its mutants still reads
# INCOMPLETE, and a file given that isn't in the tree still fails.
# Needs bash 4, git, and jq or python3; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/mutants-summary-test.sh)
set -euo pipefail

# What a job's environment says is the cases' to give, never the job's (the gate runs this).
unset GITHUB_RUN_ATTEMPT GITHUB_STEP_SUMMARY GITHUB_RUN_ID

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/silico-mutants-summary.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

fail() {
  echo "mutants-summary: $*" >&2
  exit 1
}

passed=0
pass() {
  passed=$((passed + 1))
}

# The summary step's script, as CI runs it.
summary=$tmp/summary.sh
awk '/^      - name: Summary$/ { step = 1 } step && /^        run: \|$/ { body = 1; next }
  body && /^      [^ ]/ { exit } body { sub(/^          /, ""); print }' \
  "$repo/.github/workflows/mutants.yml" >"$summary"
grep -q 'all/incomplete.txt' "$summary" || fail "couldn't cut the Summary step from mutants.yml"

# A run of the summary over the shards made in $tmp/case/results, in the bash GitHub runs it in
# (`bash -e {0}`, as run 43831530760's log shows): its exit status in $status, what it
# printed in $said. The environment is the case's.
run=0
summarize() {
  run=$((run + 1))
  local dir=$tmp/case/checkout
  rm -rf "$dir" && mkdir -p "$dir/.github"
  cp "$repo/.github/mutants-triaged.txt" "$dir/.github/"
  status=0
  said=$(cd "$dir" && env -i PATH="$PATH" HOME="$tmp/case/home" GITHUB_RUN_ID=7 \
    GITHUB_STEP_SUMMARY="$tmp/case/step-summary.md" RESULTS="$tmp/case/results" SHARDS="${SHARDS:-1}" \
    SHARD_RESULT="${SHARD_RESULT:-success}" REF=abc BASE="${BASE-main}" PACKAGES="" \
    FILES="${FILES-}" RE="${RE-}" TESTS="" TESTS_WHY="" FILLER="" \
    bash -e "$summary" 2>&1) || status=$?
}

# A fresh case: no shards kept yet.
fresh() {
  rm -rf "$tmp/case"
  mkdir -p "$tmp/case/home" "$tmp/case/results"
}

# Shard N's outcomes as its keep step leaves them (.github/mutants-keep.sh) and its artifact
# downloads them: the `kept` marker, its threads, then whatever the case adds.
shard() {
  local dir=$tmp/case/results/mutants-shard-$1
  mkdir -p "$dir"
  echo 1 >"$dir/kept"
  echo "2 threads" >"$dir/threads"
  echo "$dir"
}

expect() { # pass|fail, then the words the summary must hold
  local want=$1 words=$2 what=$3
  case $want in
  pass) [ "$status" -eq 0 ] || fail "$what: failed ($status) where it should pass: $said" ;;
  fail) [ "$status" -ne 0 ] || fail "$what: passed where it should fail: $said" ;;
  esac
  case $said in *"$words"*) ;; *) fail "$what: doesn't say \"$words\": $said" ;; esac
}

# 1. cargo-mutants found nothing to mutate in the files given (a change to data only): the
# shard ended well and said so, with every file in the tree. It passes, saying so.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
: >"$d/none"
FILES="crates/silico-oracle/src/sketch_edit.rs" summarize
expect pass "No mutants: cargo-mutants found nothing to mutate in crates/silico-oracle/src/sketch_edit.rs (the change since main)" "found no mutants"
case $said in *INCOMPLETE*) fail "found no mutants: it says INCOMPLETE: $said" ;; esac
pass

# 2. A shard that stopped before cargo-mutants listed its mutants (no list, no `none`): still
# INCOMPLETE, whether the shard job failed or (a lost marker) reads as a success.
for result in failure success; do
  fresh
  shard 0 >/dev/null
  SHARD_RESULT=$result FILES="crates/silico-oracle/src/sketch_edit.rs" summarize
  expect fail "shard 0 stopped before cargo-mutants listed its mutants" "a stopped shard ($result)"
  case $said in *"No mutants"*) fail "a stopped shard ($result): it says No mutants: $said" ;; esac
  pass
done

# 3. A file given that isn't in the tree: the shard found no mutants and named it. It fails,
# naming the file, and never says No mutants.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
echo "no file in the tree matches crates/silico-core/src/typo.rs" >"$d/none"
FILES="crates/silico-core/src/typo.rs" summarize
expect fail "no file in the tree matches crates/silico-core/src/typo.rs" "a misspelt file"
case $said in *"No mutants"*) fail "a misspelt file: it says No mutants: $said" ;; esac
pass

# 4. A regex that matched nothing: it fails, since a regex can't be checked against the tree.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
: >"$d/none"
RE='nothing\.rs' summarize
expect fail "the regex matched no mutants" "a regex that matched nothing"
pass

# 5. No files or regex (packages, or a whole change), and nothing to mutate: it passes, saying
# so.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
: >"$d/none"
summarize
expect pass "No mutants: cargo-mutants found nothing to mutate (the change since main)" "a change with nothing to mutate"
pass

# 6. Two shards that had none, one with an empty list, one with none at all, each having
# checked the files: it passes, saying so.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
echo '[]' >"$d/mutants.json"
: >"$d/none"
d=$(shard 1)
echo 1 >"$d/shard"
: >"$d/none"
SHARDS=2 FILES="crates/a.rs" summarize
expect pass "No mutants" "two shards with none"
pass

# 6b. With no base, cargo-mutants writes an empty list when the files given hold no mutants:
# one that checked them and found each passes, saying so; one that found a file missing
# fails, naming it; and one with no marker (it didn't check) fails as before, since a
# misspelt file must never pass.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
echo '[]' >"$d/mutants.json"
: >"$d/none"
BASE="" FILES="crates/silico-core/src/cache.rs" summarize
expect pass "No mutants: cargo-mutants found nothing to mutate in crates/silico-core/src/cache.rs" "no base, nothing to mutate"
case $said in *"the change since"*) fail "no base: it speaks of a change: $said" ;; esac
pass
echo "no file in the tree matches crates/silico-core/src/cahce.rs" >"$d/none"
BASE="" FILES="crates/silico-core/src/cahce.rs" summarize
expect fail "no file in the tree matches crates/silico-core/src/cahce.rs" "no base, a misspelt file"
pass
rm "$d/none"
BASE="" FILES="crates/silico-core/src/cahce.rs" summarize
expect fail "the files matched no mutants" "no base, an empty list, unchecked"
pass

# 7. And one of two shards that found none while the other stopped before listing: INCOMPLETE.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
: >"$d/none"
shard 1 >/dev/null
SHARDS=2 SHARD_RESULT=failure FILES="crates/a.rs" summarize
expect fail "shard 1 stopped before cargo-mutants listed its mutants" "one shard with none, one stopped"
pass

# 8. The controls: mutants tested, all caught, passes with no "No mutants"; one missed fails.
fresh
d=$(shard 0)
echo 0 >"$d/shard"
echo '[{"name": "a.rs:1:1"}, {"name": "a.rs:2:1"}]' >"$d/mutants.json"
printf 'a.rs:1:1: replace + with - in f\na.rs:2:1: replace f -> u8 with 0\n' >"$d/caught.txt"
FILES="crates/a.rs" summarize
expect pass "| 2 | 0 | 0 | 0 | 0 | 1 of 1 |" "two caught"
case $said in *"No mutants"*) fail "two caught: it says No mutants: $said" ;; esac
pass
printf 'a.rs:2:1: replace f -> u8 with 0\n' >"$d/missed.txt"
printf 'a.rs:1:1: replace + with - in f\n' >"$d/caught.txt"
FILES="crates/a.rs" summarize
expect fail "1 mutants missed" "one missed"
pass

# .github/mutants-none.sh, against a tree of three files.
git -C "$tmp" init -q tree
mkdir -p "$tmp/tree/crates/a/src"
echo "fn f() {}" >"$tmp/tree/crates/a/src/lib.rs"
echo "data" >"$tmp/tree/crates/a/src/data.rs"
echo "fn g() {}" >"$tmp/tree/top.rs"
git -C "$tmp/tree" add crates top.rs
git -C "$tmp/tree" -c user.name=t -c user.email=t@t commit -qm tree
commit=$(git -C "$tmp/tree" rev-parse HEAD)
none() { # out, then the files given
  local out=$1
  shift
  rm -rf "$out"
  mkdir -p "$out"
  (cd "$tmp/tree" && bash "$repo/.github/mutants-none.sh" "$out" "$commit" "$@")
}
out=$tmp/none-out

# Every file there, by path, by a file's name or a directory's (`a`), or by a glob: `none`,
# empty.
for files in "crates/a/src/data.rs" "data.rs" "crates/*/src/*.rs" "crates/a/src/lib.rs data.rs" "a" "d*.rs"; do
  set -f
  # shellcheck disable=SC2086 # split, as the shard splits FILES
  none "$out" $files
  set +f
  [ -f "$out/none" ] && [ ! -s "$out/none" ] || fail "mutants-none, '$files': not an empty none: $(cat "$out/none" 2>/dev/null)"
  pass
done

# No files given (packages, or a whole change): `none`, empty.
none "$out"
[ -f "$out/none" ] && [ ! -s "$out/none" ] || fail "mutants-none, no files: not an empty none"
pass

# A file that isn't there, beside one that is: `none` names it alone.
none "$out" crates/a/src/data.rs crates/a/src/typo.rs typo.rs
[ "$(cat "$out/none")" = "no file in the tree matches crates/a/src/typo.rs
no file in the tree matches typo.rs" ] || fail "mutants-none, a misspelt file: $(cat "$out/none")"
pass

# A glob with a / is matched from the tree's root, not as a path's tail: src/lib.rs matches
# nothing here.
none "$out" src/lib.rs
[ "$(cat "$out/none")" = "no file in the tree matches src/lib.rs" ] || fail "mutants-none, a partial path: $(cat "$out/none")"
pass

# `**` as no directory at all, as globset matches it: crates/a/src/**/lib.rs is
# crates/a/src/lib.rs, and **/top.rs the root's top.rs.
none "$out" "crates/a/src/**/lib.rs" "**/top.rs" "crates/**/lib.rs"
[ -f "$out/none" ] && [ ! -s "$out/none" ] || fail "mutants-none, \`**\` as no directory: $(cat "$out/none")"
pass

# A list of mutants written: no `none`. An empty list (no base, the files holding none): a
# `none`, with the files checked. A list that can't be read: no `none`, left to the summary.
rm -rf "$out"
mkdir -p "$out"
echo '[{"name": "a.rs:1:1"}]' >"$out/mutants.json"
(cd "$tmp/tree" && bash "$repo/.github/mutants-none.sh" "$out" "$commit" crates/a/src/typo.rs)
[ ! -e "$out/none" ] || fail "mutants-none: wrote none beside a list of mutants"
pass
echo '[]' >"$out/mutants.json"
(cd "$tmp/tree" && bash "$repo/.github/mutants-none.sh" "$out" "$commit" crates/a/src/typo.rs data.rs)
[ "$(cat "$out/none")" = "no file in the tree matches crates/a/src/typo.rs" ] ||
  fail "mutants-none, an empty list: $(cat "$out/none" 2>/dev/null)"
pass
rm "$out/none"
echo 'not json' >"$out/mutants.json"
(cd "$tmp/tree" && bash "$repo/.github/mutants-none.sh" "$out" "$commit" crates/a/src/typo.rs)
[ ! -e "$out/none" ] || fail "mutants-none: wrote none beside a list it couldn't read"
pass

# A commit that isn't there: it fails (the shard then reads as incomplete), writing nothing.
rm -rf "$out"
mkdir -p "$out"
if (cd "$tmp/tree" && bash "$repo/.github/mutants-none.sh" "$out" 0000000000000000000000000000000000000000 data.rs) 2>/dev/null; then
  fail "mutants-none: a missing commit passed"
fi
[ ! -e "$out/none" ] || fail "mutants-none: a missing commit wrote none"
pass

echo "mutants-summary: $passed cases passed"
