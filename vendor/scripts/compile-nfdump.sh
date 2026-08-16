#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
nfdump_source="$repo_root/vendor/nfdump"
nfdump_target="$repo_root/target/nfdump"
nfdump_build="$nfdump_target/build"
nfdump_stage="$nfdump_target/libexec/nfdump"

if [[ ! -d "$nfdump_source" || ! -e "$nfdump_source/.git" ]]; then
	echo "vendor/nfdump is not initialized; run: git submodule update --init --recursive" >&2
	exit 1
fi

for required_file in Makefile.am configure.ac autogen.sh; do
	if [[ ! -f "$nfdump_source/$required_file" ]]; then
		echo "vendor/nfdump is incomplete (missing $required_file); run: git submodule update --init --recursive" >&2
		exit 1
	fi
done

missing_tools=()
for tool in git make autoreconf autoconf autoheader automake aclocal libtoolize pkg-config python3 tar; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		missing_tools+=("$tool")
	fi
done
if ! command -v flex >/dev/null 2>&1 && ! command -v lex >/dev/null 2>&1; then
	missing_tools+=("flex or lex")
fi
if ! command -v bison >/dev/null 2>&1 && ! command -v yacc >/dev/null 2>&1; then
	missing_tools+=("bison or yacc")
fi
if ! command -v gcc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1; then
	missing_tools+=("gcc or clang")
fi
if ((${#missing_tools[@]} > 0)); then
	printf 'missing nfdump build dependencies: %s\n' "${missing_tools[*]}" >&2
	echo "install the Autotools, lexer/parser, compiler, and pkg-config dependencies, or use a development shell." >&2
	exit 1
fi

if ! nfdump_commit="$(git -C "$nfdump_source" rev-parse --verify HEAD^{commit})"; then
	echo "vendor/nfdump is not a valid initialized git submodule; run: git submodule update --init --recursive" >&2
	exit 1
fi
if [[ -n "$(git -C "$nfdump_source" status --porcelain)" ]]; then
	echo "vendor/nfdump has local changes; the production helper must be built from the pinned submodule commit" >&2
	exit 1
fi

mkdir -p "$nfdump_target" "$nfdump_target/libexec"
rm -rf "$nfdump_build"
mkdir -p "$nfdump_build/source"

echo "Preparing nfdump $nfdump_commit in $nfdump_build"
git -C "$nfdump_source" archive --format=tar "$nfdump_commit" |
	tar -xf - -C "$nfdump_build/source"

(
	cd "$nfdump_build/source"
	./autogen.sh
)

build_cflags="${CFLAGS:-} -O3 -DNDEBUG -g0"
(
	cd "$nfdump_build"
	CFLAGS="$build_cflags" "$nfdump_build/source/configure" \
		--srcdir="$nfdump_build/source" \
		--prefix="$nfdump_target" \
		--disable-shared \
		--enable-static \
		--disable-sflow \
		--disable-nfpcapd \
		--disable-ftconv \
		--disable-nfprofile \
		--disable-nftrack
)

# This release's Makefiles use relative include paths that assume an in-tree
# checkout. Symlink the pinned source files into the disposable build tree so
# those paths resolve without modifying the submodule.
while IFS= read -r -d '' source_file; do
	relative_file="${source_file#"$nfdump_build/source/"}"
	build_file="$nfdump_build/$relative_file"
	mkdir -p "$(dirname "$build_file")"
	if [[ ! -e "$build_file" ]]; then
		ln -s "$source_file" "$build_file"
	fi
done < <(find "$nfdump_build/source/src" -type f -print0)

build_jobs="${NFDUMP_BUILD_JOBS:-1}"
if [[ ! "$build_jobs" =~ ^[1-9][0-9]*$ ]]; then
	echo "NFDUMP_BUILD_JOBS must be a positive integer (got: $build_jobs)" >&2
	exit 1
fi
make -C "$nfdump_build" -j"$build_jobs"
make -C "$nfdump_build/src/test" -j1 check TESTS=runatlantis.sh

built_nfdump="$nfdump_build/src/nfdump/.libs/nfdump"
if [[ ! -x "$built_nfdump" ]]; then
	built_nfdump="$nfdump_build/src/nfdump/nfdump"
fi
if [[ ! -x "$built_nfdump" ]]; then
	echo "nfdump build completed without an executable at src/nfdump/.libs/nfdump" >&2
	exit 1
fi

smoke_dir="$nfdump_build/smoke"
mkdir -p "$smoke_dir"
nfgen_bin="$nfdump_build/src/test/.libs/nfgen"
if [[ ! -x "$nfgen_bin" ]]; then
	nfgen_bin="$nfdump_build/src/test/nfgen"
fi
if [[ ! -x "$nfgen_bin" ]]; then
	echo "nfdump smoke fixture generator was not built at src/test/.libs/nfgen" >&2
	exit 1
fi
(
	cd "$smoke_dir"
	"$nfgen_bin" >/dev/null
)
smoke_capture="$smoke_dir/dummy_flows.nf"
if [[ ! -s "$smoke_capture" ]]; then
	echo "nfdump smoke fixture generator did not create $smoke_capture" >&2
	exit 1
fi

version_output="$smoke_dir/version.txt"
version_text=""
if "$built_nfdump" --version >"$version_output" 2>&1; then
	version_candidate="$(<"$version_output")"
	if [[ "$version_candidate" =~ [0-9]+\.[0-9]+ ]]; then
		version_text="$version_candidate"
		echo "Smoke check passed: nfdump --version"
	fi
fi

if [[ -z "$version_text" ]]; then
	if "$built_nfdump" -V >"$version_output" 2>&1; then
		version_candidate="$(<"$version_output")"
		if [[ "$version_candidate" =~ [0-9]+\.[0-9]+ ]]; then
			echo "Smoke check passed: nfdump -V (this pinned source does not expose --version yet)"
		else
			echo "nfdump version smoke check did not print a version:" >&2
			cat "$version_output" >&2
			exit 1
		fi
	else
		echo "nfdump version smoke check failed:" >&2
		cat "$version_output" >&2
		exit 1
	fi
fi

atlantis_stream="$smoke_dir/atlantis.bin"
atlantis_error="$smoke_dir/atlantis.stderr"
if "$built_nfdump" -G none -r "$smoke_capture" -q -o atlantis 'host 203.0.113.255' >"$atlantis_stream" 2>"$atlantis_error"; then
	stream_size="$(wc -c <"$atlantis_stream")"
	if ((stream_size != 16)); then
		echo "nfdump -o atlantis emitted $stream_size bytes for a no-match stream; expected exactly a 12-byte header and 4-byte zero terminator" >&2
		exit 1
	fi

	expected_header="$smoke_dir/expected-header.bin"
	actual_header="$smoke_dir/actual-header.bin"
	printf 'ATLNFLOW\001\000\110\000' >"$expected_header"
	head -c 12 "$atlantis_stream" >"$actual_header"
	if ! cmp -s "$expected_header" "$actual_header"; then
		echo "nfdump -o atlantis emitted an unexpected 12-byte header" >&2
		exit 1
	fi

	expected_terminator="$smoke_dir/expected-terminator.bin"
	actual_terminator="$smoke_dir/actual-terminator.bin"
	printf '\0\0\0\0' >"$expected_terminator"
	tail -c 4 "$atlantis_stream" >"$actual_terminator"
	if ! cmp -s "$expected_terminator" "$actual_terminator"; then
		echo "nfdump -o atlantis omitted the zero terminator" >&2
		exit 1
	fi
	echo "Smoke check passed: nfdump -o atlantis no-match stream ($stream_size bytes)"
else
	echo "nfdump -o atlantis smoke check failed:" >&2
	cat "$atlantis_error" >&2
	exit 1
fi

staged_temp="$nfdump_stage.tmp"
install -m 0755 "$built_nfdump" "$staged_temp"
mv "$staged_temp" "$nfdump_stage"
echo "Staged validated binary at $nfdump_stage"
