import importlib
import sqlite3

import pytest


def load_module():
    module = importlib.import_module('processed_inputs')
    return importlib.reload(module)


def create_legacy_processed_inputs_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE processed_inputs (
            input_kind TEXT NOT NULL,
            input_locator TEXT NOT NULL,
            source_id TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            error_message TEXT,
            discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            processed_at DATETIME,
            PRIMARY KEY (input_kind, input_locator, source_id, bucket_start)
        ) WITHOUT ROWID
        """
    )


def add_bucket(
    processed_inputs,
    conn: sqlite3.Connection,
    *,
    input_locator: str = '/csv/input.csv',
    scan_locator: str = '/csv/input.csv',
    bucket_start: int = 300,
) -> None:
    processed_inputs.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator=input_locator,
        scan_locator=scan_locator,
        source_id='r1',
        bucket_start=bucket_start,
        bucket_end=bucket_start + 300,
    )


def mark_bucket_processed(
    processed_inputs,
    conn: sqlite3.Connection,
    *,
    input_locator: str = '/csv/input.csv',
    bucket_start: int = 300,
) -> None:
    processed_inputs.mark_input_bucket_status(
        conn,
        input_kind='csv',
        input_locator=input_locator,
        source_id='r1',
        bucket_start=bucket_start,
        status='processed',
    )


def test_empty_successful_scan_can_complete() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')

    with conn:
        processed_inputs.complete_input_scan(
            conn,
            input_kind='csv',
            scan_locator='/csv/empty.csv',
            rejected_rows=4,
            skipped_bad_column_count=2,
        )

    assert processed_inputs.input_scan_fully_processed(
        conn,
        input_kind='csv',
        scan_locator='/csv/empty.csv',
    )
    assert conn.execute(
        """
        SELECT rejected_rows, skipped_bad_column_count
        FROM processed_input_scans
        WHERE input_locator = ?
        """,
        ('/csv/empty.csv',),
    ).fetchone() == (4, 2)


def test_fatal_scan_without_completion_remains_retryable() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    add_bucket(processed_inputs, conn)

    assert not processed_inputs.input_scan_fully_processed(
        conn,
        input_kind='csv',
        scan_locator='/csv/input.csv',
    )
    assert conn.execute('SELECT * FROM processed_input_scans').fetchall() == []


def test_pipeline_completion_check_requires_scan_record_and_processed_buckets() -> None:
    processed_inputs = load_module()
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    conn = sqlite3.connect(':memory:')
    add_bucket(processed_inputs, conn)

    assert not pipeline.csv_input_fully_processed(conn, '/csv/input.csv')

    conn.execute(
        """
        INSERT INTO processed_input_scans (
            input_kind, input_locator, status, rejected_rows
        ) VALUES ('csv', '/csv/input.csv', 'processed', 0)
        """
    )
    assert not pipeline.csv_input_fully_processed(conn, '/csv/input.csv')

    mark_bucket_processed(processed_inputs, conn)
    assert pipeline.csv_input_fully_processed(conn, '/csv/input.csv')


def test_scan_completion_rejects_partial_pending_work() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    add_bucket(processed_inputs, conn, bucket_start=300)
    add_bucket(processed_inputs, conn, bucket_start=600)
    mark_bucket_processed(processed_inputs, conn, bucket_start=300)

    with pytest.raises(ValueError, match='1 bucket'):
        processed_inputs.complete_input_scan(
            conn,
            input_kind='csv',
            scan_locator='/csv/input.csv',
            rejected_rows=0,
        )

    assert conn.execute('SELECT * FROM processed_input_scans').fetchall() == []


def test_scan_completion_is_scoped_to_its_locator() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    add_bucket(processed_inputs, conn, scan_locator='/csv/first.csv')
    add_bucket(
        processed_inputs,
        conn,
        input_locator='/csv/second.csv',
        scan_locator='/csv/second.csv',
        bucket_start=600,
    )
    mark_bucket_processed(processed_inputs, conn)

    with conn:
        processed_inputs.complete_input_scan(
            conn,
            input_kind='csv',
            scan_locator='/csv/first.csv',
            rejected_rows=0,
        )

    assert processed_inputs.input_scan_fully_processed(
        conn,
        input_kind='csv',
        scan_locator='/csv/first.csv',
    )
    assert not processed_inputs.input_scan_fully_processed(
        conn,
        input_kind='csv',
        scan_locator='/csv/second.csv',
    )


def test_gap_provenance_can_be_owned_by_archive_scan() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    add_bucket(
        processed_inputs,
        conn,
        input_locator='gap://csv/r1/300',
        scan_locator='/csv/week.tar.gz',
    )
    mark_bucket_processed(
        processed_inputs,
        conn,
        input_locator='gap://csv/r1/300',
    )

    with conn:
        processed_inputs.complete_input_scan(
            conn,
            input_kind='csv',
            scan_locator='/csv/week.tar.gz',
            rejected_rows=0,
        )

    row = conn.execute(
        'SELECT input_locator, scan_locator FROM processed_inputs'
    ).fetchone()
    assert row == ('gap://csv/r1/300', '/csv/week.tar.gz')


def test_table_initialization_does_not_commit_enclosing_transaction() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    processed_inputs.init_processed_inputs_table(conn)
    conn.commit()
    conn.execute('CREATE TABLE transaction_probe (value TEXT)')
    conn.commit()

    conn.execute("INSERT INTO transaction_probe VALUES ('uncommitted')")
    add_bucket(processed_inputs, conn)
    conn.rollback()

    assert conn.execute('SELECT * FROM transaction_probe').fetchall() == []
    assert conn.execute('SELECT * FROM processed_inputs').fetchall() == []


def test_existing_rows_backfill_scan_locator() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    create_legacy_processed_inputs_table(conn)
    conn.execute(
        """
        INSERT INTO processed_inputs (
            input_kind, input_locator, source_id, bucket_start, bucket_end, status
        ) VALUES ('csv', '/csv/old.csv', 'r1', 300, 600, 'processed')
        """
    )

    processed_inputs.init_processed_inputs_table(conn)

    assert conn.execute(
        'SELECT input_locator, scan_locator FROM processed_inputs'
    ).fetchone() == ('/csv/old.csv', '/csv/old.csv')
    assert conn.execute(
        'SELECT input_locator, status FROM processed_input_scans'
    ).fetchone() == ('/csv/old.csv', 'processed')


def test_legacy_pending_or_failed_csv_rows_do_not_gain_terminal_scan_records() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    create_legacy_processed_inputs_table(conn)
    conn.executemany(
        """
        INSERT INTO processed_inputs (
            input_kind, input_locator, source_id, bucket_start, bucket_end, status
        ) VALUES ('csv', ?, 'r1', ?, ?, ?)
        """,
        (
            ('/csv/pending.csv', 300, 600, 'pending'),
            ('/csv/failed.csv', 600, 900, 'failed'),
            ('/csv/mixed.csv', 900, 1200, 'processed'),
            ('/csv/mixed.csv', 1200, 1500, 'pending'),
        ),
    )

    processed_inputs.init_processed_inputs_table(conn)

    assert conn.execute('SELECT * FROM processed_input_scans').fetchall() == []


def test_clear_incomplete_scan_removes_all_attempt_rows_but_preserves_terminal_scan() -> None:
    processed_inputs = load_module()
    conn = sqlite3.connect(':memory:')
    add_bucket(processed_inputs, conn, bucket_start=300)
    add_bucket(processed_inputs, conn, bucket_start=600)
    mark_bucket_processed(processed_inputs, conn, bucket_start=300)

    removed = processed_inputs.clear_incomplete_input_scan(
        conn,
        input_kind='csv',
        scan_locator='/csv/input.csv',
    )

    assert removed == [
        processed_inputs.InputBucketRef('r1', 300),
        processed_inputs.InputBucketRef('r1', 600),
    ]
    assert conn.execute('SELECT * FROM processed_inputs').fetchall() == []

    processed_inputs.complete_input_scan(
        conn,
        input_kind='csv',
        scan_locator='/csv/input.csv',
        rejected_rows=0,
    )
    with pytest.raises(ValueError, match='successfully completed'):
        processed_inputs.clear_incomplete_input_scan(
            conn,
            input_kind='csv',
            scan_locator='/csv/input.csv',
        )
