#!/bin/sh
# Create a fresh one-page native baseline in a new evidence directory.
set -eu
if [ "$(uname -s)" != Darwin ] || [ "$#" -ne 1 ]; then
    printf '%s\n' 'usage on macOS: baseline.sh new-evidence-directory' >&2
    exit 1
fi
mkdir -- "$1" || { printf '%s\n' 'Cannot create fresh evidence destination.' >&2; exit 1; }
output=$(cd -- "$1" && pwd)
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
set +e
/usr/bin/osascript - "$script_dir/baseline.jsx" "$output" > "$output/osascript.stdout" 2> "$output/osascript.stderr" <<'APPLESCRIPT'
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
if [ -f "$output/baseline.idml" ]; then
    shasum -a 256 "$output/baseline.idml" > "$output/SHA256SUMS"
fi
if [ "$status" -ne 0 ]; then
    cat "$output/osascript.stderr" >&2
    printf 'Native baseline failed; evidence retained in %s\n' "$output" >&2
    exit "$status"
fi
cat "$output/osascript.stdout"
