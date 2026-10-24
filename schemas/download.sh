#!/usr/bin/env bash
# Downloads the published IDML schema snapshots from their pinned GitHub
# commits, checks them against SHA256SUMS and writes their manifest.json.
#
#   ./schemas/download.sh            download into schemas/ (existing files are checked, never replaced)
#   ./schemas/download.sh DIR        download into DIR instead
#   ./schemas/download.sh --verify   only check what is in schemas/
set -euo pipefail

here=$(cd -- "$(dirname -- "$0")" && pwd)
sums="$here/SHA256SUMS"

compact="RELAX NG Compact"
trang="RELAX NG XML syntax, converted from the compact syntax by Trang 20091111 upstream"
transpect="transpect/idmlvalidation	fd816e2bde439c6c4ff1a7b053c01f4449f7a2fb"

# folder, InDesign, DOMVersion, format, repository, commit, path, note.
# Files come from the first row of a folder; further rows are other sources
# of the same files.
sources="\
6.0/single	CS4 (build not recorded)	6.0	$compact	jaumeortola/ickmull	9406b262dd4bed966dfe835e7dff344be16c2f98	ickmull/externals/idml_schema	Single-file form only (IDMarkupLanguage.rnc); the two repositories have identical copies.
6.0/single	CS4 (build not recorded)	6.0	$compact	aki2o/emacs-genrnc	da75b1966a73ad215ec2ced4522c25f4d0bf1f9a	test/systemtest/adobe_idml	Single-file form only (IDMarkupLanguage.rnc); the two repositories have identical copies.
7.0.3.535/package	CS5 7.0.3.535	7.0	$compact	$transpect	schema/rnc/7.0	-
7.5.3.333/package	CS5.5 7.5.3.333	7.5	$compact	$transpect	schema/rnc/7.5	-
8.0.0.370/package	CS6 8.0.0.370	8.0	$trang	$transpect	schema/rng/8.0	-
10.0.0.70/package	CC 2014 10.0.0.70	10.0	$trang	$transpect	schema/rng/10.0	-
11.0.0.72/package	CC 2015 11.0.0.72	11.0	$trang	$transpect	schema/rng/11.0	-
15.0.1.209/package	2020 15.0.1.209	15.0	$compact	$transpect	schema/rnc/15.0	Upstream commented out the invalid comment lines in datatype.rnc (commit 94b2854).
21.5.0.64/package	2026 21.5.0.64	21.5	$compact	metanorma/idml	adc6a109df73a60496a0b8e1fc863b8d3b538b38	reference-docs/schemas/package	Upstream commented out the invalid comment lines in datatype.rnc (fix-adobe-schema-bug.rb)."

if command -v sha256sum >/dev/null; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

# The schema files of a folder, as listed in SHA256SUMS.
files() { grep "  $1/" "$sums" | cut -d' ' -f3- | sed "s#^$1/##"; }

# The manifest.json of a published folder, from its rows in $sources.
manifest() {
    local folder=$1 first=1 indesign dom format repo commit path note
    while IFS=$'\t' read -r f indesign dom format repo commit path note; do
        [ "$f" = "$folder" ] || continue
        if [ "$first" = 1 ]; then
            printf '{\n  "indesignVersion": "%s",\n  "domVersion": "%s",\n  "format": "%s",\n  "source": [\n' "$indesign" "$dom" "$format"
        else
            printf ',\n'
        fi
        printf '    {\n      "repository": "https://github.com/%s",\n      "commit": "%s",\n      "path": "%s"\n    }' \
            "$repo" "$commit" "$path"
        first=0
        modified="no: files as published"
        [ "$note" = "-" ] || modified="$modified; $note"
    done <<<"$sources"
    local key=packageFiles
    [ "${folder##*/}" = single ] && key=singleFiles
    printf '\n  ],\n  "modified": "%s",\n  "%s": [\n' "$modified" "$key"
    files "$folder" | sed 's/.*/    "&"/' | sed '$!s/$/,/'
    printf '  ]\n}\n'
}

# check FILE EXPECTED-HASH
check() {
    if [ "$(sha256 "$1")" != "$2" ]; then
        echo "checksum mismatch: $1" >&2
        return 1
    fi
}

# check_manifest DIR FOLDER: the manifest must be the one this script writes.
check_manifest() {
    local file="$1/${2%/*}/manifest.json"
    if [ ! -f "$file" ]; then
        echo "missing: ${2%/*}/manifest.json" >&2
        return 1
    elif ! diff -q <(manifest "$2") "$file" >/dev/null; then
        echo "differs from download.sh: ${2%/*}/manifest.json" >&2
        return 1
    fi
}

published=$(cut -f1 <<<"$sources" | uniq)

if [ "${1:-}" = "--verify" ]; then
    failed=0
    while read -r hash file; do
        if [ -f "$here/$file" ]; then
            check "$here/$file" "$hash" || failed=1
        else
            echo "missing: $file" >&2
            failed=1
        fi
    done <"$sums"
    for folder in $published; do
        check_manifest "$here" "$folder" || failed=1
    done
    if [ "$failed" = 0 ]; then
        echo "all $(wc -l <"$sums" | tr -d ' ') schema files match SHA256SUMS; manifests match"
    fi
    exit "$failed"
fi

target=${1:-$here}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for folder in $published; do
    IFS=$'\t' read -r _ _ _ _ repo commit path _ <<<"$(grep -m1 "^$folder	" <<<"$sources")"
    count=0
    while read -r hash file; do
        destination="$target/$file"
        if [ -f "$destination" ]; then
            check "$destination" "$hash"
        else
            curl -fsSL --retry 3 -o "$tmp/file" "https://raw.githubusercontent.com/$repo/$commit/$path/${file#"$folder"/}"
            check "$tmp/file" "$hash"
            mkdir -p "$(dirname "$destination")"
            mv "$tmp/file" "$destination"
            chmod 644 "$destination"
        fi
        count=$((count + 1))
    done < <(grep "  $folder/" "$sums")
    if [ -f "$target/${folder%/*}/manifest.json" ]; then
        check_manifest "$target" "$folder"
    else
        manifest "$folder" >"$target/${folder%/*}/manifest.json"
    fi
    echo "$folder: $count files from $repo@${commit:0:7}"
done
