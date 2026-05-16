#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
jar="$script_dir/fib.jar"

kotlinc "$script_dir/Main.kt" -include-runtime -d "$jar"
kotlin "$jar" "$@"
