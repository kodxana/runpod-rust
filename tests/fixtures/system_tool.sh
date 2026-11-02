#!/bin/sh
scratch=$1
mode=$2
shift 2
{
    printf '%s' "$DEBIAN_FRONTEND"
    for arg in "$@"; do printf '|%s' "$arg"; done
    printf '\n'
} >> "$scratch/commands.txt"
if [ "$mode" = hang ]; then
    printf '%s' "$$" > "$scratch/pid"
    exec sleep 60
fi
if { [ "$mode" = update-fail ] && [ "$1" = update ]; } || { [ "$mode" = install-fail ] && [ "$1" = install ]; }; then
    i=0
    while [ "$i" -lt 2100 ]; do printf x >&2; i=$((i + 1)); done
    printf TAIL >&2
    exit 9
fi
exit 0
