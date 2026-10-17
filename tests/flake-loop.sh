#!/usr/bin/env bash
# Runs a test again and again under CPU load, to make a flaky test fail:
#
#   tests/flake-loop.sh [-r rounds] [-p parallel] [-s stressors] [-t seconds]
#                       [-l load] [-o dir] TEST [ARGS...]
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
# after -t seconds (default 600), so a hung test can't hold the machine. They
# also stop when the 1-minute load average reaches -l (default twice the
# CPUs), and the runs go on: a hunt can't starve the machine (#245; #123 saw a
# load of 227). FLAKE_LOOP_LOADAVG, for the tests, names a file read for the
# load in place of /proc/loadavg or sysctl.
#
#   tests/flake-loop.sh -r 20 -p 4 -s 8 headless reading_watcher_stays_connected --exact
set -euo pipefail

usage() { sed -n '2,/^$/s/^# \{0,1\}//p' "$0"; }

rounds=10 par=2 nstress=2 limit=600 out=
unset maxload
while getopts r:p:s:t:l:o:h opt; do
    case $opt in
        r) rounds=$OPTARG ;;
        p) par=$OPTARG ;;
        s) nstress=$OPTARG ;;
        t) limit=$OPTARG ;;
        l) maxload=$OPTARG ;;
        o) out=$OPTARG ;;
        h) usage && exit 0 ;;
        *) usage >&2 && exit 2 ;;
    esac
done
shift $((OPTIND - 1))
if (($# == 0)); then usage >&2 && exit 2; fi
: "${maxload=$((2 * $(getconf _NPROCESSORS_ONLN)))}"
# 0 rounds or runs would pass having run nothing, and stressors with 0
# seconds or load would stop at once: a hunt without load is -s 0.
if ! [[ "$rounds.$par.$limit.$maxload.$nstress" =~ ^([1-9][0-9]*\.){4}(0|[1-9][0-9]*)$ ]]; then
    echo "flake-loop: -r, -p, -t and -l take whole numbers from 1, -s from 0" >&2 && exit 2
fi

# Prints the 1-minute load average, or what was read in its place.
load() {
    local l='' _
    if [[ -n ${FLAKE_LOOP_LOADAVG-} ]]; then
        read -r l _ <"$FLAKE_LOOP_LOADAVG" || true
    elif [[ -r /proc/loadavg ]]; then
        read -r l _ </proc/loadavg || true
    else
        # macOS: "{ 1.50 1.20 1.00 }".
        read -r _ l _ < <(sysctl -n vm.loadavg) || true
    fi
    echo "$l"
}
number='^[0-9]+(\.[0-9]+)?$'
# Stressors that the load can't stop aren't started.
if ((nstress)) && l=$(load) && ! [[ $l =~ $number ]]; then
    echo "flake-loop: can't read the load average (read: \"$l\")" >&2 && exit 2
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

stress=() runs=() watch=
# Stops the stressors, and the runs still going with what they started (each
# leads its process group, below), on any exit; the signals are trapped so
# that they exit through here too.
cleanup() {
    if [[ -n $watch ]]; then kill "$watch" 2>/dev/null || true; fi
    if ((${#stress[@]})); then kill "${stress[@]}" 2>/dev/null || true; fi
    local pid
    for pid in ${runs[@]+"${runs[@]}"}; do kill -- "-$pid" 2>/dev/null || true; done
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
# Stops the stressors once the load average reaches -l, or can't be read
# (load isn't kept up blind), and says so; ends with the script, or once the
# stressors are gone. It stops them rather than pausing them: the 1-minute
# average lags, so pausing and resuming would swing.
watcher() {
    local l pid up
    while kill -0 "$owner" 2>/dev/null; do
        # Not the script's stderr: a sleep left by cleanup's kill holds no pipe.
        sleep 1 2>/dev/null
        up=
        for pid in "${stress[@]}"; do if kill -0 "$pid" 2>/dev/null; then up=1; fi; done
        if [[ -z $up ]]; then return; fi
        l=$(load)
        if ! [[ $l =~ $number ]]; then
            l="can't read the load average (read: \"$l\")"
        elif ((10#${l%%.*} >= maxload)); then
            l="the load average is $l, at or above -l $maxload"
        else
            continue
        fi
        kill "${stress[@]}" 2>/dev/null || true
        echo "flake-loop: $l: the stressors are stopped, the runs go on" >&2
        return
    done
}
owner=$$
for ((i = 0; i < nstress; i++)); do
    # Nothing of the caller's open: a pipe it reads isn't held open.
    stressor </dev/null >/dev/null 2>&1 &
    stress+=($!)
done
if ((nstress)); then
    # Its stderr is the script's, so that what it says reaches the user.
    watcher </dev/null >/dev/null &
    watch=$!
    echo "stressors: ${stress[*]}"
fi

fails=0
for ((r = 1; r <= rounds; r++)); do
    for ((p = 1; p <= par; p++)); do
        # A script's job starts with SIGINT and SIGQUIT ignored, and a test
        # that stops something with SIGINT would fail; under job control
        # (set -m) it starts with them as cargo test leaves them, in a process
        # group of its own. Off at once, so that bash prints no job notices.
        set -m
        "$bin" ${1+"$@"} </dev/null >"$out/$r.$p.log" 2>&1 &
        set +m
        runs+=($!)
    done
    for ((p = 1; p <= par; p++)); do
        if wait "${runs[p - 1]}"; then rm "$out/$r.$p.log"; else fails=$((fails + 1)); fi
        # What the run left in its group goes too: the group's id isn't reused
        # while it has a member. Then the run is dropped, as its pid may be.
        kill -- "-${runs[p - 1]}" 2>/dev/null || true
        unset 'runs[p - 1]'
    done
    runs=()
done
echo "runs=$((rounds * par)) fails=$fails"
if ((fails)); then echo "output of the failed runs: $out" && exit 1; fi
rmdir "$out" 2>/dev/null || true
