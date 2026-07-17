"""Versioned contract shared by native nfdump ingestion components."""

from __future__ import annotations


NFDUMP_REDUCER_CONTRACT_VERSION = 1
NFDUMP_REDUCER_INPUT_CONTRACT = 'nfdump-csv-15-v1'
NFDUMP_REDUCER_OUTPUT_CONTRACT = 'canonical-scopes-v1'
NFDUMP_REDUCER_VERSION_LINE = (
    f'nfdump_reducer {NFDUMP_REDUCER_CONTRACT_VERSION} '
    f'{NFDUMP_REDUCER_INPUT_CONTRACT} {NFDUMP_REDUCER_OUTPUT_CONTRACT}'
)
