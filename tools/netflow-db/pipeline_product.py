"""Database-level pipeline product identity and compatibility binding."""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from typing import Any

from input_revision import canonical_json, fingerprint


@dataclass(frozen=True, slots=True)
class ProductIdentity:
    """Canonical schema, selection, and result-configuration identity."""

    schema_json: str
    schema_fingerprint: str
    selection_json: str
    selection_fingerprint: str
    config_json: str
    config_fingerprint: str
    fingerprint: str

    @classmethod
    def create(
        cls,
        *,
        schema: Any,
        selection: Any,
        config: Any,
    ) -> ProductIdentity:
        schema_json = canonical_json(schema)
        selection_json = canonical_json(selection)
        config_json = canonical_json(config)
        schema_fingerprint = fingerprint(schema)
        selection_fingerprint = fingerprint(selection)
        config_fingerprint = fingerprint(config)
        return cls(
            schema_json=schema_json,
            schema_fingerprint=schema_fingerprint,
            selection_json=selection_json,
            selection_fingerprint=selection_fingerprint,
            config_json=config_json,
            config_fingerprint=config_fingerprint,
            fingerprint=fingerprint(
                {
                    'version': 1,
                    'schema': schema_fingerprint,
                    'selection': selection_fingerprint,
                    'config': config_fingerprint,
                }
            ),
        )


class ProductIdentityConflict(ValueError):
    """Raised when a database is already bound to different result semantics."""


class SourceLayoutConflict(ValueError):
    """Raised when logical nfcapd source ownership changes within one database."""


def init_pipeline_product_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS pipeline_product (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            schema_json TEXT NOT NULL,
            schema_fingerprint TEXT NOT NULL,
            selection_json TEXT NOT NULL,
            selection_fingerprint TEXT NOT NULL,
            config_json TEXT NOT NULL,
            config_fingerprint TEXT NOT NULL,
            product_fingerprint TEXT NOT NULL,
            bound_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        """
    )


def bind_product_identity(
    conn: sqlite3.Connection,
    identity: ProductIdentity,
    *,
    output_table_names: tuple[str, ...],
) -> None:
    """Bind an empty database or validate its existing product identity."""
    init_pipeline_product_table(conn)
    row = conn.execute(
        """
        SELECT schema_json, schema_fingerprint, selection_json, selection_fingerprint,
               config_json, config_fingerprint, product_fingerprint
        FROM pipeline_product WHERE singleton = 1
        """
    ).fetchone()
    if row is None:
        populated = [
            table_name
            for table_name in (*output_table_names, 'processed_inputs', 'processed_input_scans')
            if _table_has_rows(conn, table_name)
        ]
        if populated:
            raise ProductIdentityConflict(
                'Cannot bind product identity to a populated legacy database; '
                f"existing rows found in: {', '.join(populated)}. Use a new database."
            )
        conn.execute(
            """
            INSERT INTO pipeline_product (
                singleton, schema_json, schema_fingerprint,
                selection_json, selection_fingerprint,
                config_json, config_fingerprint, product_fingerprint
            ) VALUES (1, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                identity.schema_json,
                identity.schema_fingerprint,
                identity.selection_json,
                identity.selection_fingerprint,
                identity.config_json,
                identity.config_fingerprint,
                identity.fingerprint,
            ),
        )
        return

    stored = ProductIdentity(*row)
    if stored.fingerprint == identity.fingerprint:
        return
    mismatches = [
        name
        for name in ('schema', 'selection', 'config')
        if getattr(stored, f'{name}_fingerprint') != getattr(identity, f'{name}_fingerprint')
    ]
    details = '; '.join(
        f'{name}: stored={getattr(stored, f"{name}_json")} requested={getattr(identity, f"{name}_json")}'
        for name in mismatches
    )
    raise ProductIdentityConflict(
        f"Pipeline product identity mismatch in {', '.join(mismatches)}. {details}"
    )


def bind_nfcapd_source_layout(
    conn: sqlite3.Connection,
    sources: list[tuple[str, tuple[str, ...]]],
) -> None:
    """Bind normalized logical source membership before any tree publication."""
    layout = [
        {'source_id': source_id, 'members': sorted(members)}
        for source_id, members in sorted(sources)
    ]
    layout_json = canonical_json({'version': 1, 'sources': layout})
    layout_fingerprint = fingerprint({'version': 1, 'sources': layout})
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS nfcapd_source_layout (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            layout_json TEXT NOT NULL,
            layout_fingerprint TEXT NOT NULL,
            bound_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        """
    )
    row = conn.execute(
        'SELECT layout_json, layout_fingerprint FROM nfcapd_source_layout WHERE singleton = 1'
    ).fetchone()
    if row is None:
        if _table_has_rows_where(conn, 'processed_inputs', "input_kind = 'nfcapd'"):
            raise SourceLayoutConflict(
                'Cannot bind nfcapd source layout after unbound nfcapd inputs were processed. '
                'Use a new database.'
            )
        conn.execute(
            """
            INSERT INTO nfcapd_source_layout (singleton, layout_json, layout_fingerprint)
            VALUES (1, ?, ?)
            """,
            (layout_json, layout_fingerprint),
        )
        return
    if row[1] != layout_fingerprint:
        raise SourceLayoutConflict(
            'nfcapd logical source membership changed. '
            f'stored={row[0]} requested={layout_json}. Use a new database.'
        )


def _table_has_rows(conn: sqlite3.Connection, table_name: str) -> bool:
    exists = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        (table_name,),
    ).fetchone()
    return exists is not None and conn.execute(f'SELECT 1 FROM {table_name} LIMIT 1').fetchone() is not None


def _table_has_rows_where(
    conn: sqlite3.Connection,
    table_name: str,
    predicate: str,
) -> bool:
    exists = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        (table_name,),
    ).fetchone()
    return (
        exists is not None
        and conn.execute(f'SELECT 1 FROM {table_name} WHERE {predicate} LIMIT 1').fetchone()
        is not None
    )
