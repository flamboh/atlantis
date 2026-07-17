import importlib

import pytest

from flow_observation import FlowObservation


def load_module():
    return importlib.reload(importlib.import_module('flow_selection'))


def observation(src_ip: str, dst_ip: str, src_tos: int = 0) -> FlowObservation:
    return FlowObservation(
        ip_version=6 if ':' in src_ip else 4,
        src_ip=src_ip,
        dst_ip=dst_ip,
        protocol=6,
        packets=1,
        bytes_count=100,
        src_tos=src_tos,
    )


@pytest.mark.parametrize(
    ('prefix', 'src_ip', 'dst_ip', 'matches'),
    [
        ('192.0.2.99/24', '192.0.2.1', '198.51.100.1', True),
        ('192.0.2.0/24', '198.51.100.1', '192.0.2.2', True),
        ('192.0.2.0/24', '198.51.100.1', '203.0.113.1', False),
        ('2001:db8::7/32', '2001:db8::1', '2001:db9::1', True),
        ('2001:db8::/32', '2001:db9::1', '2001:db8::2', True),
    ],
)
def test_prefix_is_canonical_and_matches_either_endpoint(prefix, src_ip, dst_ip, matches) -> None:
    module = load_module()
    selection = module.FlowSelection.from_payload({'ip_prefix': prefix})

    assert selection.matches(observation(src_ip, dst_ip)) is matches
    assert selection.nfdump_prefix_filter() == f"net {selection.normalized_payload()['ip_prefix']}"


@pytest.mark.parametrize(
    ('src_tos', 'src_visibility', 'dst_visibility'),
    [
        (0, 'literal', 'literal'),
        (1, 'literal', 'anonymized'),
        (2, 'anonymized', 'literal'),
        (3, 'anonymized', 'anonymized'),
    ],
)
def test_visibility_truth_table(src_tos, src_visibility, dst_visibility) -> None:
    module = load_module()
    exact = module.FlowSelection.from_payload(
        {'src_visibility': src_visibility, 'dst_visibility': dst_visibility}
    )
    assert exact.allows_src_tos(src_tos)
    assert not exact.allows_src_tos(src_tos ^ 1)
    assert module.FlowSelection.from_payload(
        {'src_visibility': src_visibility}
    ).allows_src_tos(src_tos)
    assert module.FlowSelection.from_payload(
        {'dst_visibility': dst_visibility}
    ).allows_src_tos(src_tos)


def test_prefix_and_visibility_are_combined_with_and() -> None:
    module = load_module()
    selection = module.FlowSelection.from_payload(
        {
            'ip_prefix': '192.0.2.17/24',
            'src_visibility': 'literal',
            'dst_visibility': 'anonymized',
        }
    )

    assert selection.matches(observation('192.0.2.1', '198.51.100.1', 1))
    assert not selection.matches(observation('203.0.113.1', '198.51.100.1', 1))
    assert not selection.matches(observation('192.0.2.1', '198.51.100.1', 0))
    assert selection.normalized_payload()['ip_prefix'] == '192.0.2.0/24'


def test_invalid_or_unknown_selection_values_are_rejected() -> None:
    module = load_module()
    with pytest.raises(ValueError, match='ip_prefix'):
        module.FlowSelection.from_payload({'ip_prefix': 'not-a-prefix'})
    with pytest.raises(ValueError, match='src_visibility'):
        module.FlowSelection.from_payload({'src_visibility': 'all'})
    with pytest.raises(ValueError, match='Unknown selection'):
        module.FlowSelection.from_payload({'ip_side': 'source'})


def test_unrestricted_selection_preserves_existing_product_identity_shape() -> None:
    module = load_module()

    selection = module.FlowSelection.from_payload({'version': 1, 'kind': 'all'})

    assert selection.is_unrestricted
    assert selection.normalized_payload() == {'version': 1, 'kind': 'all'}
