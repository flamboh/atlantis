import importlib
import subprocess

import pytest


def load_module():
    nfdump_stats_v2 = importlib.import_module('nfdump_stats_v2')
    return importlib.reload(nfdump_stats_v2)


def test_build_nfcapd_bucket_payload_uses_grouped_nfdump_outputs(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()

    def fake_run(command, capture_output, text, timeout):
        assert capture_output is True
        assert text is True
        assert timeout == 300
        command_text = ' '.join(command)
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
        if '-A srcip,dstip' in command_text and 'ipv4' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout='192.0.2.1,198.51.100.1\n192.0.2.2,198.51.100.2\n',
                stderr='',
            )
        if '-A srcip,dstip' in command_text and 'ipv6' in command:
            return subprocess.CompletedProcess(
                command,
                0,
                stdout='2001:db8::1,2001:db8::2\n',
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
    assert payload['netflow_rows'] == [
        {
            'source_id': 'oh_ir1_gw',
            'bucket_start': 1744700700,
            'bucket_end': 1744701000,
            'ip_version': 4,
            'flows': 3,
            'flows_tcp': 2,
            'flows_udp': 1,
            'flows_icmp': 0,
            'flows_other': 0,
            'packets': 15,
            'packets_tcp': 10,
            'packets_udp': 5,
            'packets_icmp': 0,
            'packets_other': 0,
            'bytes': 1500,
            'bytes_tcp': 1000,
            'bytes_udp': 500,
            'bytes_icmp': 0,
            'bytes_other': 0,
        },
        {
            'source_id': 'oh_ir1_gw',
            'bucket_start': 1744700700,
            'bucket_end': 1744701000,
            'ip_version': 6,
            'flows': 1,
            'flows_tcp': 0,
            'flows_udp': 0,
            'flows_icmp': 1,
            'flows_other': 0,
            'packets': 3,
            'packets_tcp': 0,
            'packets_udp': 0,
            'packets_icmp': 3,
            'packets_other': 0,
            'bytes': 300,
            'bytes_tcp': 0,
            'bytes_udp': 0,
            'bytes_icmp': 300,
            'bytes_other': 0,
        },
    ]
    assert payload['ip_row']['sa_ipv4_count'] == 2
    assert payload['ip_row']['da_ipv6_count'] == 1
    assert payload['protocol_row']['protocols_list_ipv4'] == '17,6'
    assert payload['raw_bucket']['maad_source_ipv4'] == ['192.0.2.1', '192.0.2.2']


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
