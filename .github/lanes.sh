# Sourced by the filler's decisions (filler.yml, proptest-room.sh) and by a real mutants run's
# cancelling of filler (cancel-filler.sh), so none can disagree: the runners each label of the
# mutants lanes is on, as /etc/ci-runner/labels gives them on the VM (this token can't list
# runners): mutants, the long lane; mutants-pr, the merge lane; filler, filler's own. And the
# filler sweeps made lately (sweeps), and the explorations that ran lately (explored), which
# say whether each one's once-a-day minimum is owed.
lane_mutants='9 10 11 12 15 16 17'
lane_mutants_pr='5 6 7 8 9 10 11 12 15 16 17 20 21'
lane_filler='5 6 7 8 9 10 11 12 22 23'

# The runners of label $1's lane, by number. Any other label has no lane here: none.
lane() {
  case $1 in
  mutants) echo "$lane_mutants" ;;
  mutants-pr) echo "$lane_mutants_pr" ;;
  filler) echo "$lane_filler" ;;
  esac
}

# Whether runner $2 (its number) is of label $1's lane.
in_lane() {
  case " $(lane "$1") " in *" $2 "*) return 0 ;; esac
  return 1
}

# How many runners of label $1's lane are free: not in $running, the runners running a job now,
# by number, each with a space either side (" 5 9 23 ").
free_count() {
  local n count=0
  for n in $(lane "$1"); do
    case $running in *" $n "*) ;; *) count=$((count + 1)) ;; esac
  done
  echo "$count"
}

# Whether a runner of label $1's lane is free (free_count).
free_for() {
  [ "$(free_count "$1")" -gt 0 ]
}

# The filler oracle sweeps (oracle-sweep.yml, titled "filler ...") made on BRANCH in the last
# $1 seconds. Every one counts, going or ended, for the sweep due every four hours: a sweep
# started is that turn's. With "uncancelled" as $2, those cancelled are left out, for the
# sweep overdue after a day: one real work cancelled (cancel-filler.sh) may have ended before
# most of its cases, and the day's minimum is still owed. With "ended" as $3 too, those going
# are left out: the minimum is owed until one ends, so a real run deciding whether a sweep
# going is overdue (cancel-filler.sh) doesn't count that sweep as the day's (the filler and
# the exploration's room ask only while none is going). Needs gh, and BRANCH.
sweeps() {
  gh run list --workflow oracle-sweep.yml --branch "$BRANCH" --limit 100 --json displayTitle,createdAt,conclusion,status \
    --jq "[.[] | select((.displayTitle | startswith(\"filler \"))
            and now - (.createdAt | fromdateiso8601) < $1
            and (\"${2-}\" != \"uncancelled\" or .conclusion != \"cancelled\")
            and (\"${3-}\" != \"ended\" or .status == \"completed\"))] | length"
}

# The explorations (proptest-explore.yml) on BRANCH whose explore job passed or failed having
# run a round in the last $1 seconds, of the runs made in twice that (a run can wait, then run,
# for hours). One cancelled, or skipped for want of room, doesn't count, nor one whose starved
# build left no time for a round, which ends passed: its step "Rounds ran" is skipped
# (proptest-explore.yml). Nor one going: the day's minimum is owed until one ends. None in a
# day, the exploration is overdue (proptest-room.sh). Needs gh, GH_REPO, and BRANCH; fails
# if a look at GitHub does, whatever the caller's shell options.
explored() {
  local runs run ended count=0
  runs=$(gh run list --workflow proptest-explore.yml --branch "$BRANCH" --limit 50 --json databaseId,createdAt \
    --jq ".[] | select(now - (.createdAt | fromdateiso8601) < $((2 * $1))) | .databaseId") || return 1
  for run in $runs; do
    ended=$(gh api "repos/$GH_REPO/actions/runs/$run/jobs?per_page=100" --jq \
      "[.jobs[] | select(.name == \"explore\" and (.conclusion == \"success\" or .conclusion == \"failure\")
         and any(.steps[]?; .name == \"Rounds ran\" and .conclusion == \"success\")
         and now - (.completed_at | fromdateiso8601) < $1)] | length") || return 1
    count=$((count + ended))
  done
  echo "$count"
}
