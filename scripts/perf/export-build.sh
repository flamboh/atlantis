#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."
perf_output="${1:-$(mktemp -d /tmp/atlantis-perf-build.XXXXXX)}"
mkdir -p "$perf_output"
perf_context="${ATLANTIS_PERF_DOCKER_CONTEXT:-atlantis-perf}"
perf_image=atlantis-perf-benchmark-build
docker --context "$perf_context" build --target build --platform linux/amd64 \
  --file apps/web/Dockerfile --tag "$perf_image" .
perf_container=$(docker --context "$perf_context" create --name "atlantis-perf-bundle-$$" "$perf_image")
trap 'docker --context "$perf_context" rm "$perf_container" > /dev/null' EXIT
docker --context "$perf_context" cp "$perf_container:/app/apps/web/.svelte-kit" "$perf_output/"
printf 'Benchmark with --bundle-dir %s/.svelte-kit/output/client\n' "$perf_output"
