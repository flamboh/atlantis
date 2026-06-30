import importlib
import json
import subprocess
from pathlib import Path

import pytest


def load_module():
    maad_v2 = importlib.import_module('maad_v2')
    return importlib.reload(maad_v2)


def sample_payload(total_addrs: int = 42) -> str:
    return json.dumps(
        {
            'schemaVersion': 1,
            'metadata': {
                'input': '-',
                'minPrefixLength': 7,
                'maxPrefixLength': 23,
                'totalAddrs': total_addrs,
            },
            'structure': [{'q': -0.5, 'tauTilde': -0.98, 'sd': 0.01}],
            'spectrum': [{'alpha': 0.75, 'f': 0.60}],
            'dimensions': [{'q': 1, 'dim': 0.48}],
        }
    )


def test_parse_maad_json_accepts_demo_contract() -> None:
    maad_v2 = load_module()

    result = maad_v2.parse_maad_json(sample_payload())

    assert result.schema_version == 1
    assert result.metadata == {
        'input': '-',
        'minPrefixLength': 7,
        'maxPrefixLength': 23,
        'totalAddrs': 42,
    }
    assert result.structure == [{'q': -0.5, 'tauTilde': -0.98, 'sd': 0.01}]
    assert result.spectrum == [{'alpha': 0.75, 'f': 0.60}]
    assert result.dimensions == [{'q': 1, 'dim': 0.48}]


def test_run_maad_json_uses_explicit_timeout(monkeypatch) -> None:
    maad_v2 = load_module()
    captured = {}

    def fake_run(command, *, input, capture_output, text, timeout):
        captured['command'] = command
        captured['input'] = input
        captured['timeout'] = timeout
        return subprocess.CompletedProcess(command, 0, stdout=sample_payload(3), stderr='')

    monkeypatch.setattr(maad_v2.subprocess, 'run', fake_run)

    result = maad_v2.run_maad_json('/tmp/MAAD', {'198.51.100.1', '192.0.2.1'}, timeout_seconds=900)

    assert result.metadata['totalAddrs'] == 3
    assert captured['command'][:2] == ['/tmp/MAAD', '--input']
    assert captured['timeout'] == 900
    assert captured['input'] == '192.0.2.1\n198.51.100.1'


def test_run_maad_json_wraps_timeout_with_address_count(monkeypatch) -> None:
    maad_v2 = load_module()

    def fake_run(command, **kwargs):
        raise subprocess.TimeoutExpired(command, kwargs['timeout'])

    monkeypatch.setattr(maad_v2.subprocess, 'run', fake_run)

    with pytest.raises(maad_v2.MaadTimeoutError, match='after 600s for 2 addresses'):
        maad_v2.run_maad_json('/tmp/MAAD', {'198.51.100.1', '192.0.2.1'}, timeout_seconds=600)


def test_compute_maad_json_returns_full_contract() -> None:
    maad_v2 = load_module()
    addresses = {f'10.0.{third_octet}.{fourth_octet}' for third_octet in range(2) for fourth_octet in range(256)}
    addresses.add('192.0.2.1')

    result = maad_v2.compute_maad_json(addresses)

    assert result.metadata['totalAddrs'] == len(addresses)
    assert isinstance(result.metadata['minPrefixLength'], int)
    assert isinstance(result.metadata['maxPrefixLength'], int)
    assert result.structure
    assert isinstance(result.spectrum, list)
    assert {row['q'] for row in result.dimensions} == {1.0, 0.0, 2.0}


def test_compute_maad_json_matches_binary_metadata() -> None:
    maad_v2 = load_module()
    maad_bin = Path(__file__).resolve().parents[2] / 'vendor' / 'maad' / 'MAAD'
    if not maad_bin.is_file():
        pytest.skip('MAAD binary is not built')
    addresses = {f'10.0.{third_octet}.{fourth_octet}' for third_octet in range(2) for fourth_octet in range(256)}
    addresses.add('192.0.2.1')

    python_result = maad_v2.compute_maad_json(addresses)
    binary_result = maad_v2.run_maad_json(maad_bin, addresses)

    assert python_result.metadata == binary_result.metadata
    assert python_result.structure[0] == pytest.approx(binary_result.structure[0])
    assert len(python_result.dimensions) == len(binary_result.dimensions)
    for python_row, binary_row in zip(python_result.dimensions, binary_result.dimensions):
        assert python_row == pytest.approx(binary_row)
