# Pipeline setup

The pipeline is the Rust `atlantis-netflow-db` crate. It converts nfcapd or CSV input into a compatible SQLite database.

Use a new output database when you change selection rules or result semantics. The pipeline rejects incompatible reuse.

## Choose a path

| Path   | Entry point                    | Host requirements                                                                                                  |
| ------ | ------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Docker | `scripts/netflow-db-docker.sh` | [Git and Docker](requirements.md#docker-pipeline)                                                                  |
| Native | `scripts/netflow-db.sh`        | [Rust toolchain](requirements.md#native-pipeline) and the [nfdump build tools](requirements.md#native-nfdump-fork) |

Complete the one-time setup for your path, then follow the rest of this document. The examples below use `./scripts/netflow-db.sh`, and the native path adds `--nfdump` to every command that reads nfcapd input. To run an example with Docker, substitute `./scripts/netflow-db-docker.sh` and add `--capture-root <path>` for commands that read captures.

### Docker setup

Container options come before the pipeline command:

```bash
./scripts/netflow-db-docker.sh --capture-root /absolute/path/to/captures pipeline ...
```

The wrapper builds the image when it is missing or when its build inputs change, so pulling an update rebuilds automatically. Pass `--build` to force a rebuild. Each `--capture-root` mounts read-only at the same absolute path inside the container, so `root_path` in `datasets.json` needs no change. Output stays under the repository's `data/` directory, owned by you.

On macOS, bind mounts over large capture trees are slower than native filesystem access.

### Native setup

`scripts/netflow-db.sh` runs the crate with `cargo run --locked --release`, which compiles it when necessary. Set `NETFLOW_DB_BIN` to run a prebuilt binary instead.

The native gate below also needs `jq`, Python 3, GNU `time` with verbose (`-v`) support, and a
running per-user `systemd` manager on a cgroup-v2 host. `jq`, Python, and GNU `time` are supplied
on `PATH` by `shell.nix`; `systemd-run` and `systemctl` normally come from the host. Run the gate
from `nix-shell shell.nix` (or an equivalent environment).

nfcapd input also needs the pinned ATLANTIS nfdump fork. A system nfdump installation does not work: the pipeline uses an output mode that only the fork has. CSV input does not need nfdump.

1. Initialize the Git submodules.

   ```bash
   git submodule update --init --recursive
   ```

2. Build the fork. The script checks for the [build tools](requirements.md#native-nfdump-fork) and names any tool that is missing.

   ```bash
   ./vendor/scripts/compile-nfdump.sh
   ```

The build stages the executable at `target/nfdump/libexec/nfdump`. The `target` directory is disposable and git-ignored. Pass this path with `--nfdump` to every command that reads nfcapd input; the pipeline does not find it automatically.

## Process a dataset

First, complete the [dataset configuration](datasets.md).

Run a bounded import while you test the configuration:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD>
```

The start date and end date are inclusive; use the same date for both to process a single day. If you omit the end date, the pipeline processes each day through the latest available day.

Dataset mode calculates MAAD statistics by default. MAAD statistics describe the multifractal structure of the observed IPv4 address sets, and they power the address-structure charts. Use `--no-maad` to skip them.

If a command fails, read [Troubleshooting](troubleshooting.md).

## Process coordinated subsets

Repeat `--dataset` for two or more registry entries that describe subsets of one nfcapd tree:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset campus-a \
  --dataset campus-b \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD>
```

Multi mode supports `daily_active_sources` only. Put that selection in every registry entry. The
command infers each root and source configuration from its entry. It does not use a parent or
`source_dataset` relation.

Use the same root and logical source layout for all entries. Use whole local-day dates. Do not pass
`--config`, `--database-path`, partial time bounds, or selection flags in multi mode. The command
uses each registry entry's database path.

The pipeline performs one shared daily eligibility scan and one shared publication scan. It still
needs two physical phases because it must qualify sources over the whole local day first. A missing
required capture makes that local day incomplete for every subset.

Each subset keeps its own product database, identity, transactions, resume state, and MAAD settings.
The active set is independent for each subset, so an overlapping subset may receive the same flow.

### Gate a coordinated run

Before a full MAAD run, gate one local day with temporary output paths. Keep the gate directory
under `data/`: the Docker wrapper mounts that directory at `/workspace/data`. Copy the registry and
rewrite only its `db_path` values; keep the roots, sources, and selections unchanged:

```bash
mkdir -p data
gate_dir=$(mktemp -d data/netflow-gate.XXXXXX)
gate_root=$(realpath "$gate_dir")
gate_date=2025-06-01 # replace with one complete local day
jq --arg dir "$gate_dir" \
  '(if type == "array" then {datasets: .}
    elif type == "object" then .
    else error("registry must be an array or an object with a datasets array")
    end)
   | (.datasets // .) as $entries
   | if ($entries | type) != "array" then
       error("registry must be an array or an object with a datasets array")
     elif ($entries | length) == 0 then
       error("registry cannot be empty")
     elif any($entries[]; type != "object") then
       error("registry entries must be objects")
     else
       $entries
       | to_entries
       | map(.value.db_path = ($dir + "/" + ((.key + 1) | tostring) + ".sqlite") | .value)
     end' \
  datasets.json >"$gate_dir/datasets.json"

registry_db_paths() {
  local registry=$1
  jq -er '
    (if type == "array" then {datasets: .}
     elif type == "object" then .
     else error("registry must be an array or an object with a datasets array")
     end)
    | (.datasets // .) as $entries
    | if ($entries | type) != "array" then
        error("registry must be an array or an object with a datasets array")
      elif any($entries[]; type != "object") then
        error("registry entries must be objects")
      else
        ["campus-a", "campus-b"][] as $dataset_id
        | ($entries | map(select(.dataset_id == $dataset_id))) as $matches
        | if ($matches | length) != 1 then
            error("registry must contain exactly one entry for " + $dataset_id)
          elif (($matches[0].db_path | type) != "string") then
            error("db_path for " + $dataset_id + " must be a string")
          elif ($matches[0].db_path | length) == 0 then
            error("db_path for " + $dataset_id + " cannot be empty")
          else
            $matches[0].db_path
          end
      end
  ' "$registry"
}

mapfile -t gate_db_paths < <(registry_db_paths "$gate_dir/datasets.json")
if (( ${#gate_db_paths[@]} != 2 )); then
  echo "the gate requires campus-a and campus-b database paths" >&2
  exit 1
fi
for path in "${gate_db_paths[@]}"; do
  resolved=$(realpath -m -- "$path")
  case "$resolved" in
    "$gate_root"/*.sqlite) ;;
    *) echo "temporary database escaped gate directory: $path" >&2; exit 1 ;;
  esac
done
```

Run the same MAAD-enabled native command twice for one complete local day, saving separate cold and
no-op resource/profile logs. The cold invocation checks positive publication cardinality; the
second invocation checks the resume path and zero new publications. Record the cgroup aggregate
peak, elapsed wall time, every `netflow_db::profile` phase, and the byte sizes of each temporary
SQLite output and its `-wal` file. GNU `time -v` still writes its per-process
`Maximum resident set size` for diagnostics, but that value is not a gate: it is not the sum of
the pipeline's concurrent nfdump children.

The native gate puts the whole pipeline process tree in a transient per-user systemd scope. Its
16 GiB `MemoryMax` (`16777216` KiB) is an aggregate cgroup-v2 limit, and the gate reads the scope's
`memory.peak` and fails closed if peak accounting or the cgroup's OOM counters cannot be read.
This leaves a safe margin below roughly 20 GiB available on Barbera without requiring root. Set
`NETFLOW_GATE_MAX_MEMORY_KIB`, `NETFLOW_GATE_COLD_MAX_ELAPSED_SECONDS`,
`NETFLOW_GATE_FULL_COLD_MAX_ELAPSED_SECONDS`, `NETFLOW_GATE_FULL_NOOP_MAX_ELAPSED_SECONDS`, or
`NETFLOW_GATE_SPACE_HEADROOM_PERCENT` before running the block to change the ceilings. The
full-cold budget defaults to 30 days (`2592000` seconds); the gate conservatively projects two
times the measured one-day cold elapsed time across 394 days before launching it. The space
headroom defaults to 100% (a 2x projection). These positive-integer defaults are fail-closed:
raise them explicitly only when the host and run window justify it.
The gate refuses hosts without cgroup v2 or a usable user systemd manager; it does not silently
fall back to GNU `time` or an incomplete process-tree RSS sample.

Run this Bash block from that Nix shell; it keeps the one-day and full-history phases in the same
gated session:

```bash
set -euo pipefail
pipeline=(./scripts/netflow-db.sh)
time_bin="$(type -P time || true)"
if [[ -z "$time_bin" ]] || ! "$time_bin" -v true >/dev/null 2>&1; then
  echo "GNU time with -v is required on PATH; enter nix-shell shell.nix" >&2
  exit 1
fi
systemd_run_bin="$(type -P systemd-run || true)"
systemctl_bin="$(type -P systemctl || true)"
true_bin="$(type -P true || true)"
sleep_bin="$(type -P sleep || true)"
if [[ -z "$systemd_run_bin" || -z "$systemctl_bin" || -z "$true_bin" ]] ||
  ! [[ "$(stat -fc %T -- /sys/fs/cgroup 2>/dev/null || true)" == cgroup2fs ]] ||
  ! "$systemd_run_bin" --user --scope --quiet -p MemoryMax=64M "$true_bin" >/dev/null 2>&1; then
  echo "a running user systemd manager with cgroup v2 is required for the aggregate memory gate" >&2
  exit 1
fi
runtime_probe_unit="netflow-gate-runtime-probe-$$.scope"
if [[ -z "$sleep_bin" ]] ||
  "$systemd_run_bin" --user --scope --quiet --collect --unit="$runtime_probe_unit" \
    -p MemoryMax=64M -p RuntimeMaxSec=1s -- "$sleep_bin" 5 >/dev/null 2>&1; then
  echo "a user-systemd scope with RuntimeMaxSec is required for the elapsed-time gate" >&2
  exit 1
fi
max_memory_kib_limit="${NETFLOW_GATE_MAX_MEMORY_KIB:-16777216}"
cold_elapsed_limit_seconds="${NETFLOW_GATE_COLD_MAX_ELAPSED_SECONDS:-1800}"
full_cold_elapsed_limit_seconds="${NETFLOW_GATE_FULL_COLD_MAX_ELAPSED_SECONDS:-2592000}"
full_noop_elapsed_limit_seconds="${NETFLOW_GATE_FULL_NOOP_MAX_ELAPSED_SECONDS:-1800}"
space_headroom_percent="${NETFLOW_GATE_SPACE_HEADROOM_PERCENT:-100}"
if ! [[ "$max_memory_kib_limit" =~ ^[1-9][0-9]*$ ]] ||
  ! [[ "$cold_elapsed_limit_seconds" =~ ^[1-9][0-9]*$ ]] ||
  ! [[ "$full_cold_elapsed_limit_seconds" =~ ^[1-9][0-9]*$ ]] ||
  ! [[ "$full_noop_elapsed_limit_seconds" =~ ^[1-9][0-9]*$ ]] ||
  ! [[ "$space_headroom_percent" =~ ^[1-9][0-9]*$ ]]; then
  echo "native gate ceilings must be positive integers" >&2
  exit 1
fi

parse_elapsed_seconds() {
  awk '
    /^[[:space:]]*Elapsed \(wall clock\) time \(h:mm:ss or m:ss\):[[:space:]]*/ {
      value = $0
      sub(/^.*\):[[:space:]]*/, "", value)
      gsub(/[[:space:]]/, "", value)
      matches++
      if (value !~ /^[0-9]+:[0-9][0-9](:[0-9][0-9])?([.][0-9]+)?$/) {
        invalid = 1
        next
      }
      fields = split(value, part, ":")
      if (fields == 2) {
        if (part[2] + 0 >= 60) invalid = 1
        else seconds = (part[1] * 60) + part[2]
      } else if (fields == 3) {
        if (part[2] + 0 >= 60 || part[3] + 0 >= 60) invalid = 1
        else seconds = (part[1] * 3600) + (part[2] * 60) + part[3]
      } else {
        invalid = 1
      }
    }
    END {
      if (matches != 1 || invalid) exit 1
      printf "%.6f\n", seconds
    }
  ' "$1"
}

assert_resources() {
  local log_name=$1
  local elapsed_limit_seconds=$2
  local log_path="$gate_dir/$log_name"
  local peak_bytes peak_kib elapsed_seconds
  if ! peak_bytes="$(cat "$gate_dir/$log_name.cgroup-peak-bytes" 2>/dev/null)" ||
    ! [[ "$peak_bytes" =~ ^[1-9][0-9]*$ ]]; then
    echo "missing or malformed aggregate cgroup peak in $log_path" >&2
    return 1
  fi
  if ! elapsed_seconds="$(parse_elapsed_seconds "$log_path")"; then
    echo "missing or malformed elapsed wall time in $log_path" >&2
    return 1
  fi
  peak_kib=$(( (peak_bytes + 1023) / 1024 ))
  if (( peak_bytes > max_memory_kib_limit * 1024 )); then
    echo "aggregate memory limit exceeded in $log_path: ${peak_kib} KiB > ${max_memory_kib_limit} KiB" >&2
    return 1
  fi
  if ! awk -v actual="$elapsed_seconds" -v limit="$elapsed_limit_seconds" '
    BEGIN {
      if (actual !~ /^[0-9]+([.][0-9]+)?$/ || actual > limit) exit 1
    }
  '; then
    echo "elapsed limit exceeded in $log_path: ${elapsed_seconds}s > ${elapsed_limit_seconds}s" >&2
    return 1
  fi
  printf 'Resource gate %s: %s KiB aggregate cgroup peak, %ss elapsed\n' \
    "$log_name" "$peak_kib" "$elapsed_seconds"
}

stop_gate_unit() {
  local unit=$1
  "$systemctl_bin" --user stop "$unit" >/dev/null 2>&1 || true
}

gate_pid_is_running() {
  local pid=$1
  local state
  if ! [[ "$pid" =~ ^[1-9][0-9]*$ ]] ||
    ! state="$(awk '$1 == "State:" { print $2; exit }' "/proc/$pid/status" 2>/dev/null)"; then
    return 1
  fi
  [[ "$state" =~ ^[A-Z]$ && "$state" != Z ]]
}

monitor_cgroup_peak() {
  local cgroup_dir=$1
  local peak_path=$2
  local launch_pid=$3
  local unit=$4
  local peak_bytes events_values oom oom_kill oom_group_kill saw_sample=0
  while true; do
    if [[ ! -r "$cgroup_dir/memory.peak" || ! -r "$cgroup_dir/memory.events" ]]; then
      if gate_pid_is_running "$launch_pid"; then
        stop_gate_unit "$unit"
        return 1
      fi
      break
    fi
    if ! peak_bytes="$(cat "$cgroup_dir/memory.peak")" ||
      ! [[ "$peak_bytes" =~ ^[0-9]+$ ]]; then
      if gate_pid_is_running "$launch_pid"; then
        stop_gate_unit "$unit"
        return 1
      fi
      break
    fi
    if ! events_values="$(awk '
      $1 == "oom" {
        oom_count++
        if (NF != 2 || $2 !~ /^[0-9]+$/) invalid = 1
        else oom_value = $2
        next
      }
      $1 == "oom_kill" {
        oom_kill_count++
        if (NF != 2 || $2 !~ /^[0-9]+$/) invalid = 1
        else oom_kill_value = $2
        next
      }
      $1 == "oom_group_kill" {
        oom_group_kill_count++
        if (NF != 2 || $2 !~ /^[0-9]+$/) invalid = 1
        else oom_group_kill_value = $2
        next
      }
      END {
        if (invalid || oom_count != 1 || oom_kill_count != 1 || oom_group_kill_count != 1) exit 1
        printf "%s %s %s\n", oom_value, oom_kill_value, oom_group_kill_value
      }
    ' "$cgroup_dir/memory.events")"; then
      if gate_pid_is_running "$launch_pid"; then
        stop_gate_unit "$unit"
        return 1
      fi
      break
    fi
    read -r oom oom_kill oom_group_kill <<<"$events_values"
    if [[ "$oom" != 0 || "$oom_kill" != 0 || "$oom_group_kill" != 0 ]]; then
      if gate_pid_is_running "$launch_pid"; then
        stop_gate_unit "$unit"
      fi
      return 1
    fi
    printf '%s\n' "$peak_bytes" >"$peak_path"
    saw_sample=1
    gate_pid_is_running "$launch_pid" || break
    sleep 0.1
  done
  (( saw_sample == 1 ))
}

gate_active_unit=
gate_active_launch_pid=
gate_active_monitor_pid=
cleanup_gate_processes() {
  local unit=$gate_active_unit
  local launch_pid=$gate_active_launch_pid
  local monitor_pid=$gate_active_monitor_pid
  gate_active_unit=
  gate_active_launch_pid=
  gate_active_monitor_pid=

  if [[ -n "$unit" ]]; then
    stop_gate_unit "$unit"
  fi
  if [[ -n "$monitor_pid" ]] && gate_pid_is_running "$monitor_pid"; then
    kill "$monitor_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$monitor_pid" ]]; then
    wait "$monitor_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$launch_pid" ]]; then
    wait "$launch_pid" >/dev/null 2>&1 || true
  fi
}

gate_error() {
  local status=$?
  cleanup_gate_processes
  trap - ERR
  exit "$status"
}

gate_signal() {
  local status=$1
  cleanup_gate_processes
  trap - ERR
  exit "$status"
}

trap gate_error ERR
trap 'gate_signal 129' HUP
trap 'gate_signal 130' INT
trap 'gate_signal 143' TERM
trap cleanup_gate_processes EXIT

run_gate() {
  local log_name=$1
  local published_pattern=$2
  local start_date="${3:-$gate_date}"
  local end_date="${4:-$gate_date}"
  local registry="${5:-$gate_dir/datasets.json}"
  local elapsed_limit_seconds="${6:-$cold_elapsed_limit_seconds}"
  local unit="netflow-gate-${log_name%.log}-$$.scope"
  local launch_pid pipeline_status monitor_pid monitor_status control_group cgroup_dir=
  gate_active_unit=$unit
  RUST_LOG=netflow_db::profile=info "$time_bin" -v \
    "$systemd_run_bin" --user --scope --quiet --collect --unit="$unit" \
    -p "MemoryMax=${max_memory_kib_limit}K" \
    -p "RuntimeMaxSec=${elapsed_limit_seconds}s" -- \
    "${pipeline[@]}" pipeline \
      --datasets "$registry" \
      --dataset campus-a --dataset campus-b \
      --start-date "$start_date" --end-date "$end_date" \
      --require-complete \
    >"$gate_dir/$log_name" 2>&1 &
  launch_pid=$!
  gate_active_launch_pid=$launch_pid
  for _ in {1..300}; do
    control_group="$("$systemctl_bin" --user show "$unit" --property=ControlGroup --value 2>/dev/null || true)"
    if [[ "$control_group" == /* && "$control_group" != *..* &&
      "$control_group" != *$'\n'* && "$control_group" != *$'\t'* ]]; then
      cgroup_dir="/sys/fs/cgroup$control_group"
      if [[ -r "$cgroup_dir/memory.peak" && -r "$cgroup_dir/memory.events" ]]; then
        break
      fi
      cgroup_dir=
    fi
    gate_pid_is_running "$launch_pid" || break
    sleep 0.1
  done
  if [[ -z "$cgroup_dir" ]]; then
    echo "could not locate aggregate cgroup for $unit" >&2
    cleanup_gate_processes
    return 1
  fi
  monitor_cgroup_peak "$cgroup_dir" "$gate_dir/$log_name.cgroup-peak-bytes" "$launch_pid" "$unit" &
  monitor_pid=$!
  gate_active_monitor_pid=$monitor_pid
  if wait "$monitor_pid"; then monitor_status=0; else monitor_status=$?; fi
  if (( monitor_status != 0 )); then
    stop_gate_unit "$unit"
  fi
  if wait "$launch_pid"; then pipeline_status=0; else pipeline_status=$?; fi
  gate_active_unit=
  gate_active_launch_pid=
  gate_active_monitor_pid=
  if (( pipeline_status != 0 || monitor_status != 0 )); then
    echo "aggregate memory gate failed for $log_name (pipeline=$pipeline_status monitor=$monitor_status)" >&2
    return 1
  fi
  cat "$gate_dir/$log_name"
  grep -Eq '^Five-minute coverage: [1-9][0-9]* complete' "$gate_dir/$log_name"
  grep -Eq '^Five-minute coverage: [1-9][0-9]* complete, 0 partial, 0 unknown$' \
    "$gate_dir/$log_name"
  grep -Eq "$published_pattern" "$gate_dir/$log_name"
  assert_resources "$log_name" "$elapsed_limit_seconds"
}
run_gate one-day-cold.log '^Published five-minute buckets: [1-9][0-9]*$' \
  "$gate_date" "$gate_date" "$gate_dir/datasets.json" "$cold_elapsed_limit_seconds"

mapfile -t destination_db_paths < <(registry_db_paths datasets.json)
if (( ${#destination_db_paths[@]} != 2 )); then
  echo "the gate requires campus-a and campus-b database paths" >&2
  exit 1
fi

destination_related_paths() {
  local database=$1
  local resolved path parent name suffix
  if ! resolved="$(realpath -m -- "$database")"; then
    echo "could not resolve final database path: $database" >&2
    return 1
  fi
  for path in "$database" "$resolved"; do
    parent=${path%/*}
    [[ "$parent" == "$path" ]] && parent=.
    name=${path##*/}
    printf '%s\n' "$path"
    for suffix in -journal -wal -shm; do
      printf '%s%s\n' "$path" "$suffix"
    done
    printf '%s/.%s.operation.lock\n' "$parent" "$name"
  done
}

assert_fresh_destination_paths() {
  local database=$1
  local related
  while IFS= read -r related; do
    if [[ -e "$related" || -L "$related" ]]; then
      echo "full-cold requires a fresh final database path; found existing database, SQLite sidecar, or operation lock: $related" >&2
      echo "Remove that path and retry; the final destination must be fresh before full-cold: $database" >&2
      return 1
    fi
  done < <(destination_related_paths "$database")
}

for db_path in "${destination_db_paths[@]}"; do
  assert_fresh_destination_paths "$db_path"
done

for db_path in "${gate_db_paths[@]}"; do
  python3 - "$db_path" <<'PY'
import sqlite3
import sys

database = sys.argv[1]
with sqlite3.connect(database, timeout=30) as connection:
    result = connection.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
if result is None or result[0] != 0:
    raise SystemExit(f"WAL checkpoint was busy for {database!r}: {result!r}")
PY
  if [[ -e "$db_path-wal" && "$(stat -c %s -- "$db_path-wal")" != 0 ]]; then
    echo "WAL was not truncated for $db_path" >&2
    exit 1
  fi
done

python3 - "$space_headroom_percent" \
  "${gate_db_paths[0]}" "${gate_db_paths[1]}" \
  "${destination_db_paths[0]}" "${destination_db_paths[1]}" <<'PY' | tee "$gate_dir/space.log"
import os
import shutil
import sys
from collections import defaultdict

headroom_percent = int(sys.argv[1])
source_paths = sys.argv[2:4]
destination_paths = sys.argv[4:6]
if headroom_percent < 1 or len(source_paths) != 2 or len(destination_paths) != 2:
    raise SystemExit("invalid space-gate arguments")

history_days = 394
factor = history_days * (100 + headroom_percent)
projections = []
for source, destination in zip(source_paths, destination_paths):
    source_size = os.stat(source).st_size
    if source_size < 1:
        raise SystemExit(f"one-day database is empty: {source!r}")
    destination_realpath = os.path.realpath(destination)
    destination_parent = os.path.dirname(destination_realpath) or "."
    if not os.path.isdir(destination_parent):
        raise SystemExit(f"destination directory does not exist: {destination_parent!r}")
    projected = (source_size * factor + 99) // 100
    projections.append((destination_parent, destination, source_size, projected))

by_filesystem = defaultdict(list)
for projection in projections:
    by_filesystem[os.stat(projection[0]).st_dev].append(projection)

for device, files in by_filesystem.items():
    free = shutil.disk_usage(files[0][0]).free
    required = sum(item[3] for item in files)
    print(f"filesystem {device}: {required} projected bytes required, {free} bytes free")
    for _, destination, source_size, projected in files:
        print(f"  {destination}: {source_size} one-day bytes -> {projected} projected bytes")
    if required > free:
        raise SystemExit(f"insufficient free space on filesystem {device}")
PY
run_gate one-day-noop.log '^Published five-minute buckets: 0$' \
  "$gate_date" "$gate_date" "$gate_dir/datasets.json" "$cold_elapsed_limit_seconds"

for path in "$gate_dir"/*.sqlite "$gate_dir"/*.sqlite-wal; do
  if [[ -e "$path" ]]; then
    stat --printf='%n %s bytes\n' "$path"
  fi
done | tee "$gate_dir/sizes.log"

one_day_cold_elapsed_seconds="$(parse_elapsed_seconds "$gate_dir/one-day-cold.log")"
full_cold_projection_seconds="$(awk -v one_day="$one_day_cold_elapsed_seconds" '
  BEGIN {
    if (one_day !~ /^[0-9]+([.][0-9]+)?$/ || one_day <= 0) exit 1
    projected = one_day * 394 * 2
    rounded = int(projected)
    if (projected > rounded) rounded++
    printf "%d\n", rounded
  }
')"
if ! [[ "$full_cold_projection_seconds" =~ ^[1-9][0-9]*$ ]] ||
  ! awk -v projected="$full_cold_projection_seconds" -v limit="$full_cold_elapsed_limit_seconds" '
    BEGIN {
      if (projected !~ /^[0-9]+$/ || limit !~ /^[0-9]+$/ || projected > limit) exit 1
    }
  '; then
  echo "projected full-cold elapsed time exceeds its configured budget: ${full_cold_projection_seconds}s > ${full_cold_elapsed_limit_seconds}s" >&2
  exit 1
fi
printf 'Full cold elapsed projection: %ss (2x %s-day one-day cold elapsed of %ss); budget: %ss\n' \
  "$full_cold_projection_seconds" 394 "$one_day_cold_elapsed_seconds" \
  "$full_cold_elapsed_limit_seconds"

run_gate full-cold.log '^Published five-minute buckets: [1-9][0-9]*$' \
  2025-06-01 2026-06-29 datasets.json "$full_cold_elapsed_limit_seconds"
run_gate full-noop.log '^Published five-minute buckets: 0$' \
  2025-06-01 2026-06-29 datasets.json "$full_noop_elapsed_limit_seconds"
```

The block runs four distinct assertions: `one-day-cold.log` requires positive publication from
the temporary databases, `one-day-noop.log` requires zero new publication while retaining complete
coverage, `full-cold.log` requires positive publication from the real registry, and
`full-noop.log` requires zero new publication over the complete history. Every run is covered by
the same aggregate cgroup memory monitor and process-tree-safe `RuntimeMaxSec` limit. The full
cold run is launched only after the one-day elapsed time has been parsed and its conservative
2x/394-day projection fits the explicit full-cold budget; the full no-op remains after that build.

The full-history no-op asserts zero published buckets, positive complete coverage with no partial
or unknown buckets, the same 16 GiB-by-default aggregate cgroup memory ceiling, and its separate
elapsed ceiling. The one-day space projection is deliberately conservative: it scales each
checkpointed output by 394 days and adds the configured headroom for SQLite growth and WAL space,
then compares the combined requirement with free bytes on the actual destination filesystems
before the full build is launched.

The Docker wrapper is suitable for a functional smoke check, not this RSS gate. It does not expose
a container memory limit or cgroup peak sampler, and timing the wrapper measures the local Docker
client rather than the process inside the container. If you use Docker for the smoke check, keep
the cold and no-op logs separate and keep the capture root absolute:

```bash
set -euo pipefail
docker_gate() {
  local log_name=$1
  local published_pattern=$2
  ./scripts/netflow-db-docker.sh --capture-root /absolute/path/to/captures pipeline \
    --datasets "$gate_dir/datasets.json" \
    --dataset campus-a --dataset campus-b \
    --start-date "$gate_date" --end-date "$gate_date" \
    --require-complete \
    2>&1 | tee "$gate_dir/$log_name"
  grep -Eq '^Five-minute coverage: [1-9][0-9]* complete' "$gate_dir/$log_name"
  grep -Eq "$published_pattern" "$gate_dir/$log_name"
}
docker_gate docker-cold.log '^Published five-minute buckets: [1-9][0-9]*$'
docker_gate docker-noop.log '^Published five-minute buckets: 0$'
```

Both the temporary registry and SQLite outputs work in this Docker smoke check because their paths
are relative to the mounted repository `data/` directory.

## Select flows

Selection conditions use AND logic. The IP prefix can match the source endpoint or the destination endpoint.

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
  --database-path data/example-public/netflow.sqlite \
  --ip-prefix 192.0.2.0/24 \
  --src-visibility literal
```

A selected population is a different database product. Thus, selection options require an explicit `--database-path`.
Dataset registry entries may instead persist a `selection` beside their dedicated `db_path`; dataset
mode applies that selection automatically.

Available selection options are:

- `--ip-prefix`
- `--daily-active-sources`
- `--src-visibility literal|anonymized`
- `--dst-visibility literal|anonymized`

`--daily-active-sources` applies the fixed active-user definition used to choose the UOregon
candidate subnets. It requires an IPv4 `/16` and cannot be combined with the visibility flags:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
  --database-path data/example-active/netflow.sqlite \
  --ip-prefix 0.220.0.0/16 \
  --daily-active-sources
```

For each complete local day, the pipeline sums qualifying traffic by exact source address across
each unique physical capture member. A source is active when it has at least 3 flows, 20 packets,
and 2,000 bytes that day. Qualifying traffic is IPv4 TCP or UDP from an anonymized source in the
target `/16`, with source port at least 1024. Destination ports and TCP flags are unrestricted.
Only that qualifying traffic from active sources is published.

This mode supports exactly one `nfcapd_tree` input and whole local days. A day missing any expected
physical capture is skipped rather than published as zero. If input evidence changes after a day
was published, rebuild the whole day with `--force`; a single five-minute repair is not safe because
it can change the active-source set for every bucket in that day.

## Use a pipeline configuration

Configuration mode supports CSV input, nfcapd input, and mixed input. Explicit `csv` and `nfcapd` inputs and `csv_tree` and `nfcapd_tree` discovery inputs go in the top-level `inputs` list.

```bash
./scripts/netflow-db.sh pipeline \
  --config /path/to/pipeline.json \
  --database-path data/example/netflow.sqlite
```

Put flow selection in the top-level `selection` object:

```json
{
  "selection": {
    "ip_prefix": "192.0.2.0/24",
    "src_visibility": "literal",
    "dst_visibility": "anonymized"
  },
  "inputs": []
}
```

The equivalent active-source selection is deliberately a named policy rather than configurable
thresholds:

```json
{
  "selection": {
    "kind": "daily_active_sources",
    "ip_prefix": "0.220.0.0/16"
  },
  "inputs": [
    {
      "input_kind": "nfcapd_tree",
      "root_path": "/path/to/captures",
      "source_ids": ["gateway-a", "gateway-b"],
      "start_date": "2025-06-01",
      "end_date": "2026-06-29"
    }
  ]
}
```

On the native path, nfcapd input needs the fork path: set the top-level `"nfdump"` value to `"target/nfdump/libexec/nfdump"`, or pass `--nfdump` when the configuration does not set it.

## Common options

| Option            | Purpose                                    |
| ----------------- | ------------------------------------------ |
| `--database-path` | Changes the SQLite output path.            |
| `--datasets`      | Reads a different dataset registry file.   |
| `--start-time`    | Sets the start of a half-open time window. |
| `--end-time`      | Sets the end of a half-open time window.   |
| `--nfdump`        | Names the nfdump executable.               |
| `--force`         | Rewrites selected nfcapd buckets.          |
| `--no-maad`       | Skips the MAAD statistics.                 |

Time limits must align with local-day boundaries.

## Verify the output

Run the compatibility check after the pipeline finishes:

```bash
./scripts/netflow-db.sh verify data/example/netflow.sqlite \
  --dataset-id example \
  --require-data \
  --require-maad-data \
  --require-processed \
  --require-rollup-parity \
  --require-no-raw-ip
```

The command prints an `OK` line when the database is compatible. A failed requirement returns a nonzero exit status.

For pipeline identity and export rules, read the [pipeline contract](../code/pipeline-contract.md).
