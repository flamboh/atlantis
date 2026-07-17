import csv
import importlib
import io
import json
import os
from pathlib import Path
import subprocess
import time

import pytest


def load_module():
    module = importlib.import_module('nfdump_stats')
    return importlib.reload(module)


class FakeProcess:
    def __init__(self, stdout: str, *, returncode: int = 0, stderr=None, timeout: bool = False):
        self.stdout = io.StringIO(stdout)
        self.returncode = None
        self._final_returncode = returncode
        self._stderr = stderr
        self._timeout = timeout
        self.killed = False

    def wait(self, timeout=None):
        if self._timeout and not self.killed:
            raise subprocess.TimeoutExpired('nfdump', timeout)
        self.returncode = -9 if self.killed else self._final_returncode
        return self.returncode

    def poll(self):
        return self.returncode

    def kill(self):
        self.killed = True
        self.returncode = -9


def native_csv(*rows: str) -> str:
    return (
        'received,lastSeen,firstSeen,srcAddr,dstAddr,srcPort,dstPort,proto,packets,'
        'bytes,srcTos,dstTos,flows,minTTL,maxTTL\n'
        + ''.join(f'{row}\n' for row in rows)
    )


def install_fake_popen(monkeypatch, module, stdout: str, *, returncode: int = 0, stderr=''):
    calls = []
    processes = []

    def fake_popen(command, **kwargs):
        calls.append((command, kwargs))
        if stderr:
            kwargs['stderr'].write(stderr)
            kwargs['stderr'].flush()
        process = FakeProcess(stdout, returncode=returncode)
        processes.append(process)
        return process

    monkeypatch.setattr(module.subprocess, 'Popen', fake_popen)
    return calls, processes


def compiled_payload(module, stdout: str, selection=None) -> dict:
    flow_selection = importlib.import_module('flow_selection')
    selection = selection or flow_selection.FlowSelection()
    result = subprocess.run(
        module.build_reducer_command(module.DEFAULT_NFDUMP_REDUCER_BIN, selection),
        input=stdout,
        text=True,
        capture_output=True,
        check=True,
    )
    return json.loads(result.stdout)


def compiled_result(module, stdout: str, *extra_arguments: str) -> subprocess.CompletedProcess[str]:
    flow_selection = importlib.import_module('flow_selection')
    command = module.build_reducer_command(
        module.DEFAULT_NFDUMP_REDUCER_BIN,
        flow_selection.FlowSelection(),
    )
    return subprocess.run(
        [*command, *extra_arguments],
        input=stdout,
        text=True,
        capture_output=True,
    )


def canonical_from_python(module, stdout: str, selection=None):
    flow_selection = importlib.import_module('flow_selection')
    normalized_rows = importlib.import_module('normalized_rows')
    statistical_bucket = importlib.import_module('statistical_bucket')
    selection = selection or flow_selection.FlowSelection()
    bucket = statistical_bucket.StatisticalBucket(
        statistical_bucket.BucketKey('r1', '5m', 0, 300),
        dense=True,
    )
    rows = csv.reader(io.StringIO(stdout))
    assert next(rows) == [
        'received',
        'lastSeen',
        'firstSeen',
        'srcAddr',
        'dstAddr',
        'srcPort',
        'dstPort',
        'proto',
        'packets',
        'bytes',
        'srcTos',
        'dstTos',
        'flows',
        'minTTL',
        'maxTTL',
    ]
    for values in rows:
        row = normalized_rows.normalize_nfdump_csv_values(values, 'r1')
        if selection.matches(row.observation):
            bucket.add(row.observation)
    return bucket.finish()


def canonical_from_compiled(module, stdout: str, selection=None):
    statistical_bucket = importlib.import_module('statistical_bucket')
    return module._canonical_bucket_from_reducer(
        compiled_payload(module, stdout, selection),
        statistical_bucket.BucketKey('r1', '5m', 0, 300),
    )


def test_native_bucket_uses_one_all_family_streaming_observation_pass(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()
    stdout = native_csv(
        '1744733000.000,1744733001.500,1744733000.000,192.0.2.1,198.51.100.1,'
        '1023,1024,6,10,1000,2,0,2,31,64',
        '1744733000.000,1744733000.000,1744733000.000,2001:db8::1,2001:db8::2,'
        '3.1,0,58,1,100,0,0,1,0,0',
    )
    reduced = compiled_payload(module, stdout)
    calls = []
    monkeypatch.setattr(
        module,
        '_run_compiled_reducer',
        lambda path, selection: calls.append((path, selection)) or reduced,
    )

    payload = module.build_nfcapd_bucket_payload(
        '/captures/r1/nfcapd.202504150005',
        'r1',
    )

    assert len(calls) == 1
    command = module.build_nfdump_command('/captures/r1/nfcapd.202504150005')
    assert command[:5] == ['nfdump', '-r', '/captures/r1/nfcapd.202504150005', '-q', '-o']
    assert 'ipv4' not in command and 'ipv6' not in command and '-6' not in command
    all_v4 = next(
        row
        for row in payload['traffic_rows']
        if row['ip_version'] == 4
        and row['src_visibility'] == 'all'
        and row['dst_visibility'] == 'all'
    )
    all_v6 = next(
        row
        for row in payload['traffic_rows']
        if row['ip_version'] == 6
        and row['src_visibility'] == 'all'
        and row['dst_visibility'] == 'all'
    )
    assert (all_v4['flows'], all_v4['packets'], all_v4['average_duration_ms']) == (
        2,
        10,
        1500,
    )
    assert (all_v4['average_min_ttl'], all_v4['average_max_ttl']) == (31, 64)
    assert (all_v6['flows'], all_v6['average_duration_ms']) == (1, 0)
    assert (all_v6['average_min_ttl'], all_v6['average_max_ttl']) == (None, None)
    assert any(row['unique_port_count'] == 1 for row in payload['port_count_rows'])


def test_native_selection_pushes_prefix_once_and_filters_visibility(monkeypatch) -> None:
    module = load_module()
    selection_module = importlib.import_module('flow_selection')
    stdout = native_csv(
        '1744733000.000,1744733001.000,1744733000.000,192.0.2.1,198.51.100.1,'
        '1,2,6,1,10,1,0,1,20,30',
        '1744733000.000,1744733001.000,1744733000.000,192.0.2.2,198.51.100.2,'
        '3,4,17,1,10,0,0,1,20,30',
    )
    selection = selection_module.FlowSelection.from_payload(
        {
            'ip_prefix': '192.0.2.99/24',
            'src_visibility': 'literal',
            'dst_visibility': 'anonymized',
        }
    )
    reduced = compiled_payload(module, stdout, selection)
    calls = []
    monkeypatch.setattr(
        module,
        '_run_compiled_reducer',
        lambda path, selected: calls.append((path, selected)) or reduced,
    )

    payload = module.build_nfcapd_bucket_payload(
        '/captures/r1/nfcapd.202504150005', 'r1', selection
    )

    assert len(calls) == 1
    assert module.build_nfdump_command('/captures/r1/nfcapd.202504150005', selection)[-1] == (
        'net 192.0.2.0/24'
    )
    assert module.build_reducer_command(module.DEFAULT_NFDUMP_REDUCER_BIN, selection)[-4:] == [
        '--src-visibility',
        'literal',
        '--dst-visibility',
        'anonymized',
    ]
    all_v4 = next(
        row
        for row in payload['traffic_rows']
        if row['ip_version'] == 4
        and row['src_visibility'] == 'all'
        and row['dst_visibility'] == 'all'
    )
    assert all_v4['flows'] == 1


def test_native_no_match_preserves_dense_zero_coverage(monkeypatch) -> None:
    module = load_module()
    reduced = compiled_payload(module, native_csv('No matching flows'))
    calls = []
    monkeypatch.setattr(
        module,
        '_run_compiled_reducer',
        lambda path, selection: calls.append((path, selection)) or reduced,
    )

    payload = module.build_nfcapd_bucket_payload('/captures/r1/nfcapd.202504150005', 'r1')

    assert len(calls) == 1
    assert len(payload['traffic_rows']) == 10
    assert all(row['flows'] == 0 for row in payload['traffic_rows'])
    assert len(payload['port_count_rows']) == 40
    assert all(row['unique_port_count'] == 0 for row in payload['port_count_rows'])


def test_native_parser_failure_kills_process(monkeypatch) -> None:
    module = load_module()
    _calls, processes = install_fake_popen(monkeypatch, module, '1,2,malformed\n')

    with pytest.raises(RuntimeError, match='Malformed nfdump CSV row'):
        module.build_nfcapd_bucket_payload_python('/captures/r1/nfcapd.202504150005', 'r1')

    assert processes[0].killed is True


def test_native_nonzero_exit_reports_streamed_stderr(monkeypatch) -> None:
    module = load_module()
    install_fake_popen(monkeypatch, module, '', returncode=2, stderr='decoder failed')

    with pytest.raises(RuntimeError, match='decoder failed'):
        module.build_nfcapd_bucket_payload_python('/captures/r1/nfcapd.202504150005', 'r1')


def test_native_timeout_kills_process(monkeypatch) -> None:
    module = load_module()
    processes = []

    def fake_popen(_command, **_kwargs):
        process = FakeProcess('', timeout=True)
        processes.append(process)
        return process

    monkeypatch.setattr(module.subprocess, 'Popen', fake_popen)

    with pytest.raises(module.NfdumpTimeoutError):
        module.build_nfcapd_bucket_payload_python('/captures/r1/nfcapd.202504150005', 'r1')

    assert processes[0].killed is True


def test_compiled_reducer_accepts_real_nfdump_header_and_fails_closed_on_bad_row() -> None:
    module = load_module()
    header = (
        'received,lastSeen,firstSeen,srcAddr,dstAddr,srcPort,dstPort,proto,packets,'
        'bytes,srcTos,dstTos,flows,minTTL,maxTTL\n'
    )
    payload = compiled_payload(
        module,
        header
        + '1744733000.000,1744733001.000,1744733000.000,192.0.2.1,198.51.100.1,'
        '0,65535,6,1,10,0,0,1,0,64\n',
    )
    assert payload['input_contract'] == 'nfdump-csv-15-v1'
    assert payload['scopes'][0]['metrics'][19:21] == [64, 1]

    result = compiled_result(module, header + '1,2,malformed\n')
    assert result.returncode != 0
    assert 'line 2' in result.stderr


def test_compiled_reducer_matches_python_oracle_for_all_metrics_and_scopes() -> None:
    module = load_module()
    stdout = native_csv(
        '0.000,1.500,0.000,192.0.2.1,198.51.100.1,0,1023,6,10,1000,0,0,2,31,64',
        '0.000,2.000,1.000,192.0.2.1,198.51.100.2,1024,65535,17,20,2000,1,255,3,0,0',
        '0.000,0.000,0.000,2001:db8::1,2001:db8::2,8.0,3.1,58,30,3000,2,0,1,1,255',
        '0.000,4.250,4.000,2001:db8::3,2001:db8::4,443,53,132,40,4000,3,1,1,10,20',
    )

    assert canonical_from_compiled(module, stdout) == canonical_from_python(module, stdout)


def test_compiled_reducer_matches_python_oracle_with_visibility_selection() -> None:
    module = load_module()
    flow_selection = importlib.import_module('flow_selection')
    selection = flow_selection.FlowSelection.from_payload(
        {'src_visibility': 'literal', 'dst_visibility': 'anonymized'}
    )
    stdout = native_csv(
        '0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,1,0,1,20,30',
        '0.000,1.000,0.000,192.0.2.2,198.51.100.2,3,4,17,1,10,0,0,1,20,30',
    )

    assert canonical_from_compiled(module, stdout, selection) == canonical_from_python(
        module, stdout, selection
    )


def test_compiled_reducer_matches_python_for_valid_icmp_pseudo_ports() -> None:
    module = load_module()
    stdout = native_csv(
        '0.000,1.000,0.000,192.0.2.1,198.51.100.1,8.0,3.1,1,1,10,0,0,1,20,30',
        '0.000,1.000,0.000,2001:db8::1,2001:db8::2,128.0,1.4,58,1,10,0,0,1,20,30',
    )

    compiled = canonical_from_compiled(module, stdout)
    assert compiled == canonical_from_python(module, stdout)
    assert all(
        entry.bitmap == 1
        for entry in compiled.ports
        if entry.scope.src_visibility == 'all' and entry.scope.dst_visibility == 'all'
    )


@pytest.mark.parametrize(
    ('protocol', 'pseudo_port'),
    [
        ('6', '3.1'),
        ('1', '3.'),
        ('58', '.1'),
        ('1', '3.1.2'),
        ('1', '256.1'),
        ('1', ' 3.1'),
        ('58', '3.1 '),
        (' 1', '3.1'),
    ],
)
def test_compiled_and_python_reject_invalid_dotted_ports(
    protocol: str, pseudo_port: str
) -> None:
    module = load_module()
    stdout = native_csv(
        f'0.000,1.000,0.000,192.0.2.1,198.51.100.1,0,{pseudo_port},{protocol},'
        '1,10,0,0,1,20,30'
    )

    assert compiled_result(module, stdout).returncode != 0
    with pytest.raises(importlib.import_module('csv_ingest').CsvSourceConfigError):
        canonical_from_python(module, stdout)


@pytest.mark.parametrize(
    ('timestamp', 'expected'),
    [
        ('-9223372036854775.808', 0),
        ('9223372036854775.807', 0),
        ('-0.001', 0),
    ],
)
def test_compiled_reducer_timestamp_boundaries_match_python(timestamp: str, expected: int) -> None:
    module = load_module()
    stdout = native_csv(
        f'0.000,{timestamp},{timestamp},192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2'
    )

    compiled = canonical_from_compiled(module, stdout)
    assert compiled == canonical_from_python(module, stdout)
    assert compiled.traffic[0].metrics.duration_sum_ms == expected


@pytest.mark.parametrize(
    ('row', 'message'),
    [
        (
            '0.000,9223372036854775.808,9223372036854775.808,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2',
            'out of range',
        ),
        (
            '0.000,1.0000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2',
            'millisecond precision',
        ),
        (
            '0.000,2.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,9223372036854775807,1,2',
            'duration sum exceeds',
        ),
    ],
)
def test_compiled_reducer_rejects_timestamp_and_metric_overflow(row: str, message: str) -> None:
    module = load_module()
    result = compiled_result(module, native_csv(row))

    assert result.returncode != 0
    assert message in result.stderr


@pytest.mark.parametrize(
    ('stdin', 'message'),
    [
        ('', 'missing CSV header'),
        ('wrong,header\n', 'unexpected CSV header'),
        (native_csv() + '\n', 'empty CSV row'),
        (native_csv('No matching flows', 'No matching flows'), 'only data row'),
    ],
)
def test_compiled_reducer_requires_exact_stream_framing(stdin: str, message: str) -> None:
    module = load_module()
    result = compiled_result(module, stdin)

    assert result.returncode != 0
    assert message in result.stderr


def test_compiled_reducer_cli_requires_exact_versioned_contract() -> None:
    module = load_module()
    binary = str(module.DEFAULT_NFDUMP_REDUCER_BIN)
    cases = [
        [binary],
        [binary, '--contract-version', '2', '--input-contract', 'nfdump-csv-15-v1', '--output-contract', 'canonical-scopes-v1'],
        [binary, '--version', '--src-visibility', 'literal'],
        [binary, '--unknown'],
    ]

    for command in cases:
        result = subprocess.run(command, text=True, capture_output=True)
        assert result.returncode != 0


def test_reducer_output_validator_rejects_drift_and_noncanonical_values() -> None:
    module = load_module()
    statistical_bucket = importlib.import_module('statistical_bucket')
    key = statistical_bucket.BucketKey('r1', '5m', 0, 300)
    payload = compiled_payload(module, native_csv())

    malformed = json.loads(json.dumps(payload))
    malformed['output_contract'] = 'future'
    with pytest.raises(RuntimeError, match='output contract'):
        module._canonical_bucket_from_reducer(malformed, key)

    malformed = json.loads(json.dumps(payload))
    malformed['scopes'][0]['metrics'][0] = 1
    with pytest.raises(RuntimeError, match='protocol metrics'):
        module._canonical_bucket_from_reducer(malformed, key)

    malformed = json.loads(json.dumps(payload))
    malformed['scopes'][0]['source_addresses'] = ['192.0.2.01']
    with pytest.raises(RuntimeError, match='address'):
        module._canonical_bucket_from_reducer(malformed, key)

    malformed = json.loads(json.dumps(payload))
    malformed['scopes'][0]['source_ports_hex'] = '00'
    with pytest.raises(RuntimeError, match='Non-canonical'):
        module._canonical_bucket_from_reducer(malformed, key)

    malformed = json.loads(json.dumps(payload))
    malformed['scopes'][0]['ip_version'] = '4'
    with pytest.raises(RuntimeError, match='scope'):
        module._canonical_bucket_from_reducer(malformed, key)

    malformed = json.loads(json.dumps(payload))
    malformed['scopes'][0]['protocols'] = ['01']
    with pytest.raises(RuntimeError, match='protocols'):
        module._canonical_bucket_from_reducer(malformed, key)

    with pytest.raises(RuntimeError, match='duplicate JSON field'):
        module._strict_json_object([('version', 1), ('version', 1)])


def write_executable(path: Path, body: str) -> None:
    path.write_text(f'#!/bin/sh\n{body}', encoding='utf-8')
    path.chmod(0o700)


def test_compiled_pipeline_reports_both_process_failures(monkeypatch, tmp_path: Path) -> None:
    module = load_module()
    fake_nfdump = tmp_path / 'nfdump'
    write_executable(
        fake_nfdump,
        "printf 'received,lastSeen,firstSeen,srcAddr,dstAddr,srcPort,dstPort,proto,packets,bytes,srcTos,dstTos,flows,minTTL,maxTTL\\n1,2,bad\\n'\n"
        "echo 'nfdump detail' >&2\nexit 7\n",
    )
    monkeypatch.setenv('NFDUMP_BIN', str(fake_nfdump))

    with pytest.raises(RuntimeError) as raised:
        module._run_compiled_reducer('/ignored', importlib.import_module('flow_selection').FlowSelection())

    message = str(raised.value)
    assert "nfdump exit=7" in message
    assert "nfdump detail" in message
    assert "reducer exit=1" in message
    assert "line 2" in message


def test_compiled_pipeline_shared_timeout_kills_pair(monkeypatch, tmp_path: Path) -> None:
    module = load_module()
    fake_nfdump = tmp_path / 'nfdump'
    write_executable(fake_nfdump, "echo 'nfdump waiting' >&2\nsleep 5\n")
    monkeypatch.setenv('NFDUMP_BIN', str(fake_nfdump))
    monkeypatch.setattr(module, 'NFDUMP_TIMEOUT_SECONDS', 0.05)
    started = time.monotonic()

    with pytest.raises(module.NfdumpTimeoutError, match='nfdump waiting'):
        module._run_compiled_reducer('/ignored', importlib.import_module('flow_selection').FlowSelection())

    assert time.monotonic() - started < 2


def test_compiled_pipeline_timeout_kills_nfdump_descendants(
    monkeypatch,
    tmp_path: Path,
) -> None:
    module = load_module()
    fake_nfdump = tmp_path / 'nfdump'
    child_pid_path = tmp_path / 'child.pid'
    write_executable(
        fake_nfdump,
        f"sleep 30 &\necho $! > '{child_pid_path}'\nwait\n",
    )
    monkeypatch.setenv('NFDUMP_BIN', str(fake_nfdump))
    monkeypatch.setattr(module, 'NFDUMP_TIMEOUT_SECONDS', 0.05)

    with pytest.raises(module.NfdumpTimeoutError):
        module._run_compiled_reducer(
            '/ignored',
            importlib.import_module('flow_selection').FlowSelection(),
        )

    child_pid = int(child_pid_path.read_text(encoding='utf-8'))
    deadline = time.monotonic() + 1
    while Path(f'/proc/{child_pid}').exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    assert not Path(f'/proc/{child_pid}').exists()


def test_compiled_pipeline_preflights_both_executables(monkeypatch, tmp_path: Path) -> None:
    module = load_module()
    selection = importlib.import_module('flow_selection').FlowSelection()
    monkeypatch.setenv('NFDUMP_REDUCER_BIN', str(tmp_path / 'missing-reducer'))
    with pytest.raises(RuntimeError, match='reducer is unavailable'):
        module._run_compiled_reducer('/ignored', selection)

    monkeypatch.setenv('NFDUMP_REDUCER_BIN', str(module.DEFAULT_NFDUMP_REDUCER_BIN))
    monkeypatch.setenv('NFDUMP_BIN', str(tmp_path / 'missing-nfdump'))
    with pytest.raises(RuntimeError, match='nfdump executable is unavailable'):
        module._run_compiled_reducer('/ignored', selection)


def test_reducer_handshake_rejects_wrong_executable(monkeypatch, tmp_path: Path) -> None:
    module = load_module()
    wrong = tmp_path / 'wrong-reducer'
    write_executable(wrong, "echo 'wrong 1 contract'\n")
    module._verify_reducer.cache_clear()
    monkeypatch.setenv('NFDUMP_REDUCER_BIN', str(wrong))

    with pytest.raises(RuntimeError, match='Unsupported nfdump reducer'):
        module._run_compiled_reducer('/ignored', importlib.import_module('flow_selection').FlowSelection())


def test_failed_reducer_build_does_not_replace_existing_binary(tmp_path: Path) -> None:
    module = load_module()
    binary = module.DEFAULT_NFDUMP_REDUCER_BIN
    before = binary.read_bytes()
    compiler = tmp_path / 'failing-cxx'
    write_executable(compiler, "echo 'compiler failed' >&2\nexit 42\n")
    result = subprocess.run(
        ['./scripts/build_nfdump_reducer.sh'],
        cwd=Path(__file__).resolve().parents[2],
        env={**os.environ, 'CXX': str(compiler)},
        text=True,
        capture_output=True,
    )

    assert result.returncode != 0
    assert binary.read_bytes() == before


def test_parse_nfcapd_bucket_start_uses_first_fold_for_ambiguous_fall_back(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()

    assert (
        module.parse_nfcapd_bucket_start('/captures/r1/nfcapd.202511020115')
        == 1762071300
    )


def test_parse_nfcapd_bucket_start_rejects_tmp_suffix() -> None:
    module = load_module()

    with pytest.raises(ValueError, match='Invalid nfcapd filename'):
        module.parse_nfcapd_bucket_start('/captures/r1/nfcapd.202511020115.tmp')
