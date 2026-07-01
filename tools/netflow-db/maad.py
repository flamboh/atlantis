"""
MAAD integration helpers for pipeline.

Consumes the MAAD JSON stdout contract:
MAAD --input - --output - --format json --structure --spectrum --dimensions
"""

from __future__ import annotations

import json
import math
import os
import subprocess
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any


MIN_MAAD_ADDRS = 2
DEFAULT_MAAD_TIMEOUT_SECONDS = int(os.environ.get('MAAD_TIMEOUT_SECONDS', '300'))
SPILLOVER_THRESHOLD = 0.05
DELTA_Q = 1.0 / 8.0
MIN_Q = -0.5
MAX_Q = 3.5
QS = [MIN_Q + (index * DELTA_Q) for index in range(int((MAX_Q - MIN_Q) / DELTA_Q) + 1)]


class MaadV2Error(ValueError):
    """Raised when MAAD output or configuration is invalid."""


class MaadTimeoutError(TimeoutError):
    """Raised when a MAAD invocation exceeds the configured timeout."""


@dataclass(frozen=True)
class MaadJsonResult:
    schema_version: int
    metadata: dict[str, Any]
    structure: list[dict[str, Any]]
    spectrum: list[dict[str, Any]]
    dimensions: list[dict[str, Any]]


@dataclass(frozen=True)
class PreparedMoment:
    parent_counts: tuple[int, ...]
    child_power_counts: tuple[tuple[int, ...], ...]


def parse_maad_json(output: str) -> MaadJsonResult:
    """Parse the MAAD JSON stdout contract."""
    try:
        payload = json.loads(output)
    except json.JSONDecodeError as error:
        raise MaadV2Error('MAAD output was not valid JSON.') from error

    if not isinstance(payload, dict):
        raise MaadV2Error('MAAD JSON output must be an object.')

    schema_version = payload.get('schemaVersion')
    if schema_version != 1:
        raise MaadV2Error(f'Unsupported MAAD schemaVersion: {schema_version!r}.')

    metadata = payload.get('metadata')
    structure = payload.get('structure')
    spectrum = payload.get('spectrum')
    dimensions = payload.get('dimensions')

    if not isinstance(metadata, dict):
        raise MaadV2Error('MAAD metadata must be an object.')
    for key, value in {
        'structure': structure,
        'spectrum': spectrum,
        'dimensions': dimensions,
    }.items():
        if not isinstance(value, list):
            raise MaadV2Error(f'MAAD {key} must be a list.')

    return MaadJsonResult(
        schema_version=schema_version,
        metadata=metadata,
        structure=structure,
        spectrum=spectrum,
        dimensions=dimensions,
    )


def run_maad_json(
    maad_bin: str | Path,
    addresses: set[str | int],
    *,
    timeout_seconds: int = DEFAULT_MAAD_TIMEOUT_SECONDS,
) -> MaadJsonResult:
    """Run MAAD against a set of IPv4 addresses and parse its JSON output."""
    if len(addresses) < MIN_MAAD_ADDRS:
        return empty_maad_result(len(addresses))

    address_values = sorted({parse_ipv4_address(address) for address in addresses})
    input_payload = '\n'.join(format_ipv4_address(address) for address in address_values)
    try:
        result = subprocess.run(
            [
                str(maad_bin),
                '--input',
                '-',
                '--output',
                '-',
                '--format',
                'json',
                '--structure',
                '--spectrum',
                '--dimensions',
            ],
            input=input_payload,
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as error:
        raise MaadTimeoutError(
            f'MAAD timed out after {timeout_seconds}s for {len(addresses)} addresses'
        ) from error
    if result.returncode != 0:
        raise RuntimeError(f'MAAD failed: {result.stderr.strip()}')
    return parse_maad_json(result.stdout)


def compute_maad_json(addresses: set[str | int]) -> MaadJsonResult:
    """Compute MAAD-compatible JSON in process for already-materialized IPv4 sets."""
    if len(addresses) < MIN_MAAD_ADDRS:
        return empty_maad_result(len(addresses))

    address_values = sorted({parse_ipv4_address(address) for address in addresses})
    counts_by_length = build_prefix_counts_by_length(address_values)
    min_prefix_length = first_atomic_length(counts_by_length)
    max_prefix_length = first_spillover_length(counts_by_length)
    if min_prefix_length > max_prefix_length:
        return empty_maad_result(len(address_values))

    prefix_lengths = list(range(min_prefix_length, max_prefix_length + 1))
    prepared_moments = prepare_moments(counts_by_length, prefix_lengths)
    structure_rows = compute_structure_rows(prepared_moments)
    spectrum_rows = compute_spectrum_rows(structure_rows)
    dimension_rows = compute_dimension_rows(counts_by_length, prefix_lengths, structure_rows, len(address_values))

    return MaadJsonResult(
        schema_version=1,
        metadata={
            'input': '-',
            'minPrefixLength': min_prefix_length,
            'maxPrefixLength': max_prefix_length,
            'totalAddrs': len(address_values),
        },
        structure=structure_rows,
        spectrum=spectrum_rows,
        dimensions=dimension_rows,
    )


@lru_cache(maxsize=2_000_000)
def parse_ipv4_address(address: str | int) -> int:
    """Parse dotted IPv4 without constructing ipaddress objects in the hot path."""
    if isinstance(address, int):
        if 0 <= address <= 0xFFFFFFFF:
            return address
        raise MaadV2Error(f'Invalid IPv4 address for MAAD: {address!r}.')
    parts = address.split('.')
    if len(parts) != 4:
        raise MaadV2Error(f'Invalid IPv4 address for MAAD: {address!r}.')
    value = 0
    for part in parts:
        try:
            octet = int(part)
        except ValueError as error:
            raise MaadV2Error(f'Invalid IPv4 address for MAAD: {address!r}.') from error
        if octet < 0 or octet > 255:
            raise MaadV2Error(f'Invalid IPv4 address for MAAD: {address!r}.')
        value = (value << 8) | octet
    return value


def format_ipv4_address(address: int) -> str:
    """Format a 32-bit IPv4 integer as dotted quad."""
    return '.'.join(str((address >> shift) & 0xFF) for shift in (24, 16, 8, 0))


def build_prefix_counts_by_length(addresses: list[int]) -> list[dict[int, int]]:
    """Return prefix-count maps for prefix lengths 0..32."""
    counts_by_length = [dict() for _ in range(33)]
    for address in addresses:
        for prefix_length in range(33):
            prefix = address >> (32 - prefix_length) if prefix_length > 0 else 0
            counts = counts_by_length[prefix_length]
            counts[prefix] = counts.get(prefix, 0) + 1
    return counts_by_length


def first_atomic_length(counts_by_length: list[dict[int, int]]) -> int:
    """Return the first prefix length containing a singleton address prefix."""
    for prefix_length in range(1, 33):
        if any(count == 1 for count in counts_by_length[prefix_length].values()):
            return prefix_length
    return 33


def first_spillover_length(counts_by_length: list[dict[int, int]]) -> int:
    """Return the Haskell MAAD spill-over parent prefix length."""
    for child_length in range(1, 33):
        capacity = 2.0 ** (32 - child_length)
        if any((count / capacity) >= (1.0 - SPILLOVER_THRESHOLD) for count in counts_by_length[child_length].values()):
            return child_length
    return 33


def compute_structure_rows(
    prepared_moments: list[PreparedMoment],
) -> list[dict[str, float]]:
    """Compute MAAD structure rows for q in [-0.5, 3.5] step 1/8."""
    rows = []
    for q in QS:
        moments = [one_moment(prepared, q) for prepared in prepared_moments]
        tau_tilde = sum(moment[0] for moment in moments) / len(moments)
        sd = math.sqrt(sum(moment[1] for moment in moments) / len(moments))
        rows.append({'q': q, 'tauTilde': tau_tilde, 'sd': sd})
    return rows


def prepare_moments(
    counts_by_length: list[dict[int, int]],
    prefix_lengths: list[int],
) -> list[PreparedMoment]:
    """Precompute parent/child prefix counts once for the q sweep."""
    prepared = []
    for prefix_length in prefix_lengths:
        parent_counts_by_prefix = {
            prefix: count
            for prefix, count in counts_by_length[prefix_length].items()
            if count > 1
        }
        parent_counts = []
        child_power_counts = []
        next_counts = counts_by_length[prefix_length + 1]
        for prefix, count in parent_counts_by_prefix.items():
            children = tuple(
                child_count
                for child_prefix in (prefix << 1, (prefix << 1) | 1)
                if (child_count := next_counts.get(child_prefix)) is not None
            )
            if children:
                parent_counts.append(count)
                child_power_counts.append(children)
        prepared.append(
            PreparedMoment(
                parent_counts=tuple(parent_counts),
                child_power_counts=tuple(child_power_counts),
            )
        )
    return prepared


def one_moment(prepared: PreparedMoment, q: float) -> tuple[float, float]:
    """Compute the modified O&W estimator for one prepared prefix length and q."""
    if not prepared.parent_counts:
        return 0.0, 0.0

    parent_powers = [count**q for count in prepared.parent_counts]
    child_power_sums = [
        sum(child_count**q for child_count in child_counts)
        for child_counts in prepared.child_power_counts
    ]
    this_z = sum(parent_powers)
    next_z = sum(child_power_sums)
    if this_z <= 0 or next_z <= 0:
        return 0.0, 0.0

    d2 = 0.0
    for parent_power, child_power_sum in zip(parent_powers, child_power_sums):
        d2 += ((parent_power / this_z) - (child_power_sum / next_z)) ** 2.0
    return math.log2(this_z) - math.log2(next_z), d2


def compute_spectrum_rows(structure_rows: list[dict[str, float]]) -> list[dict[str, float]]:
    """Compute multifractal spectrum rows from structure rows."""
    alphas = []
    for index in range(1, len(structure_rows) - 1):
        previous_tau = structure_rows[index - 1]['tauTilde']
        q = structure_rows[index]['q']
        tau = structure_rows[index]['tauTilde']
        next_tau = structure_rows[index + 1]['tauTilde']
        alpha = (next_tau - previous_tau) / (2 * DELTA_Q)
        alphas.append((alpha, q * alpha - tau))

    rows = []
    started = False
    for (alpha, _f), (next_alpha, next_f) in zip(alphas, alphas[1:]):
        decreasing = alpha > next_alpha
        if not started and not decreasing:
            continue
        if not decreasing:
            break
        started = True
        rows.append({'alpha': next_alpha, 'f': next_f})
    return rows


def compute_dimension_rows(
    counts_by_length: list[dict[int, int]],
    prefix_lengths: list[int],
    structure_rows: list[dict[str, float]],
    total_addresses: int,
) -> list[dict[str, float]]:
    """Compute generalized dimensions rows."""
    rows = [{'q': 1.0, 'dim': info_dimension(counts_by_length, prefix_lengths, total_addresses)}]
    for row in structure_rows:
        q = row['q']
        if math.isclose(q, 0.0) or math.isclose(q, 2.0):
            rows.append({'q': q, 'dim': row['tauTilde'] / (q - 1.0)})
    return rows


def info_dimension(
    counts_by_length: list[dict[int, int]],
    prefix_lengths: list[int],
    total_addresses: int,
) -> float:
    """Compute the q=1 generalized dimension using the MAAD regression."""
    points = []
    total = float(total_addresses)
    for prefix_length in prefix_lengths:
        entropy = 0.0
        for count in counts_by_length[prefix_length].values():
            p = count / total
            entropy += p * math.log2(p)
        points.append((-float(prefix_length), entropy))

    mean_x = sum(point[0] for point in points) / len(points)
    mean_y = sum(point[1] for point in points) / len(points)
    denominator = sum((point[0] - mean_x) ** 2 for point in points)
    if denominator == 0:
        return 0.0
    numerator = sum((point[0] - mean_x) * (point[1] - mean_y) for point in points)
    return numerator / denominator


def empty_maad_result(total_addrs: int = 0) -> MaadJsonResult:
    """Return an empty MAAD result for too-small buckets."""
    return MaadJsonResult(
        schema_version=1,
        metadata={
            'input': '-',
            'minPrefixLength': None,
            'maxPrefixLength': None,
            'totalAddrs': total_addrs,
        },
        structure=[],
        spectrum=[],
        dimensions=[],
    )
