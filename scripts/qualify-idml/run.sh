#!/bin/sh
# Optional native check. Writes only a fresh evidence directory and closes only
# the document opened by qualify.jsx. Normal Rust tests do not invoke InDesign.
set -eu
if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'This check requires macOS and Adobe InDesign.' >&2
    exit 1
fi
if [ "$#" -ne 2 ]; then
    printf '%s\n' 'usage: run.sh input.idml new-evidence-directory' >&2
    exit 1
fi
input=$(cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")
[ -f "$input" ] || { printf '%s\n' 'Input file is missing.' >&2; exit 1; }
# Atomic creation rejects existing destinations (including symlinks). The parent
# directory must already exist; never reuse or clean up an earlier capture.
mkdir -- "$2" || { printf '%s\n' 'Cannot create fresh evidence destination.' >&2; exit 1; }
output=$(cd -- "$2" && pwd)
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cp -- "$input" "$output/input.idml"
shasum -a 256 "$output/input.idml" > "$output/SHA256SUMS"
set +e
/usr/bin/osascript - "$script_dir/qualify.jsx" "$output" > "$output/osascript.stdout" 2> "$output/osascript.stderr" <<'APPLESCRIPT'
on run arguments
    set nativeScript to POSIX file (item 1 of arguments)
    set evidenceDirectory to item 2 of arguments
    with timeout of 600 seconds
        tell application id "com.adobe.InDesign"
            return do script nativeScript language javascript with arguments {evidenceDirectory}
        end tell
    end timeout
end run
APPLESCRIPT
status=$?
set -e
printf '%s\n' "$status" > "$output/osascript.exit-status"
if [ -f "$output/resaved.idml" ]; then
    shasum -a 256 "$output/resaved.idml" >> "$output/SHA256SUMS"
fi
if [ "$status" -ne 0 ]; then
    cat "$output/osascript.stderr" >&2
    printf 'Native qualification failed; evidence retained in %s\n' "$output" >&2
    exit "$status"
fi
cat "$output/osascript.stdout"
