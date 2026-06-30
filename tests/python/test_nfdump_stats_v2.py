import importlib
import subprocess

import pytest


def load_module():
    nfdump_stats_v2 = importlib.import_module('nfdump_stats_v2')
    return importlib.reload(nfdump_stats_v2)


def test_build_nfcapd_bucket_payload_uses_grouped_nfdump_outputs(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()
    commands = []

    def fake_run(command, capture_output, text, timeout):
        commands.append(command)
        assert capture_output is True
        assert text is True
        assert timeout == 300
        command_text = ' '.join(command)
        if '-A proto,srctos' in command_text and 'ipv4' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=(
                    'proto,srcTos,packets,bytes,flows\n'
                    '6,2,10,1000,2\n'
                    '17,1,5,500,1\n'
                ),
                stderr='',
            )
        if '-A proto,srctos' in command_text and 'ipv6' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout='proto,srcTos,packets,bytes,flows\n58,0,3,300,1\n',
                stderr='',
            )
        if '-A proto' in command_text and 'ipv4' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=(
                    'firstSeen,duration,proto,packets,bytes,bps,bpp,flows\n'
                    '2025-01-01 00:00:00.000,1.0,6,10,1000,0,0,2\n'
                    '2025-01-01 00:00:00.000,1.0,17,5,500,0,0,1\n'
                ),
                stderr='',
            )
        if '-A proto' in command_text and 'ipv6' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=(
                    'firstSeen,duration,proto,packets,bytes,bps,bpp,flows\n'
                    '2025-01-01 00:00:00.000,1.0,58,3,300,0,0,1\n'
                ),
                stderr='',
            )
        if '-A srcip,dstip,srctos' in command_text:
            assert 'ipv4' not in command
            assert 'ipv6' not in command
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=(
                    'srcAddr,dstAddr,srcTos\n'
                    '192.0.2.1,198.51.100.1,2\n'
                    '192.0.2.2,198.51.100.2,1\n'
                    '2001:db8::1,2001:db8::2,0\n'
                ),
                stderr='',
            )
        if '-A srcip,dstip' in command_text:
            assert 'ipv4' not in command
            assert 'ipv6' not in command
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=(
                    '192.0.2.1,198.51.100.1\n'
                    '192.0.2.2,198.51.100.2\n'
                    '2001:db8::1,2001:db8::2\n'
                ),
                stderr='',
            )
        raise AssertionError(f'unexpected command: {command}')

    monkeypatch.setattr(module.subprocess, 'run', fake_run)

    payload = module.build_nfcapd_bucket_payload(
        '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
        source_id='oh_ir1_gw',
    )

    assert payload['processed_bucket'] == {
        'input_kind': 'nfcapd',
        'input_locator': '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
        'source_id': 'oh_ir1_gw',
        'bucket_start': 1744700700,
        'bucket_end': 1744701000,
    }
    assert 'netflow_rows' not in payload
    assert 'ip_row' not in payload
    assert 'protocol_row' not in payload
    assert 'maad_source_ipv4' not in payload['raw_bucket']
    assert payload['traffic_v3_rows'][0]['src_visibility'] == 'all'
    assert payload['traffic_v3_rows'][0]['dst_visibility'] == 'all'
    assert {
        (row['ip_version'], row['src_visibility'], row['dst_visibility'], row['address_side'], row['unique_address_count'])
        for row in payload['address_count_v3_rows']
    } == {
        (4, 'all', 'all', 'source', 2),
        (4, 'all', 'all', 'destination', 2),
        (4, 'anonymized', 'literal', 'source', 1),
        (4, 'anonymized', 'literal', 'destination', 1),
        (4, 'literal', 'anonymized', 'source', 1),
        (4, 'literal', 'anonymized', 'destination', 1),
        (6, 'all', 'all', 'source', 1),
        (6, 'all', 'all', 'destination', 1),
        (6, 'literal', 'literal', 'source', 1),
        (6, 'literal', 'literal', 'destination', 1),
    }
    assert len(commands) == 3


def test_parse_nfcapd_bucket_start_uses_first_fold_for_ambiguous_fall_back(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()

    assert (
        module.parse_nfcapd_bucket_start('/captures/oh_ir1_gw/2025/11/02/nfcapd.202511020115')
        == 1762071300
    )


def test_parse_nfcapd_bucket_start_rejects_tmp_suffix() -> None:
    module = load_module()

    with pytest.raises(ValueError, match='Invalid nfcapd filename'):
        module.parse_nfcapd_bucket_start('/captures/oh_ir1_gw/2025/11/02/nfcapd.202511020115.tmp')


def test_read_address_sets_by_version_uses_fast_ipv4_path(monkeypatch) -> None:
    module = load_module()

    def fake_run(command, capture_output, text, timeout):
        return subprocess.CompletedProcess(
            command,
            0,
            stdout=(
                '192.0.2.1,198.51.100.1\n'
                'source,destination\n'
                '2001:0db8::1,2001:0db8::2\n'
                'not-ip,198.51.100.2\n'
            ),
            stderr='',
        )

    monkeypatch.setattr(module.subprocess, 'run', fake_run)

    source_ipv4, destination_ipv4, source_ipv6, destination_ipv6 = module.read_address_sets_by_version(
        '/captures/nfcapd.202508190500'
    )

    assert source_ipv4 == {3221225985}
    assert destination_ipv4 == {3325256705}
    assert source_ipv6 == {'2001:db8::1'}
    assert destination_ipv6 == {'2001:db8::2'}


def test_read_protocol_counters_skips_sparse_nfdump_rows(monkeypatch, caplog) -> None:
    module = load_module()

    def fake_run(command, capture_output, text, timeout):
        return subprocess.CompletedProcess(
            command,
            0,
            stdout=(
                'firstSeen,duration,proto,packets,bytes,bps,bpp,flows\n'
                '2025-01-01 00:00:00.000,1.0,58,3,300,0,0,1\n'
                '2025-01-01 00:00:00.000,1.0,,3,300,0,0,1\n'
                'Summary,,,,,,,\n'
            ),
            stderr='',
        )

    monkeypatch.setattr(module.subprocess, 'run', fake_run)

    rows = module.read_protocol_counters('/captures/nfcapd.202508190000', 6)

    assert rows == [(58, 3, 300, 1)]
    assert 'Skipping malformed nfdump protocol row' in caplog.text


def test_read_protocol_counters_treats_no_matching_flows_as_empty(monkeypatch, caplog) -> None:
    module = load_module()

    def fake_run(command, capture_output, text, timeout):
        return subprocess.CompletedProcess(
            command,
            0,
            stdout=(
                'firstSeen,duration,proto,packets,bytes,bps,bpp,flows\n'
                'No matching flows\n'
            ),
            stderr='',
        )

    monkeypatch.setattr(module.subprocess, 'run', fake_run)

    rows = module.read_protocol_counters('/captures/nfcapd.202508190500', 6)

    assert rows == []
    assert 'Skipping malformed nfdump protocol row' not in caplog.text
