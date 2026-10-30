#!/bin/sh
root=$1
mode=$2
shift 2
(IFS='|'; printf '%s\n' "$*") >> "$root/commands.txt"
for arg in "$@"; do
    if [ "$arg" = '--version' ]; then
        case "$mode" in bootstrap|bootstrap-fail) exit 1;; esac
        exit 0
    fi
    if [ "$arg" = 'ensurepip' ]; then
        if [ "$mode" = 'bootstrap-fail' ]; then echo 'bootstrap unavailable' >&2; exit 1; fi
        exit 0
    fi
done
if [ "$3" = 'wheel' ]; then
    if [ "$mode" = 'wheel-fail' ]; then echo 'wheel build failed' >&2; exit 1; fi
    shift 6
    if [ "$mode" != 'no-wheel' ]; then
        : > "$1/runpod-1-py3-none-any.whl"
        : > "$1/runpod-2-py3-none-any.whl"
    fi
    exit 0
fi
if [ "$mode" = 'large-error' ]; then
    head -c 262144 /dev/zero | tr '\000' x >&2
    printf '\n' >&2
fi
echo 'Collecting numpy>=1.0'
echo '  Downloading numpy-2.0-cp312.whl'
echo 'Collecting requests[security]'
case "$mode" in large-error|vendor-fail) echo 'resolver failed' >&2; exit 1;; esac
if [ -d "$root/seed" ]; then
    while [ "$1" != '--target' ]; do shift; done
    shift
    mkdir -p "$1"
    cp -R "$root/seed/." "$1/"
fi
exit 0
