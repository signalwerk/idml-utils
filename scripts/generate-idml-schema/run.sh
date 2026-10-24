#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
    printf '%s\n' 'This launcher requires macOS and Adobe InDesign.' >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

exec /usr/bin/osascript - "$script_dir/generate-idml-schema.jsx" <<'APPLESCRIPT'
on run arguments
    set schemaScript to POSIX file (item 1 of arguments)

    with timeout of 600 seconds
        tell application id "com.adobe.InDesign"
            return do script schemaScript language javascript
        end tell
    end timeout
end run
APPLESCRIPT
