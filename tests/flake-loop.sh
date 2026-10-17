#!/usr/bin/env bash
# Runs a test again and again under CPU load, to make a flaky test fail:
#
#   tests/flake-loop.sh [-r rounds] [-p parallel] [-s stressors] [-t seconds]
#                       [-o dir] TEST [ARGS...]
#
# TEST is an integration test in tests/ (headless, cli, ...), built with
# `cargo test --release`, or the path of an executable (a name with a /).
# ARGS go to it as they are: a filter, --exact, -q. Each of the rounds
# (default 10) runs parallel copies (default 2) at once, from the repository's
# root as cargo does, beside the stressors (default 2), busy loops. The output
# of each run that fails is kept in dir (default: a new one in $TMPDIR). At the
# end it prints runs= and fails=, and exits 1 if a run failed.
#
# No stressor outlives the script (#123): it stops them however it ends, and
# each stops by itself within a second of the script going, SIGKILL too, or
# after -t seconds (default 600), so a hung test can't hold the machine.
#
#   tests/flake-loop.sh -r 20 -p 4 -s 8 headless reading_watcher_stays_connected --exact
set -euo pipefail

usage() { sed -n '2,/^$/s/^# \{0,1\}//p' "$0"; }

rounds=10 par=2 nstress=2 limit=600 out=
while getopts r:p:s:t:o:h opt; do
    case $opt in
        r) rounds=$OPTARG ;;
        p) par=$OPTARG ;;
        s) nstress=$OPTARG ;;
        t) limit=$OPTARG ;;
        o) out=$OPTARG ;;
        h) usage && exit 0 ;;
        *) usage >&2 && exit 2 ;;
    esac
done
shift $((OPTIND - 1))
if (($# == 0)); then usage >&2 && exit 2; fi
if ! [[ "$rounds.$par.$nstress.$limit" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "flake-loop: -r, -p, -s and -t take whole numbers" >&2 && exit 2
fi
test=$1
shift
if [[ $test == */* && $test != /* ]]; then test=$PWD/$test; fi
cd "$(dirname "$0")/.."
if [[ $test == */* ]]; then
    bin=$test
else
    bin=$(cargo test --release --no-run --test "$test" --message-format=json |
        grep -F '"kind":["test"]' | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p')
fi
tmp=${TMPDIR:-/tmp}
out=${out:-$(mktemp -d "${tmp%/}/flake-loop.XXXXXX")}
mkdir -p "$out"

stress=() runs=()
# Stops the stressors and the runs still going, on any exit; the signals are
# trapped so that they exit through here too.
cleanup() {
    local pids=(${stress[@]+"${stress[@]}"} ${runs[@]+"${runs[@]}"})
    if ((${#pids[@]})); then kill "${pids[@]}" 2>/dev/null || true; fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# Spins until the script is gone or the time is up. Builtins only, so nothing
# forks while it runs; checking every 10000 turns is every 10 to 40 ms.
stressor() {
    local end=$((SECONDS + limit)) i
    while kill -0 "$owner" 2>/dev/null && ((SECONDS < end)); do
        for ((i = 0; i < 10000; i++)); do :; done
    done
}
owner=$$
for ((i = 0; i < nstress; i++)); do
    # Nothing of the caller's open: a pipe it reads isn't held open.
    stressor </dev/null >/dev/null 2>&1 &
    stress+=($!)
done
if ((nstress)); then echo "stressors: ${stress[*]}"; fi

fails=0
for ((r = 1; r <= rounds; r++)); do
    for ((p = 1; p <= par; p++)); do
        # A script's job starts with SIGINT and SIGQUIT ignored, and a test
        # that stops something with SIGINT would fail; under job control
        # (set -m) it starts with them as cargo test leaves them. Off at once,
        # so that bash prints no job notices.
        set -m
        "$bin" ${1+"$@"} </dev/null >"$out/$r.$p.log" 2>&1 &
        set +m
        runs+=($!)
    done
    for ((p = 1; p <= par; p++)); do
        if wait "${runs[p - 1]}"; then rm "$out/$r.$p.log"; else fails=$((fails + 1)); fi
    done
    runs=()
done
echo "runs=$((rounds * par)) fails=$fails"
if ((fails)); then echo "output of the failed runs: $out" && exit 1; fi
rmdir "$out" 2>/dev/null || true
