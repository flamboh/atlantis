#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/netflow-db-cluster.sh [options] -- [extra pipeline flags]

Split an inclusive local-date range into contiguous day shards, build each shard with
`netflow-db pipeline` as a detached job on a remote host, copy the shards back, merge them with
`netflow-db merge-shards`, and verify the merged product.

Required:
  --hosts LIST         Comma-separated ssh hosts. Append :N for N concurrent shards on a host
                       (e.g. nodeA:2,nodeB,nodeC).
  --dataset ID         Dataset ID in the remote registry.
  --start-date DATE    First local day (YYYY-MM-DD).
  --end-date DATE      Last local day, inclusive.
  --output PATH        New merged product on this machine; it must not exist.

Options (defaults may also come from the environment):
  --remote-dir DIR     Remote install directory [NETFLOW_CLUSTER_REMOTE_DIR, default:
                       $HOME/atlantis-cluster expanded with this machine's $HOME]. Every host
                       uses this same absolute path. It holds bin/netflow-db,
                       nfdump/libexec/nfdump, datasets.json and work/.
  --work-dir DIR       Local directory for shard copies, logs and the recorded layout
                       [NETFLOW_CLUSTER_WORK_DIR, default: <output>.shards].
  --keep-shards        Keep the local shard copies after verify passes.
  --deploy-bin PATH    Copy this netflow-db binary to every host first.
  --deploy-nfdump PATH Copy this nfdump executable to every host first.
  --deploy-datasets PATH
                       Copy this dataset registry to every host first.
  --netflow-db PATH    Local netflow-db used for merge and verify
                       [NETFLOW_DB_BIN, default: <remote-dir>/bin/netflow-db on this machine].

Every shard uses identical flags, an explicit end date, and the same nfdump path, so every shard
records the same product identity. Remote shard databases stay on host-local disk under
<remote-dir>/work, named by dataset and day range. Rerun the same command to resume: running
shards are reattached and completed days are skipped. A rerun with a different layout is refused.
EOF
}

hosts=""
dataset=""
start_date=""
end_date=""
output=""
remote_dir="${NETFLOW_CLUSTER_REMOTE_DIR:-}"
work_dir="${NETFLOW_CLUSTER_WORK_DIR:-}"
keep_shards=0
deploy_bin=""
deploy_nfdump=""
deploy_datasets=""
local_bin="${NETFLOW_DB_BIN:-}"
poll_seconds="${NETFLOW_CLUSTER_POLL_SECONDS:-30}"
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
    --keep-shards) keep_shards=1; shift ;;
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
if ! "$local_bin" merge-shards --help >/dev/null 2>&1; then
  echo "local netflow-db at $local_bin cannot run merge-shards; pass --netflow-db" >&2
  exit 2
fi
output="$(realpath -m "$output")"
work_dir="$(realpath -m "${work_dir:-$output.shards}")"
if [[ -e "$output" ]]; then
  echo "output already exists: $output" >&2
  exit 1
fi

slots=()
IFS=',' read -r -a host_specs <<<"$hosts"
unique_hosts=()
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
  unique_hosts+=("$host")
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
if ((shard_count > ${#days[@]})); then
  shard_count=${#days[@]}
fi
per_shard=$(((${#days[@]} + shard_count - 1) / shard_count))

shard_hosts=()
shard_names=()
shard_ranges=()
safe_dataset="$(printf '%s' "$dataset" | tr -c 'A-Za-z0-9_.-' '-')"
for ((shard = 0; shard < shard_count; shard++)); do
  first=$((shard * per_shard))
  if ((first >= ${#days[@]})); then
    break
  fi
  last=$((first + per_shard - 1))
  if ((last >= ${#days[@]})); then
    last=$((${#days[@]} - 1))
  fi
  shard_hosts+=("${slots[$shard]}")
  shard_ranges+=("${days[$first]} ${days[$last]}")
  shard_names+=("$safe_dataset.${days[$first]}_${days[$last]}")
done

mkdir -p "$work_dir"
layout="$(
  printf 'dataset %s\nremote-dir %s\nflags %s\n' "$dataset" "$remote_dir" "${extra[*]}"
  for index in "${!shard_names[@]}"; do
    printf 'shard %s %s\n' "${shard_hosts[$index]}" "${shard_ranges[$index]}"
  done
)"
if [[ -f "$work_dir/layout" ]]; then
  if [[ "$(cat "$work_dir/layout")" != "$layout" ]]; then
    echo "$work_dir/layout records a different shard layout; rerun with the same --hosts, dates," >&2
    echo "dataset and flags, or delete that file to start over" >&2
    exit 1
  fi
else
  printf '%s\n' "$layout" >"$work_dir/layout"
fi

trap 'trap - INT TERM; kill 0' INT TERM

remote_quote() {
  printf '%q' "$1"
}

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

run_shard() {
  local host=$1 name=$2 first=$3 last=$4
  local remote_work="$remote_dir/work"
  local db="$remote_work/$name.sqlite"
  local snapshot="$remote_work/$name.snapshot.sqlite"
  local status="$remote_work/$name.status"
  local pid_file="$remote_work/$name.pid"
  local remote_log="$remote_work/$name.log"
  local pipeline=(
    "$remote_dir/bin/netflow-db" pipeline
    --dataset "$dataset"
    --datasets "$remote_dir/datasets.json"
    --start-date "$first"
    --end-date "$last"
    --database-path "$db"
    --nfdump "$remote_dir/nfdump/libexec/nfdump"
    "${extra[@]}"
  )
  local job
  job="$(printf '%q ' "${pipeline[@]}") && rm -f $(remote_quote "$snapshot")"
  job+=" && $(printf '%q ' "$remote_dir/bin/netflow-db" sqlite-maintenance "$db" "$snapshot")"
  job+="; echo \$? > $(remote_quote "$status.tmp") && mv -f $(remote_quote "$status.tmp") $(remote_quote "$status")"
  local start
  start="cd $(remote_quote "$remote_work")
if [ -f $(remote_quote "$pid_file") ] && kill -0 \"\$(cat $(remote_quote "$pid_file"))\" 2>/dev/null; then
  echo attached
  exit 0
fi
rm -f $(remote_quote "$status")
setsid nohup bash -c $(remote_quote "$job") >$(remote_quote "$remote_log") 2>&1 </dev/null &
echo \$! >$(remote_quote "$pid_file")
echo started"
  local state
  state="$(ssh -o BatchMode=yes -- "$host" bash -s <<<"$start")"
  echo "shard $name on $host: $state"

  local probe
  probe="if [ -f $(remote_quote "$status") ]; then echo \"exit \$(cat $(remote_quote "$status"))\"
elif kill -0 \"\$(cat $(remote_quote "$pid_file") 2>/dev/null)\" 2>/dev/null; then echo running
else echo lost; fi"
  local result=""
  while true; do
    if ! result="$(ssh -o BatchMode=yes -o ConnectTimeout=20 -- "$host" bash -s <<<"$probe")"; then
      echo "shard $name on $host: cannot reach host, retrying" >&2
      result=""
    fi
    case "$result" in
      exit* | lost) break ;;
    esac
    sleep "$poll_seconds"
  done
  scp -q -- "$host:$remote_log" "$work_dir/$name.log" || true
  if [[ "$result" != "exit 0" ]]; then
    echo "shard $name on $host failed ($result); see $work_dir/$name.log" >&2
    return 1
  fi
  scp -q -- "$host:$snapshot" "$work_dir/$name.sqlite"
  ssh -o BatchMode=yes -- "$host" "rm -f $(remote_quote "$snapshot")"
}

pids=()
shard_paths=()
for index in "${!shard_names[@]}"; do
  read -r first last <<<"${shard_ranges[$index]}"
  echo "shard ${shard_names[$index]}: $first..$last on ${shard_hosts[$index]}"
  run_shard "${shard_hosts[$index]}" "${shard_names[$index]}" "$first" "$last" &
  pids+=("$!")
  shard_paths+=("$work_dir/${shard_names[$index]}.sqlite")
done

failed=0
for pid in "${pids[@]}"; do
  if ! wait "$pid"; then
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
if ((!keep_shards)); then
  for path in "${shard_paths[@]}"; do
    rm -f -- "$path" "$path-wal" "$path-shm" "$path-journal"
  done
fi
echo "merged product: $output"
