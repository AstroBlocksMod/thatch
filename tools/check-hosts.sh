#!/usr/bin/env bash
# Keeps the hosts under hosts/ fenced off from the rest of Thatch.
#
# A host may wrap a copyleft project (Scratch Everywhere! is LGPL-3.0), so everything that touches
# one lives in its own directory, and nothing outside that directory may depend on anything in it.
# This script checks, on the files git tracks:
#
#   1. Every host has a LICENSE that says which files carry which license.
#   2. Every file of a host names its license in an SPDX-License-Identifier line near its top, or
#      in a file of the same name with ".license" added. The projects pinned as git submodules
#      bring their own license files and are not looked into.
#   3. Nothing outside hosts/ mentions a path into hosts/, which is what an include, a build
#      dependency or a workspace member would need. Prose may: the docs, the README files and the
#      CI workflows are exempt, and so are the few files listed in ALLOWED below.
#   4. No file outside hosts/ declares a GPL-family license of its own.
#
# Run it from anywhere in the repository: tools/check-hosts.sh

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Files outside hosts/ that may name a path into it, each for a reason.
ALLOWED=(
    ".gitmodules"            # pins the submodules
    ".gitignore"             # ignores the hosts' build directories
    "tools/check-hosts.sh"   # this script
    # `thatch build --host` looks for a host's build output by path. It reads the description the
    # host's build wrote there and links what that names; it includes and builds nothing of a host.
    "crates/thatch-cli/src/main.rs"
)

failed=0
fail() {
    echo "check-hosts: $*" >&2
    failed=1
}

# ---- 1 and 2: every host is labeled -----------------------------------------------------------

for host in hosts/*/; do
    [ -d "$host" ] || continue
    host=${host%/}
    if ! git ls-files --error-unmatch "$host/LICENSE" >/dev/null 2>&1; then
        fail "$host has no LICENSE that says which files carry which license"
    fi
done

# Each line is "<mode> <object> <stage><tab><path>". Mode 160000 is a submodule, which is one
# entry here however many files it holds.
while IFS= read -r entry; do
    mode=${entry%% *}
    file=${entry#*$'\t'}
    [ "$mode" = "160000" ] && continue
    if head -n 5 "$file" | grep -q "SPDX-License-Identifier: "; then
        continue
    fi
    if [ -f "$file.license" ] && grep -q "SPDX-License-Identifier: " "$file.license"; then
        continue
    fi
    fail "$file has no SPDX-License-Identifier line in its first five lines, and no $file.license"
done < <(git ls-files --stage -- hosts)

# ---- 3: nothing outside depends on a host ----------------------------------------------------

is_allowed() {
    local file=$1 allowed
    case "$file" in
        docs/* | *.md | .github/workflows/*) return 0 ;;
    esac
    for allowed in "${ALLOWED[@]}"; do
        [ "$file" = "$allowed" ] && return 0
    done
    return 1
}

while IFS= read -r file; do
    is_allowed "$file" && continue
    fail "$file mentions a path into hosts/; nothing outside hosts/ may depend on a host:"
    git grep -n -I -e 'hosts/' -- "$file" | sed 's/^/    /' >&2
done < <(git grep -l -I -e 'hosts/' -- . ':(exclude)hosts' || true)

# ---- 4: no copyleft outside ------------------------------------------------------------------

while IFS= read -r file; do
    [ "$file" = "tools/check-hosts.sh" ] && continue
    fail "$file declares a GPL-family license; copyleft code may only live under hosts/"
done < <(git grep -l -I -E -e 'SPDX-License-Identifier:.*GPL' -- . ':(exclude)hosts' || true)

if [ "$failed" -ne 0 ]; then
    exit 1
fi
echo "check-hosts: the hosts are fenced off"
