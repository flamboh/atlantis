#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${NETFLOW_DB_DOCKER_IMAGE:-atlantis-netflow-db:local}"

usage() {
	cat >&2 <<'EOF'
usage: scripts/netflow-db-docker.sh [--build] [--capture-root PATH]... [--] <netflow-db args...>

  --build              Rebuild the local image before running the command.
  --capture-root PATH  Mount an absolute capture root read-only at the same path.

Container options must appear before the netflow-db command.
EOF
}

force_build=0
capture_roots=()
while (($# > 0)); do
	case "$1" in
	--build)
		force_build=1
		shift
		;;
	--capture-root)
		if (($# < 2)); then
			echo "--capture-root requires a path" >&2
			usage
			exit 2
		fi
		capture_roots+=("$2")
		shift 2
		;;
	--)
		shift
		break
		;;
	*)
		break
		;;
	esac
done

if (($# == 0)); then
	usage
	exit 2
fi

if ! command -v docker >/dev/null 2>&1; then
	echo "docker is required; install Docker Engine or Docker Desktop" >&2
	exit 1
fi

for capture_root in "${capture_roots[@]}"; do
	if [[ "$capture_root" != /* || "$capture_root" == "/" ]]; then
		echo "capture root must be an absolute directory other than /: $capture_root" >&2
		exit 2
	fi
	if [[ "$capture_root" == *,* ]]; then
		echo "capture root cannot contain a comma: $capture_root" >&2
		exit 2
	fi
	if [[ ! -d "$capture_root" ]]; then
		echo "capture root is not a directory: $capture_root" >&2
		exit 2
	fi
done

tree_entry="$(git -C "$ROOT_DIR" ls-tree HEAD -- vendor/nfdump)"
read -r submodule_mode submodule_type nfdump_commit submodule_path <<<"$tree_entry"
if [[ "$submodule_mode" != "160000" || "$submodule_type" != "commit" || "$submodule_path" != "vendor/nfdump" || ! "$nfdump_commit" =~ ^[0-9a-f]{40}$ ]]; then
	echo "unable to read the pinned vendor/nfdump commit from the current Git tree" >&2
	exit 1
fi

if ((force_build)) || ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
	docker build \
		--build-arg "NFDUMP_COMMIT=$nfdump_commit" \
		--tag "$IMAGE" \
		"$ROOT_DIR"
fi

mkdir -p "$ROOT_DIR/data"

docker_args=(
	run
	--rm
	--user "$(id -u):$(id -g)"
	--env HOME=/tmp
	--workdir /workspace
	--mount "type=bind,source=$ROOT_DIR/data,target=/workspace/data"
)

if [[ -f "$ROOT_DIR/datasets.json" ]]; then
	docker_args+=(
		--mount "type=bind,source=$ROOT_DIR/datasets.json,target=/workspace/datasets.json,readonly"
	)
fi

for capture_root in "${capture_roots[@]}"; do
	docker_args+=(
		--mount "type=bind,source=$capture_root,target=$capture_root,readonly"
	)
done

exec docker "${docker_args[@]}" "$IMAGE" "$@"
