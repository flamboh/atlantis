import importlib

import pytest


def load_module():
    module = importlib.import_module('statistical_bucket')
    return importlib.reload(module)


def test_flow_observation_expands_visibility_and_classifies_protocol() -> None:
    module = load_module()
    bucket = module.StatisticalBucket(module.BucketKey('r1', '5m', 0, 300))

    bucket.add(
        module.FlowObservation(
            ip_version=4,
            src_ip='192.0.2.1',
            dst_ip='198.51.100.1',
            protocol=6,
            packets=10,
            bytes_count=1000,
            src_tos=2,
        )
    )

    snapshot = bucket.finish()
    assert [entry.scope for entry in snapshot.traffic] == [
        module.Scope(4, 'all', 'all'),
        module.Scope(4, 'anonymized', 'literal'),
    ]
    assert all(entry.metrics.flows_tcp == 1 for entry in snapshot.traffic)
    assert all(entry.protocols == ('6',) for entry in snapshot.protocols)
    assert snapshot.addresses[0].addresses == ('198.51.100.1',)


@pytest.mark.parametrize(
    ('src_tos', 'exact_visibility'),
    [
        (0, ('literal', 'literal')),
        (1, ('literal', 'anonymized')),
        (2, ('anonymized', 'literal')),
        (3, ('anonymized', 'anonymized')),
        (32, ('literal', 'literal')),
        (34, ('anonymized', 'literal')),
    ],
)
def test_flow_observation_visibility_uses_only_source_tos_low_bits(
    src_tos: int,
    exact_visibility: tuple[str, str],
) -> None:
    module = load_module()
    bucket = module.StatisticalBucket(module.BucketKey('r1', '5m', 0, 300))

    bucket.add(
        module.FlowObservation(
            ip_version=4,
            src_ip='192.0.2.1',
            dst_ip='198.51.100.1',
            protocol=6,
            packets=1,
            bytes_count=1,
            src_tos=src_tos,
        )
    )

    assert {
        (entry.scope.src_visibility, entry.scope.dst_visibility)
        for entry in bucket.finish().traffic
    } == {('all', 'all'), exact_visibility}


def test_include_retargets_sums_unions_and_tracks_coverage() -> None:
    module = load_module()
    children = []
    for bucket_start, address in ((0, '192.0.2.2'), (300, '192.0.2.1')):
        child = module.StatisticalBucket(
            module.BucketKey('physical', '5m', bucket_start, bucket_start + 300)
        )
        child.add(
            module.FlowObservation(
                ip_version=4,
                src_ip=address,
                dst_ip='198.51.100.1',
                protocol=17,
                packets=2,
                bytes_count=20,
                src_tos=0,
            )
        )
        children.append(child.finish())

    aggregate = module.StatisticalBucket(module.BucketKey('logical', '30m', 0, 1800))
    for child in children:
        aggregate.include(child)
    snapshot = aggregate.finish()

    assert snapshot.key.source_id == 'logical'
    assert snapshot.traffic[0].metrics.flows == 2
    source_addresses = next(
        entry
        for entry in snapshot.addresses
        if entry.scope == module.Scope(4, 'all', 'all') and entry.address_side == 'source'
    )
    assert source_addresses.addresses == ('192.0.2.1', '192.0.2.2')
    assert snapshot.five_minute_starts == frozenset({0, 300})
    assert snapshot.has_complete_five_minute_coverage is False


def test_flow_and_grouped_adapters_produce_the_same_sparse_snapshot() -> None:
    module = load_module()
    key = module.BucketKey('r1', '5m', 0, 300)
    flows = [
        module.FlowObservation(4, '192.0.2.1', '198.51.100.1', 6, 2, 20, 2),
        module.FlowObservation(4, '192.0.2.2', '198.51.100.2', 6, 3, 30, 2),
    ]
    per_flow = module.StatisticalBucket(key)
    for fact in flows:
        per_flow.add(fact)

    grouped = module.StatisticalBucket(key)
    grouped.add(module.GroupedTrafficFact(4, 6, 2, 2, 5, 50))
    for scope in (
        module.Scope(4, 'all', 'all'),
        module.Scope(4, 'anonymized', 'literal'),
    ):
        grouped.add(
            module.ScopedAddressesFact(
                scope,
                'source',
                {'192.0.2.1', '192.0.2.2'},
            )
        )
        grouped.add(
            module.ScopedAddressesFact(
                scope,
                'destination',
                {'198.51.100.1', '198.51.100.2'},
            )
        )

    assert grouped.finish() == per_flow.finish()


def test_dense_bucket_has_all_zero_query_scopes() -> None:
    module = load_module()
    snapshot = module.StatisticalBucket(
        module.BucketKey('r1', '5m', 0, 300),
        dense=True,
    ).finish()

    assert len(snapshot.traffic) == 10
    assert len(snapshot.protocols) == 10
    assert len(snapshot.addresses) == 20
    assert all(entry.metrics.flows == 0 for entry in snapshot.traffic)
    assert snapshot.has_complete_five_minute_coverage is True


def test_invalid_ip_version_preserves_existing_error() -> None:
    module = load_module()
    bucket = module.StatisticalBucket(module.BucketKey('r1', '5m', 0, 300))

    with pytest.raises(ValueError, match='Unsupported ip_version: 5'):
        bucket.add(
            module.FlowObservation(
                ip_version=5,
                src_ip='192.0.2.1',
                dst_ip='198.51.100.1',
                protocol=6,
                packets=1,
                bytes_count=1,
                src_tos=0,
            )
        )


def test_observation_metrics_are_weighted_with_independent_missingness() -> None:
    module = load_module()
    bucket = module.StatisticalBucket(module.BucketKey('r1', '5m', 0, 300))
    bucket.add(
        module.FlowObservation(
            4,
            '192.0.2.1',
            '198.51.100.1',
            6,
            1,
            10,
            0,
            duration_ms=0,
            min_ttl=None,
            max_ttl=10,
            flow_count=1,
        )
    )
    bucket.add(
        module.FlowObservation(
            4,
            '192.0.2.2',
            '198.51.100.2',
            6,
            1,
            10,
            0,
            duration_ms=100,
            min_ttl=20,
            max_ttl=None,
            flow_count=3,
        )
    )

    metrics = bucket.finish().traffic[0].metrics

    assert metrics.flows == 4
    assert (metrics.duration_sum_ms, metrics.duration_count) == (300, 4)
    assert (metrics.min_ttl_sum, metrics.min_ttl_count) == (60, 3)
    assert (metrics.max_ttl_sum, metrics.max_ttl_count) == (10, 1)


def test_port_bitmaps_union_independently_of_order_and_keep_boundary_ports() -> None:
    module = load_module()
    key = module.BucketKey('r1', '5m', 0, 300)

    def child(src_port: int, dst_port: int):
        builder = module.StatisticalBucket(key)
        builder.add(
            module.FlowObservation(
                4,
                '192.0.2.1',
                '198.51.100.1',
                6,
                1,
                1,
                0,
                src_port=src_port,
                dst_port=dst_port,
            )
        )
        return builder.finish()

    children = [child(0, 1023), child(1023, 1024), child(1024, 65535)]
    forward = module.StatisticalBucket(module.BucketKey('r1', '30m', 0, 1800))
    reverse = module.StatisticalBucket(module.BucketKey('r1', '30m', 0, 1800))
    for snapshot in children:
        forward.include(snapshot)
    for snapshot in reversed(children):
        reverse.include(snapshot)

    assert forward.finish() == reverse.finish()
    all_scope_ports = {
        entry.port_side: entry.bitmap
        for entry in forward.finish().ports
        if entry.scope == module.Scope(4, 'all', 'all')
    }
    assert {port for port in range(65536) if all_scope_ports['source'] & (1 << port)} == {
        0,
        1023,
        1024,
    }
    assert {port for port in range(65536) if all_scope_ports['destination'] & (1 << port)} == {
        1023,
        1024,
        65535,
    }


def test_measurement_sum_overflow_rejects_before_sqlite_insert() -> None:
    module = load_module()
    bucket = module.StatisticalBucket(module.BucketKey('r1', '5m', 0, 300))

    with pytest.raises(OverflowError, match='duration_sum_ms'):
        bucket.add(
            module.FlowObservation(
                4,
                '192.0.2.1',
                '198.51.100.1',
                6,
                1,
                1,
                0,
                duration_ms=module.MAX_SQLITE_INTEGER,
                flow_count=2,
            )
        )
