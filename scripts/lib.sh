# Shared helpers, sourced by the other scripts.
set -euo pipefail

die() {
    echo "$*" >&2
    exit 1
}

require_factorio_closed() {
    if pgrep -x factorio >/dev/null; then
        die "Quit Factorio first."
    fi
}
