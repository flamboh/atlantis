import importlib
import subprocess

import pytest


def load_module():
    nfdump_stats = importlib.import_module('nfdump_stats')
    return importlib.reload(nfdump_stats)


def test_build_nfcapd_bucket_payload_uses_grouped_nfdump_outputs(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    module = load_module()
    statistical_bucket = importlib.import_module('statistical_bucket')
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
    assert payload['canonical_bucket'].key.source_id == 'oh_ir1_gw'
    assert payload['traffic_rows'][0]['src_visibility'] == 'all'
    assert payload['traffic_rows'][0]['dst_visibility'] == 'all'
    address_counts = {
        (row['ip_version'], row['src_visibility'], row['dst_visibility'], row['address_side'], row['unique_address_count'])
        for row in payload['address_count_rows']
    }
    expected_address_counts = {
        (ip_version, src_visibility, dst_visibility, address_side, 0)
        for ip_version in (4, 6)
        for src_visibility, dst_visibility in statistical_bucket.ZERO_FILL_VISIBILITY_PAIRS
        for address_side in ('source', 'destination')
    }
    expected_address_counts -= {
        (4, 'all', 'all', 'source', 0),
        (4, 'all', 'all', 'destination', 0),
        (4, 'anonymized', 'literal', 'source', 0),
        (4, 'anonymized', 'literal', 'destination', 0),
        (4, 'literal', 'anonymized', 'source', 0),
        (4, 'literal', 'anonymized', 'destination', 0),
        (6, 'all', 'all', 'source', 0),
        (6, 'all', 'all', 'destination', 0),
        (6, 'literal', 'literal', 'source', 0),
        (6, 'literal', 'literal', 'destination', 0),
    }
    expected_address_counts |= {
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
    assert address_counts == expected_address_counts
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


def test_read_scoped_protocol_counters_treats_no_matching_flows_as_empty(monkeypatch, caplog) -> None:
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

    rows = module.read_scoped_protocol_counters('/captures/nfcapd.202508190500', 6)

    assert rows == []
    assert 'Skipping malformed nfdump scoped protocol row' not in caplog.text


def test_empty_grouped_nfcapd_outputs_emit_zero_rows_for_all_query_scopes() -> None:
    statistical_bucket = importlib.import_module('statistical_bucket')
    stats = importlib.import_module('stats')
    bucket = statistical_bucket.StatisticalBucket(
        statistical_bucket.BucketKey('r1', '5m', 1744700700, 1744701000),
        dense=True,
    ).finish()
    rows = stats.canonical_bucket_rows(bucket)

    assert {
        (row['ip_version'], row['src_visibility'], row['dst_visibility'], row['flows'])
        for row in rows['traffic_rows']
    } == {
        (ip_version, src_visibility, dst_visibility, 0)
        for ip_version in (4, 6)
        for src_visibility, dst_visibility in statistical_bucket.ZERO_FILL_VISIBILITY_PAIRS
    }
    assert {
        (row['ip_version'], row['src_visibility'], row['dst_visibility'], row['protocols_list'])
        for row in rows['protocol_rows']
    } == {
        (ip_version, src_visibility, dst_visibility, '')
        for ip_version in (4, 6)
        for src_visibility, dst_visibility in statistical_bucket.ZERO_FILL_VISIBILITY_PAIRS
    }


def test_native_selection_pushes_prefix_to_every_command_and_filters_visibility(
    monkeypatch,
) -> None:
    module = load_module()
    selection_module = importlib.import_module('flow_selection')
    commands = []

    def fake_run(command, capture_output, text, timeout):
        commands.append(command)
        command_text = ' '.join(command)
        if '-A srcip,dstip,srctos' in command_text:
            stdout = (
                'srcAddr,dstAddr,srcTos\n'
                '192.0.2.1,198.51.100.1,1\n'
                '192.0.2.2,198.51.100.2,0\n'
            )
        else:
            stdout = (
                'proto,srcTos,packets,bytes,flows\n'
                '6,1,10,1000,2\n'
                '17,0,5,500,1\n'
            )
        return subprocess.CompletedProcess(command, 0, stdout=stdout, stderr='')

    monkeypatch.setattr(module.subprocess, 'run', fake_run)
    selection = selection_module.FlowSelection.from_payload(
        {
            'ip_prefix': '192.0.2.99/24',
            'src_visibility': 'literal',
            'dst_visibility': 'anonymized',
        }
    )

    payload = module.build_nfcapd_bucket_payload(
        '/captures/r1/nfcapd.202504150005',
        'r1',
        selection,
    )

    assert len(commands) == 3
    assert all('net 192.0.2.0/24' in ' '.join(command) for command in commands)
    all_v4 = next(
        row
        for row in payload['traffic_rows']
        if row['ip_version'] == 4
        and row['src_visibility'] == 'all'
        and row['dst_visibility'] == 'all'
    )
    exact_v4 = next(
        row
        for row in payload['traffic_rows']
        if row['ip_version'] == 4
        and row['src_visibility'] == 'literal'
        and row['dst_visibility'] == 'anonymized'
    )
    assert (all_v4['flows'], exact_v4['flows']) == (2, 2)
    assert all(
        row['unique_address_count'] in (0, 1)
        for row in payload['address_count_rows']
    )
