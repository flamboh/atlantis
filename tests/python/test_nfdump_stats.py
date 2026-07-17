import importlib
import io
import subprocess

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
        'trr,ter,tsr,srcaddr,dstaddr,srcport,dstport,proto,packets,bytes,'
        'srctos,dsttos,flows,minttl,maxttl\n'
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


def test_native_bucket_uses_one_all_family_streaming_observation_pass(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()
    stdout = native_csv(
        '1744733000.000,1744733001.500,1744733000.000,192.0.2.1,198.51.100.1,'
        '1023,1024,6,10,1000,2,0,2,31,64',
        '1744733000.000,1744733000.000,1744733000.000,2001:db8::1,2001:db8::2,'
        '3.1,0,58,1,100,0,0,1,0,0',
    )
    calls, _processes = install_fake_popen(monkeypatch, module, stdout)

    payload = module.build_nfcapd_bucket_payload(
        '/captures/r1/nfcapd.202504150005',
        'r1',
    )

    assert len(calls) == 1
    command, kwargs = calls[0]
    assert command[:5] == ['nfdump', '-r', '/captures/r1/nfcapd.202504150005', '-q', '-o']
    assert 'ipv4' not in command and 'ipv6' not in command and '-6' not in command
    assert kwargs['stdout'] is subprocess.PIPE
    assert kwargs['stderr'] is not subprocess.PIPE
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
    calls, _ = install_fake_popen(monkeypatch, module, stdout)
    selection = selection_module.FlowSelection.from_payload(
        {
            'ip_prefix': '192.0.2.99/24',
            'src_visibility': 'literal',
            'dst_visibility': 'anonymized',
        }
    )

    payload = module.build_nfcapd_bucket_payload(
        '/captures/r1/nfcapd.202504150005', 'r1', selection
    )

    assert len(calls) == 1
    assert calls[0][0][-1] == 'net 192.0.2.0/24'
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
    calls, _ = install_fake_popen(monkeypatch, module, 'No matching flows\n')

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
        module.build_nfcapd_bucket_payload('/captures/r1/nfcapd.202504150005', 'r1')

    assert processes[0].killed is True


def test_native_nonzero_exit_reports_streamed_stderr(monkeypatch) -> None:
    module = load_module()
    install_fake_popen(monkeypatch, module, '', returncode=2, stderr='decoder failed')

    with pytest.raises(RuntimeError, match='decoder failed'):
        module.build_nfcapd_bucket_payload('/captures/r1/nfcapd.202504150005', 'r1')


def test_native_timeout_kills_process(monkeypatch) -> None:
    module = load_module()
    processes = []

    def fake_popen(_command, **_kwargs):
        process = FakeProcess('', timeout=True)
        processes.append(process)
        return process

    monkeypatch.setattr(module.subprocess, 'Popen', fake_popen)

    with pytest.raises(module.NfdumpTimeoutError):
        module.build_nfcapd_bucket_payload('/captures/r1/nfcapd.202504150005', 'r1')

    assert processes[0].killed is True


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
