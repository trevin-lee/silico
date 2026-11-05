# Sourced by the filler's decisions (filler.yml, proptest-room.sh), so the two can't disagree:
# the runners each label of the mutants lanes is on, as /etc/ci-runner/labels gives them on the
# VM (this token can't list runners): mutants, the long lane; mutants-pr, the merge lane;
# filler, filler's own. And the filler sweeps made lately (sweeps).
lane_mutants='9 10 11 12 15 16 17'
lane_mutants_pr='5 6 7 8 9 10 11 12 15 16 17 20 21'
lane_filler='5 6 7 8 9 10 11 12 22 23'

# Whether a runner of label $1's lane is free: not in $running, the runners running a job now,
# by number, each with a space either side (" 5 9 23 "). Any other label has no lane here.
free_for() {
  local label=$1 lane n
  case $label in
  mutants) lane=$lane_mutants ;;
  mutants-pr) lane=$lane_mutants_pr ;;
  filler) lane=$lane_filler ;;
  *) return 1 ;;
  esac
  for n in $lane; do
    case $running in *" $n "*) ;; *) return 0 ;; esac
  done
  return 1
}

# The filler oracle sweeps (oracle-sweep.yml, titled "filler ...") made on BRANCH in the last
# $1 seconds. Every one counts, going or ended, for the sweep due every four hours: a sweep
# started is that turn's. With "uncancelled" as $2, those cancelled are left out, for the
# sweep overdue after a day: one real work cancelled (cancel-filler.sh) may have ended before
# most of its cases, and the day's minimum is still owed. Needs gh, and BRANCH.
sweeps() {
  gh run list --workflow oracle-sweep.yml --branch "$BRANCH" --limit 100 --json displayTitle,createdAt,conclusion \
    --jq "[.[] | select((.displayTitle | startswith(\"filler \"))
            and now - (.createdAt | fromdateiso8601) < $1
            and (\"${2-}\" != \"uncancelled\" or .conclusion != \"cancelled\"))] | length"
}
