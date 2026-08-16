#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"$script_dir/compile-nfdump.sh"
"$script_dir/compile-maad.sh"
"$script_dir/compile-burstify.sh"
