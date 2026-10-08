#!/usr/bin/env bash
# Proves the shared OCCT build cache (cache.sh, older-layouts.sh) keeps, replaces, and
# removes entries as documented, against fake builds. Needs no OCCT build.
# Run through pixi (for perl):  pixi run test-cache
set -euo pipefail

scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck disable=SC1091
. "$scripts/platform.sh"
# shellcheck disable=SC1091
. "$scripts/test-lib.sh"

# Fake builds are complete: their install/.stamp holds their stamp.
cache_sh() { bash "$scripts/cache.sh" "$@"; }
count_entries() { find "$1" -maxdepth 1 -name "$2" | wc -l | tr -d ' '; }
fake_stamp() { printf "%064d" "$1"; }
new_build() { # <stamp>
  local d
  d=$(mktemp -d "$cache/.build-XXXXXX")
  mkdir -p "$d/install" "$d/src"
  echo "$1" > "$d/install/.stamp"
  echo "$d"
}
publish_fakes() { # <stamp number>...
  local i
  for i in "$@"; do cache_sh publish "$cache" "$(fake_stamp "$i")" "$(new_build "$(fake_stamp "$i")")" >/dev/null; done
}
used_in_order() { # <stamp number>..., least recently used first
  local i n=0
  for i in "$@"; do
    n=$((n + 1))
    touch -t "20260101000$n" "$cache/.used-$(fake_stamp "$i")"
  done
}
build_of() { echo "$cache/$(readlink "$cache/$(fake_stamp "$1")")"; } # all fakes share a name prefix
entries() { # <cache>: its fake stamps' numbers
  find "$1" -maxdepth 1 -type l | xargs -n1 basename | sed 's/^0*//' | sort -n | tr '\n' ' '
}
dangling_in() { find "$1" -maxdepth 1 -type l ! -exec test -e {} \; -print | wc -l | tr -d ' '; }

# Publishing: only the first build of a stamp wins, publishing records a use, and an
# incomplete build is refused.
cache=$tmp/cache1
mkdir -p "$cache"
stamp=$(printf 'a%.0s' $(seq 1 64))
for who in first second; do
  work=$(new_build "$stamp")
  echo "$who" > "$work/marker"
  echo "$who: $(cache_sh publish "$cache" "$stamp" "$work")" >> "$tmp/publish.log"
done
if [ "$(cat "$cache/$stamp/marker")" = first ] && grep -q "second: already published" "$tmp/publish.log" \
  && [ "$(count_entries "$cache" '.occt-*')" = 1 ] && [ ! -e "$work" ] && [ -f "$cache/.used-$stamp" ]; then
  echo "ok: cache publish keeps the first build, discards the second, and records the use"
else
  guard_broken "cache publish let a second build replace or nest in the first, or recorded no use"
fi
work=$(new_build "$(fake_stamp 2)")
expect_fail_with "cache publish (incomplete build)" "refusing to publish" \
  cache_sh publish "$cache" "$(fake_stamp 3)" "$work"

# Pruning keeps entries by last use, which here differs from name order and publish order,
# and removes the rest with their builds and records.
cache=$tmp/cache2
mkdir -p "$cache"
publish_fakes 1 2 3 4 5
used_in_order 3 4 5 2 1
cache_sh prune "$cache" 3
if [ "$(entries "$cache")" = "1 2 5 " ] && [ "$(count_entries "$cache" '.occt-*')" = 3 ] \
  && [ "$(count_entries "$cache" '.used-*')" = 3 ] && [ "$(dangling_in "$cache")" = 0 ]; then
  echo "ok: cache prune keeps the 3 most recently used entries and their builds"
else
  guard_broken "cache prune kept [$(entries "$cache")], $(count_entries "$cache" '.occt-*') builds, $(count_entries "$cache" '.used-*') records, $(dangling_in "$cache") dangling links"
fi

# Pruning never removes an entry a live checkout uses, even the least recently used; a
# gone checkout's registration is dropped, and its entry becomes prunable.
cache=$tmp/cache3
mkdir -p "$cache"
publish_fakes 1 2 3
cache_sh use "$cache" "$(fake_stamp 1)" "$tmp/checkoutA"
cache_sh use "$cache" "$(fake_stamp 2)" "$tmp/checkoutB"
used_in_order 1 2 3
rm -rf "$tmp/checkoutB"
cache_sh prune "$cache" 1
if [ "$(entries "$cache")" = "1 3 " ] && [ -e "$tmp/checkoutA/install/.stamp" ] \
  && [ "$(find "$cache/.links" -type f | wc -l | tr -d ' ')" = 1 ]; then
  echo "ok: cache prune keeps entries a live checkout uses and forgets gone checkouts"
else
  guard_broken "cache prune kept [$(entries "$cache")]: removed a used entry, or kept a gone checkout's"
fi

# Using an entry records a use, and a used entry counts toward <keep>: after using the
# least recently used entry, it alone survives a prune that keeps one.
cache=$tmp/cache4
mkdir -p "$cache"
publish_fakes 1 2 3 4
used_in_order 1 2 3 4
cache_sh use "$cache" "$(fake_stamp 1)" "$tmp/checkoutC"
cache_sh prune "$cache" 1
if [ "$(entries "$cache")" = "1 " ]; then
  echo "ok: cache use records a use, and used entries count toward the entries kept"
else
  guard_broken "cache prune kept [$(entries "$cache")] after using 1: use recorded no use, or used entries were kept on top"
fi

# A registration follows its checkout's current link: once a checkout uses another entry,
# or the same stamp in another cache, its old entry is no longer protected.
cache=$tmp/cache5-other
mkdir -p "$cache"
publish_fakes 3
cache=$tmp/cache5
mkdir -p "$cache"
publish_fakes 1 2 3 4
cache_sh use "$cache" "$(fake_stamp 1)" "$tmp/checkoutE"
cache_sh use "$cache" "$(fake_stamp 2)" "$tmp/checkoutE"
cache_sh use "$cache" "$(fake_stamp 3)" "$tmp/checkoutF"
cache_sh use "$tmp/cache5-other" "$(fake_stamp 3)" "$tmp/checkoutF"
used_in_order 1 2 3 4
cache_sh prune "$cache" 1
if [ "$(entries "$cache")" = "2 4 " ]; then
  echo "ok: cache prune protects only the entry each checkout uses now"
else
  guard_broken "cache prune kept [$(entries "$cache")]: a checkout's earlier entry, or its entry in another cache, still protected one"
fi

# A record of an entry that's gone is dropped, however young, and never counted. (Under the
# lock, no entry is ever half-published.)
cache=$tmp/cache6
mkdir -p "$cache"
publish_fakes 1 2 3
used_in_order 1 2 3
touch "$cache/.used-$(fake_stamp 7)"
cache_sh prune "$cache" 3
if [ "$(entries "$cache")" = "1 2 3 " ] && [ ! -e "$cache/.used-$(fake_stamp 7)" ]; then
  echo "ok: cache prune drops records of entries that are gone, without counting them"
else
  guard_broken "cache prune kept [$(entries "$cache")]: a record without an entry was counted, or kept"
fi

# The cache is canonical, so a checkout using it through a symlink (or by a relative path)
# is still protected when pruning names the cache directly.
cache=$tmp/cache-real
mkdir -p "$cache"
ln -s "$cache" "$tmp/cache-alias"
publish_fakes 1 2 3
(cd "$tmp" && cache_sh use cache-alias "$(fake_stamp 1)" checkoutG)
used_in_order 1 2 3
cache_sh prune "$cache" 1
if [ "$(entries "$cache")" = "1 3 " ] && [ -e "$tmp/checkoutG/install/.stamp" ]; then
  echo "ok: cache prune protects entries used through another name for the cache"
else
  guard_broken "cache prune kept [$(entries "$cache")]: a checkout using a symlink or relative path lost its protection"
fi

# Broken entries (a build deleted by hand, wholly or partly, one holding another stamp's
# build, or a real directory where the link should be, as a copy that follows symlinks
# leaves) can't be used, are replaced by publishing, and are removed by pruning before
# ranking, even when a checkout uses them.
cache=$tmp/cache-broken
mkdir -p "$cache"
publish_fakes 1 2 3 4 6
cache_sh use "$cache" "$(fake_stamp 6)" "$tmp/checkoutH"
used_in_order 1 4 6
rm -rf "$(build_of 2)" "$(build_of 6)"
rm -rf "$(build_of 3)/install"
cp -R "$(build_of 1)" "$cache/.occt-copy"
ln -s .occt-copy "$cache/$(fake_stamp 5)" # a build of stamp 1 under stamp 5's name
for i in 7 8; do
  mkdir -p "$cache/$(fake_stamp "$i")/install"
  fake_stamp "$i" > "$cache/$(fake_stamp "$i")/install/.stamp"
done
if cache_sh use "$cache" "$(fake_stamp 3)" "$tmp/checkoutJ" || [ -e "$tmp/checkoutJ/install" ]; then
  guard_broken "cache use accepted a partly deleted entry"
elif cache_sh use "$cache" "$(fake_stamp 5)" "$tmp/checkoutJ" || [ -e "$tmp/checkoutJ/install" ]; then
  guard_broken "cache use accepted an entry holding another stamp's build"
elif cache_sh use "$cache" "$(fake_stamp 8)" "$tmp/checkoutJ" || [ -e "$tmp/checkoutJ/install" ]; then
  guard_broken "cache use accepted a real directory in place of an entry's link"
else
  echo "ok: cache use rejects a broken entry"
fi
results=""
for i in 2 3 7; do
  work=$(new_build "$(fake_stamp "$i")")
  echo "rebuilt $i" > "$work/marker"
  results="$results$(cache_sh publish "$cache" "$(fake_stamp "$i")" "$work" 2>&1 || echo " (failed)");"
done
pruned=$(cache_sh prune "$cache" 3 2>&1 && echo ok || echo failed)
if [ "$results" = "published over a broken entry;published over a broken entry;published over a broken entry;" ] \
  && [ "$pruned" = ok ] && [ "$(cat "$cache/$(fake_stamp 7)/marker")" = "rebuilt 7" ] \
  && [ "$(cat "$cache/$(fake_stamp 2)/marker")" = "rebuilt 2" ] && [ "$(cat "$cache/$(fake_stamp 3)/marker")" = "rebuilt 3" ] \
  && [ "$(entries "$cache")" = "2 3 7 " ] && [ ! -e "$cache/$(fake_stamp 8)" ] && [ "$(count_entries "$cache" '.occt-*')" = 3 ] \
  && [ ! -e "$cache/.used-$(fake_stamp 6)" ]; then
  echo "ok: cache publish replaces wholly and partly deleted entries, and prune removes broken ones without counting them"
else
  guard_broken "cache publish over broken entries said [$results], and prune ($pruned) kept [$(entries "$cache")]"
fi

# The lock. Every command must wait while another process holds it, and must hold it,
# exclusively, for as long as it runs. (So a prune can't remove an entry that a checkout
# registers meanwhile, and so on.) No step depends on timing: the holder keeps the lock
# until told to let go, and every wait is bounded.
wait_for() { # <seconds> <test>...: poll until the test passes; fail after <seconds>
  local deadline=$(($(date +%s) + $1))
  shift
  until "$@"; do
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep 0.05
  done
}
cache=$tmp/cache-lock
mkdir -p "$cache"
publish_fakes 1 2 3
used_in_order 1 2 3
rm -f "$tmp/locked" "$tmp/release"
perl -MFcntl=:flock -e '
  open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die;
  open(my $m, ">", $ARGV[1]) or die; close $m;
  for (1 .. 600) { exit 0 if -e $ARGV[2]; select(undef, undef, undef, 0.05) }
  die "lock holder: never released\n"' "$cache/.lock" "$tmp/locked" "$tmp/release" &
holder=$!
if wait_for 30 test -e "$tmp/locked"; then
  cache_sh prune "$cache" 1 &
  waiters=($!)
  work=$(new_build "$(fake_stamp 4)")
  cache_sh publish "$cache" "$(fake_stamp 4)" "$work" >/dev/null &
  waiters+=($!)
  cache_sh use "$cache" "$(fake_stamp 1)" "$tmp/checkoutK" &
  waiters+=($!)
  sleep 0.5
  alive=0
  for pid in "${waiters[@]}"; do if kill -0 "$pid" 2>/dev/null; then alive=$((alive + 1)); fi; done
  during="$(entries "$cache")|$(if [ -e "$tmp/checkoutK/install" ]; then echo used; fi)|$alive"
  touch "$tmp/release"
  # (Once the lock is free they run in any order, and use fails if prune ran first.)
  wait "$holder" "${waiters[@]}" || true
  if [ "$during" = "1 2 3 ||3" ]; then
    echo "ok: cache prune, publish, and use wait for the lock"
  else
    guard_broken "cache commands acted while another process held the lock (entries|use|still waiting: $during)"
  fi
else
  kill "$holder" 2>/dev/null || true
  guard_broken "the test's lock holder never took the lock"
fi
# Park a command partway through, with its registration file replaced by a FIFO that
# nobody reads yet, and check that the lock is held exclusively meanwhile.
cache=$tmp/cache-held
mkdir -p "$cache/.links" "$tmp/checkoutL"
publish_fakes 1
checkout=$(cd "$tmp/checkoutL" && pwd)
mkfifo "$cache/.links/$(printf '%s' "$checkout" | sha256_of | cut -d' ' -f1)"
cache_sh use "$cache" "$(fake_stamp 1)" "$checkout" &
user=$!
if wait_for 30 test -L "$checkout/install"; then
  if perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; exit(flock($f, LOCK_SH | LOCK_NB) ? 1 : 0)' "$cache/.lock"; then
    echo "ok: cache use holds the lock exclusively while it runs"
  else
    guard_broken "another process took the cache lock while cache.sh use was running"
  fi
else
  guard_broken "cache use never got far enough to link the checkout"
fi
cat "$cache"/.links/* >/dev/null &
reader=$!
wait "$user" || true
kill "$reader" 2>/dev/null || true

# Concurrent prunes all succeed.
cache=$tmp/cache-race
mkdir -p "$cache"
errors=0
for round in 1 2 3 4; do
  # shellcheck disable=SC2046
  publish_fakes $(seq 1 30)
  pids=()
  for k in 1 2 3 4; do
    cache_sh prune "$cache" 2 2>/dev/null &
    pids+=($!)
  done
  for pid in "${pids[@]}"; do wait "$pid" || errors=$((errors + 1)); done
  rm -rf "${cache:?}" && mkdir -p "$cache"
done
if [ "$errors" = 0 ]; then
  echo "ok: concurrent cache prunes all succeed"
else
  guard_broken "$errors of 16 concurrent cache prunes failed"
fi

# Older layouts: only Silico's own v<n> directories, never the current one or anything
# else in what may be a shared directory like ~/.cache.
root_dir=$tmp/cache-root
mkdir -p "$root_dir/v0/.links" "$root_dir/v2/downloads" "$root_dir/v1/.links" "$root_dir/vfoo/.links" \
  "$root_dir/v3" "$root_dir/other/.links" "$root_dir/pip"
found=$(bash "$scripts/older-layouts.sh" "$root_dir" v1 | xargs -n1 basename | tr '\n' ' ')
if [ "$found" = "v0 v2 " ]; then
  echo "ok: older-layouts names only older layouts of Silico's cache"
else
  guard_broken "older-layouts named [$found], expected [v0 v2 ]"
fi

# Builds in progress survive pruning, including slow ones, and so do live builds of any
# age; builds abandoned over a day ago, unrecorded entries beyond <keep>, and builds no
# entry points at are removed.
cache=$tmp/cache7
mkdir -p "$cache/.build-live" "$cache/.build-slow" "$cache/.build-old" "$cache/.occt-legacy/install" \
  "$cache/.occt-orphan"
touch -t "$(minutes_ago 30)" "$cache/.build-slow"
touch -t 200001010000 "$cache/.build-old"
fake_stamp 9 > "$cache/.occt-legacy/install/.stamp"
ln -s .occt-legacy "$cache/$(fake_stamp 9)"
publish_fakes 1 2 3
cache_sh use "$cache" "$(fake_stamp 1)" "$tmp/checkoutI"
touch -t 200001010000 "$cache"/.occt-0000*
cache_sh prune "$cache" 3
if [ -d "$cache/.build-live" ] && [ -d "$cache/.build-slow" ] && [ ! -e "$cache/.build-old" ] \
  && [ "$(entries "$cache")" = "1 2 3 " ] && [ "$(dangling_in "$cache")" = 0 ] && [ -e "$tmp/checkoutI/install/.stamp" ] \
  && [ ! -e "$cache/.occt-legacy" ] && [ ! -e "$cache/.occt-orphan" ] && [ "$(count_entries "$cache" '.occt-*')" = 3 ]; then
  echo "ok: cache prune keeps builds in progress and live builds, and removes abandoned, unrecorded, and orphaned ones"
else
  guard_broken "cache prune kept [$(entries "$cache")] with $(dangling_in "$cache") dangling: it touched a build in progress or a live build, or kept an abandoned, unrecorded, or orphaned one"
fi

finish_guards test-cache
