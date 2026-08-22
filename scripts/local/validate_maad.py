#!/usr/bin/env python3
"""Compare Rust and Haskell MAAD JSON output for local address-set cases."""

from __future__ import annotations

import argparse
import ipaddress
import json
import math
import subprocess
import sys
from dataclasses import dataclass
from typing import Any, Iterable


DEFAULT_TOLERANCE = 1e-10
MAX_ERROR_DETAILS = 12


@dataclass(frozen=True)
class Case:
    name: str
    path: str


class CaseError(Exception):
    """A user-facing failure for one validation case."""


def nonnegative_float(raw: str) -> float:
    try:
        value = float(raw)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"not a number: {raw!r}") from error
    if not math.isfinite(value) or value < 0:
        raise argparse.ArgumentTypeError("tolerance must be finite and non-negative")
    return value


def parse_case_spec(raw: str) -> Case:
    name, separator, path = raw.partition("=")
    if not separator or not name.strip() or not path:
        raise CaseError(f"case must be NAME=PATH, got {raw!r}")
    name = name.strip()
    if any(character.isspace() for character in name):
        raise CaseError(f"case name must not contain whitespace: {name!r}")
    return Case(name=name, path=path)


def validate_input(path: str) -> None:
    """Check each non-empty line without rewriting or materializing the file."""

    try:
        with open(path, "r", encoding="ascii", errors="strict", newline="") as stream:
            for line_number, raw_line in enumerate(stream, start=1):
                value = raw_line.strip()
                if not value:
                    raise CaseError(f"{path}: line {line_number}: expected an IPv4 address")
                try:
                    address = ipaddress.ip_address(value)
                except ValueError as error:
                    raise CaseError(
                        f"{path}: line {line_number}: invalid IPv4 address {value!r}"
                    ) from error
                if not isinstance(address, ipaddress.IPv4Address):
                    raise CaseError(
                        f"{path}: line {line_number}: expected IPv4, got {value!r}"
                    )
    except CaseError:
        raise
    except (OSError, UnicodeError) as error:
        raise CaseError(f"unable to read input {path!r}: {error}") from error


def compact_error(text: str) -> str:
    detail = " ".join(text.split())
    if len(detail) > 240:
        return detail[:237] + "..."
    return detail


def run_json(command: list[str], label: str) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            check=False,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as error:
        raise CaseError(f"{label} command failed: {error}") from error

    if completed.returncode != 0:
        detail = compact_error(completed.stderr)
        suffix = f": {detail}" if detail else ""
        raise CaseError(f"{label} command exited {completed.returncode}{suffix}")

    try:
        result = json.loads(completed.stdout, parse_constant=_reject_nonfinite_json)
    except (TypeError, ValueError) as error:
        detail = compact_error(completed.stdout)
        raise CaseError(f"{label} emitted invalid JSON{': ' + detail if detail else ''}") from error
    if not isinstance(result, dict):
        raise CaseError(f"{label} JSON root must be an object")
    return result


def _reject_nonfinite_json(value: str) -> None:
    raise ValueError(f"non-finite JSON value {value}")


def exact_integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise CaseError(f"{field} must be an integer")
    return value


def normalized_prefix_lengths(result: dict[str, Any], label: str) -> tuple[int, ...]:
    metadata = result.get("metadata")
    if not isinstance(metadata, dict):
        raise CaseError(f"{label} metadata must be an object")

    # Rust stores the lengths directly. The oracle retains the same information
    # as prefix_counts entries; counts are deliberately not part of this shape.
    if "prefixLengths" in metadata:
        raw_lengths = metadata["prefixLengths"]
    elif "prefix_counts" in metadata:
        raw_counts = metadata["prefix_counts"]
        if not isinstance(raw_counts, list):
            raise CaseError(f"{label} metadata.prefix_counts must be an array")
        raw_lengths = []
        for index, entry in enumerate(raw_counts):
            if not isinstance(entry, dict) or "pl" not in entry:
                raise CaseError(
                    f"{label} metadata.prefix_counts[{index}] must contain pl"
                )
            raw_lengths.append(entry["pl"])
    else:
        raise CaseError(f"{label} metadata has no prefix lengths")

    if not isinstance(raw_lengths, list):
        raise CaseError(f"{label} metadata prefix lengths must be an array")
    return tuple(
        exact_integer(value, f"{label} metadata prefix length {index}")
        for index, value in enumerate(raw_lengths)
    )


def numeric(value: Any, field: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise CaseError(f"{field} must be numeric")
    converted = float(value)
    if not math.isfinite(converted):
        raise CaseError(f"{field} must be finite")
    return converted


def close_enough(left: float, right: float, absolute: float, relative: float) -> bool:
    return abs(left - right) <= absolute + relative * max(abs(left), abs(right))


def compare_rows(
    rust: dict[str, Any],
    haskell: dict[str, Any],
    section: str,
    fields: tuple[str, ...],
    absolute: float,
    relative: float,
) -> list[str]:
    errors: list[str] = []
    rust_rows = rust.get(section)
    haskell_rows = haskell.get(section)
    if not isinstance(rust_rows, list) or not isinstance(haskell_rows, list):
        return [f"{section} must be an array in both results"]
    if len(rust_rows) != len(haskell_rows):
        errors.append(f"{section} row count rust={len(rust_rows)} haskell={len(haskell_rows)}")
    for index, (rust_row, haskell_row) in enumerate(zip(rust_rows, haskell_rows)):
        if not isinstance(rust_row, dict) or not isinstance(haskell_row, dict):
            errors.append(f"{section}[{index}] must be an object in both results")
            continue
        for field in fields:
            try:
                rust_value = numeric(rust_row.get(field), f"rust {section}[{index}].{field}")
                haskell_value = numeric(
                    haskell_row.get(field), f"haskell {section}[{index}].{field}"
                )
            except CaseError as error:
                errors.append(str(error))
                continue
            if not close_enough(rust_value, haskell_value, absolute, relative):
                errors.append(
                    f"{section}[{index}].{field} rust={rust_value:.17g} "
                    f"haskell={haskell_value:.17g}"
                )
            if len(errors) >= MAX_ERROR_DETAILS:
                return errors
    return errors


def compare_results(
    rust: dict[str, Any],
    haskell: dict[str, Any],
    absolute: float,
    relative: float,
) -> tuple[list[str], tuple[int, int, int, int, int]]:
    errors: list[str] = []

    try:
        rust_metadata = rust["metadata"]
        haskell_metadata = haskell["metadata"]
        if not isinstance(rust_metadata, dict) or not isinstance(haskell_metadata, dict):
            raise CaseError("metadata must be an object in both results")
        rust_total = exact_integer(rust_metadata.get("totalAddrs"), "rust metadata.totalAddrs")
        haskell_total = exact_integer(
            haskell_metadata.get("totalAddrs"), "haskell metadata.totalAddrs"
        )
        rust_prefixes = normalized_prefix_lengths(rust, "rust")
        haskell_prefixes = normalized_prefix_lengths(haskell, "haskell")
    except (KeyError, CaseError) as error:
        errors.append(str(error))
        rust_total = haskell_total = -1
        rust_prefixes = haskell_prefixes = ()

    if rust_total != haskell_total:
        errors.append(f"total addresses rust={rust_total} haskell={haskell_total}")
    if rust_prefixes != haskell_prefixes:
        errors.append(f"prefix lengths rust={list(rust_prefixes)} haskell={list(haskell_prefixes)}")

    section_fields = {
        "structure": ("q", "tauTilde", "sd"),
        "spectrum": ("alpha", "f"),
        "dimensions": ("q", "dim"),
    }
    for section, fields in section_fields.items():
        errors.extend(compare_rows(rust, haskell, section, fields, absolute, relative))
        if len(errors) >= MAX_ERROR_DETAILS:
            break

    counts = (
        rust_total,
        len(rust.get("structure", [])) if isinstance(rust.get("structure"), list) else -1,
        len(rust.get("spectrum", [])) if isinstance(rust.get("spectrum"), list) else -1,
        len(rust.get("dimensions", [])) if isinstance(rust.get("dimensions"), list) else -1,
        len(rust_prefixes),
    )
    return errors[:MAX_ERROR_DETAILS], counts


def case_specs(parser: argparse.ArgumentParser, args: argparse.Namespace) -> list[Case]:
    raw_cases = [*args.named_cases, *args.cases]
    if not raw_cases:
        parser.error("provide at least one NAME=PATH case")

    parsed: list[Case] = []
    names: set[str] = set()
    for raw in raw_cases:
        try:
            case = parse_case_spec(raw)
        except CaseError as error:
            parser.error(str(error))
        if case.name in names:
            parser.error(f"duplicate case name: {case.name!r}")
        names.add(case.name)
        parsed.append(case)
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Compare Rust and Haskell MAAD JSON for local IPv4 address files.",
        epilog=(
            "Cases are NAME=PATH; paths are passed unchanged to both binaries. "
            "Example: %(prog)s --rust target/release/netflow-db "
            "--haskell /path/to/MAAD sample=/tmp/addresses.txt"
        ),
    )
    parser.add_argument("--rust", required=True, help="Rust netflow-db binary")
    parser.add_argument("--haskell", required=True, help="Haskell MAAD binary")
    parser.add_argument(
        "--abs-tol",
        "--absolute-tolerance",
        dest="absolute_tolerance",
        type=nonnegative_float,
        default=DEFAULT_TOLERANCE,
        help=f"absolute numeric tolerance (default: {DEFAULT_TOLERANCE:g})",
    )
    parser.add_argument(
        "--rel-tol",
        "--relative-tolerance",
        dest="relative_tolerance",
        type=nonnegative_float,
        default=DEFAULT_TOLERANCE,
        help=f"relative numeric tolerance (default: {DEFAULT_TOLERANCE:g})",
    )
    parser.add_argument(
        "--case",
        dest="named_cases",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="address file case (repeatable; positional cases are also accepted)",
    )
    parser.add_argument("cases", nargs="*", metavar="NAME=PATH")
    return parser


def run_case(
    case: Case,
    rust_binary: str,
    haskell_binary: str,
    absolute: float,
    relative: float,
) -> bool:
    try:
        validate_input(case.path)
        rust = run_json([rust_binary, "maad", case.path], "Rust")
        haskell = run_json(
            [
                haskell_binary,
                "--input",
                case.path,
                "--output",
                "-",
                "--format",
                "json",
                "--structure",
                "--spectrum",
                "--dimensions",
            ],
            "Haskell",
        )
        errors, counts = compare_results(rust, haskell, absolute, relative)
    except CaseError as error:
        print(f"{case.name}: FAIL {error}")
        return False

    if errors:
        print(f"{case.name}: FAIL")
        for error in errors:
            print(f"  {error}")
        return False

    total, structure_rows, spectrum_rows, dimension_rows, prefix_count = counts
    print(
        f"{case.name}: PASS total={total} prefixes={prefix_count} "
        f"rows=structure:{structure_rows},spectrum:{spectrum_rows},dimensions:{dimension_rows}"
    )
    return True


def main(argv: Iterable[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    cases = case_specs(parser, args)

    all_passed = True
    for case in cases:
        all_passed = run_case(
            case,
            args.rust,
            args.haskell,
            args.absolute_tolerance,
            args.relative_tolerance,
        ) and all_passed
    return 0 if all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
