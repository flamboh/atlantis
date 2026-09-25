#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/netflow-db-cluster.sh [options] -- [extra pipeline flags]

Split an inclusive local-date range into contiguous day shards, build each shard with
`netflow-db pipeline` on a remote host over ssh, copy the shards back, merge them with
`netflow-db merge-shards`, and verify the merged product.

Required:
  --hosts LIST         Comma-separated ssh hosts. Append :N for N concurrent shards on a host
                       (e.g. nodeA:2,nodeB,nodeC). At most 11 slots in total.
  --dataset ID         Dataset ID in the remote registry.
  --start-date DATE    First local day (YYYY-MM-DD).
  --end-date DATE      Last local day, inclusive.
  --output PATH        New merged product on this machine; it must not exist.

Options (defaults may also come from the environment):
  --remote-dir DIR     Remote install directory, same absolute path on every host
                       [NETFLOW_CLUSTER_REMOTE_DIR, default: $HOME/atlantis-cluster]. It holds
                       bin/netflow-db, nfdump/libexec/nfdump, datasets.json and work/.
  --work-dir DIR       Local directory for copied shards [NETFLOW_CLUSTER_WORK_DIR,
                       default: <output>.shards].
  --deploy-bin PATH    Copy this netflow-db binary to every host first.
  --deploy-nfdump PATH Copy this nfdump executable to every host first.
  --deploy-datasets PATH
                       Copy this dataset registry to every host first.
  --netflow-db PATH    Local netflow-db used for merge and verify
                       [NETFLOW_DB_BIN, default: <remote-dir>/bin/netflow-db on this machine].

Every shard uses identical flags and the same nfdump path, so every shard records the same
product identity. Remote shard databases stay on host-local disk under <remote-dir>/work.
Re-running the same command resumes: completed days are skipped by the pipeline.
EOF
}

hosts=""
dataset=""
start_date=""
end_date=""
output=""
remote_dir="${NETFLOW_CLUSTER_REMOTE_DIR:-}"
work_dir="${NETFLOW_CLUSTER_WORK_DIR:-}"
deploy_bin=""
deploy_nfdump=""
deploy_datasets=""
local_bin="${NETFLOW_DB_BIN:-}"
extra=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --hosts) hosts="$2"; shift 2 ;;
    --dataset) dataset="$2"; shift 2 ;;
    --start-date) start_date="$2"; shift 2 ;;
    --end-date) end_date="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    --remote-dir) remote_dir="$2"; shift 2 ;;
    --work-dir) work_dir="$2"; shift 2 ;;
    --deploy-bin) deploy_bin="$2"; shift 2 ;;
    --deploy-nfdump) deploy_nfdump="$2"; shift 2 ;;
    --deploy-datasets) deploy_datasets="$2"; shift 2 ;;
    --netflow-db) local_bin="$2"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    --) shift; extra=("$@"); break ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for required in hosts dataset start_date end_date output; do
  if [[ -z "${!required}" ]]; then
    echo "missing --${required//_/-}" >&2
    usage >&2
    exit 2
  fi
done
remote_dir="${remote_dir:-$HOME/atlantis-cluster}"
if [[ "$remote_dir" != /* ]]; then
  echo "--remote-dir must be absolute so every host records the same nfdump path" >&2
  exit 2
fi
local_bin="${local_bin:-$remote_dir/bin/netflow-db}"
output="$(realpath -m "$output")"
work_dir="$(realpath -m "${work_dir:-$output.shards}")"
if [[ -e "$output" ]]; then
  echo "output already exists: $output" >&2
  exit 1
fi

slots=()
IFS=',' read -r -a host_specs <<<"$hosts"
for spec in "${host_specs[@]}"; do
  host="${spec%%:*}"
  count=1
  if [[ "$spec" == *:* ]]; then
    count="${spec##*:}"
  fi
  if ! [[ "$count" =~ ^[1-9][0-9]*$ ]]; then
    echo "invalid host concurrency in $spec" >&2
    exit 2
  fi
  for ((index = 0; index < count; index++)); do
    slots+=("$host")
  done
done

days=()
day="$start_date"
while [[ ! "$day" > "$end_date" ]]; do
  days+=("$day")
  day="$(date -d "$day + 1 day" +%F)"
done
if [[ ${#days[@]} -eq 0 ]]; then
  echo "empty date range" >&2
  exit 2
fi
shard_count=${#slots[@]}
if ((shard_count > 11)); then
  echo "merge-shards accepts at most 11 shards; use at most 11 slots in --hosts" >&2
  exit 2
fi
if ((shard_count > ${#days[@]})); then
  shard_count=${#days[@]}
fi

remote_quote() {
  printf '%q' "$1"
}

unique_hosts=()
for spec in "${host_specs[@]}"; do
  unique_hosts+=("${spec%%:*}")
done
deploy() {
  local host=$1 source=$2 target=$3
  local staged="$target.deploy.$$"
  scp -q -p -- "$source" "$host:$staged"
  ssh -o BatchMode=yes -- "$host" "mv -f $(remote_quote "$staged") $(remote_quote "$target")"
}

for host in "${unique_hosts[@]}"; do
  ssh -o BatchMode=yes -- "$host" "mkdir -p $(remote_quote "$remote_dir")/bin $(remote_quote "$remote_dir")/nfdump/libexec $(remote_quote "$remote_dir")/work"
  if [[ -n "$deploy_bin" ]]; then
    deploy "$host" "$deploy_bin" "$remote_dir/bin/netflow-db"
  fi
  if [[ -n "$deploy_nfdump" ]]; then
    deploy "$host" "$deploy_nfdump" "$remote_dir/nfdump/libexec/nfdump"
  fi
  if [[ -n "$deploy_datasets" ]]; then
    deploy "$host" "$deploy_datasets" "$remote_dir/datasets.json"
  fi
done

mkdir -p "$work_dir"
run_label="$(printf '%s' "$dataset-$start_date-$end_date" | tr -c 'A-Za-z0-9_.-' '-')"
pids=()
shard_paths=()
per_shard=$(((${#days[@]} + shard_count - 1) / shard_count))
for ((shard = 0; shard < shard_count; shard++)); do
  first=$((shard * per_shard))
  if ((first >= ${#days[@]})); then
    break
  fi
  last=$((first + per_shard - 1))
  if ((last >= ${#days[@]})); then
    last=$((${#days[@]} - 1))
  fi
  host="${slots[$shard]}"
  name="$run_label.shard-$shard"
  remote_db="$remote_dir/work/$name.sqlite"
  remote_snapshot="$remote_dir/work/$name.snapshot.sqlite"
  pipeline=(
    "$remote_dir/bin/netflow-db" pipeline
    --dataset "$dataset"
    --datasets "$remote_dir/datasets.json"
    --start-date "${days[$first]}"
    --end-date "${days[$last]}"
    --database-path "$remote_db"
    --nfdump "$remote_dir/nfdump/libexec/nfdump"
    "${extra[@]}"
  )
  remote_command="cd $(remote_quote "$remote_dir/work") && $(printf '%q ' "${pipeline[@]}")"
  remote_command+=" && rm -f $(remote_quote "$remote_snapshot")"
  remote_command+=" && $(printf '%q ' "$remote_dir/bin/netflow-db" sqlite-maintenance "$remote_db" "$remote_snapshot")"
  log="$work_dir/$name.log"
  echo "shard $shard: ${days[$first]}..${days[$last]} on $host (log: $log)"
  (
    ssh -o BatchMode=yes -o ServerAliveInterval=60 -- "$host" bash -s <<<"$remote_command" >"$log" 2>&1
    scp -q -- "$host:$remote_snapshot" "$work_dir/$name.sqlite"
    ssh -o BatchMode=yes -- "$host" "rm -f $(remote_quote "$remote_snapshot")"
  ) &
  pids+=("$!")
  shard_paths+=("$work_dir/$name.sqlite")
done

failed=0
for index in "${!pids[@]}"; do
  if ! wait "${pids[$index]}"; then
    echo "shard $index failed; see $work_dir/$run_label.shard-$index.log" >&2
    failed=1
  fi
done
if ((failed)); then
  echo "rerun the same command to resume; completed days are skipped" >&2
  exit 1
fi

if ((${#shard_paths[@]} == 1)); then
  cp -- "${shard_paths[0]}" "$output"
else
  "$local_bin" merge-shards --output "$output" "${shard_paths[@]}"
fi
verify_flags=(--require-data --require-processed --require-rollup-parity --require-no-raw-ip)
if [[ " ${extra[*]} " != *" --no-maad "* ]]; then
  verify_flags+=(--require-maad-data)
fi
"$local_bin" verify "$output" --dataset-id "$dataset" "${verify_flags[@]}"
echo "merged product: $output"
