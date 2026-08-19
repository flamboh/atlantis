# User documentation

These documents explain how to install and operate ATLANTIS.

ATLANTIS reads nfcapd captures from a NetFlow collector. The pipeline converts the captures into a SQLite database, and the dashboard visualizes that database. CSV input is also available as an adapter for external datasets.

## Before you start

Have this available:

- nfcapd capture files on disk, in the layout that [dataset configuration](datasets.md#input-directory-layout) shows.
- Approximately 30 minutes. The one-time tool builds take most of this time.

## First use

1. [Install the tools and the dependencies](setup-web.md).
2. [Define a dataset](datasets.md) that points at your captures.
3. [Build and verify the database](setup-pipeline.md).
4. [Run the dashboard](setup-web.md#run-the-dashboard).

If a step fails, read [Troubleshooting](troubleshooting.md).

## Other tasks

- [Query a database](querying.md)
- [Verify, publish, and deploy](operations.md)

The [code documentation](../code/README.md) gives architecture and development information.
