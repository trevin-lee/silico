# Sourced by the filler's decisions (filler.yml, proptest-room.sh): the runners each label of
# the mutants lanes is on, as /etc/ci-runner/labels gives them on the VM (this token can't
# list runners): mutants, the long lane; mutants-pr, the merge lane; filler, filler's own.
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
