# Dataset configuration

`datasets.json` tells the pipeline where your captures are. It is git-ignored, so machine-specific paths stay out of commits.

## Create the file

```bash
cp datasets.json.example datasets.json
cp .env.example .env
```

1. Set `root_path` to the directory that holds your captures.
2. Add one entry in `sources` for each collector directory under `root_path`.
3. In `.env`, set `DEFAULT_DATASET` to your `dataset_id`.

## Minimal dataset

```json
[
  {
    "dataset_id": "example",
    "root_path": "/path/to/captures",
    "sources": [{ "source_id": "router-a", "members": ["router-a"] }]
  }
]
```

The pipeline writes `data/example/netflow.sqlite`, and the dashboard finds it there automatically.

## Input directory layout

```text
<root_path>/
  <member>/
    YYYY/MM/DD/nfcapd.YYYYMMddHHmm
```

Each name in `members` must be a directory directly under `root_path`, or the pipeline stops.

## Combine collectors

A source can combine several collector directories:

```json
"sources": [
  { "source_id": "router-a", "members": ["router-a"] },
  { "source_id": "router-b", "members": ["router-b"] },
  { "source_id": "all-routers", "members": ["router-a", "router-b"] }
]
```

Renaming a source or changing its members needs a new database.

## Classify internal and external endpoints

`locality` lists the addresses that belong to your network. Matching endpoints are `internal`, and everything else is `external`. The dashboard uses this for ingress, egress, lateral, and transit traffic.

```json
"locality": [
  { "type": "prefixes", "prefixes": ["192.0.2.0/24", "2001:db8::/32"] },
  { "type": "addresses", "addresses": ["198.51.100.53"] },
  { "type": "addresses", "path": "data/private/internal-addresses.txt" }
]
```

| Rule type        | Matches                                                                                                 |
| ---------------- | ------------------------------------------------------------------------------------------------------- |
| `prefixes`       | Any listed IPv4 or IPv6 CIDR. Host bits must be zero.                                                   |
| `addresses`      | Exact addresses, inline in `addresses` or one per line in a `path` file (`#` starts a comment).         |
| `tos_anonymized` | Endpoints flagged by the capture anonymizer in the low source-ToS bits. Most networks do not set these. |

- Relative `path` values resolve from the repository root. Keep address files under the git-ignored `data/` directory, which the Docker wrapper can also read.
- Without `locality`, all traffic shows as transit. A large transit share usually means the rules miss part of your address space.
- Changing a rule, or the contents of an address file, needs a new database.
- MAAD is not computed for address sets that hold only internal addresses unless the dataset sets `"maad_internal_side": true`. For ingress, egress, and lateral traffic, the dashboard's MAAD cards say so for the internal side instead of drawing an empty chart. Changing the setting needs a new database.

## Define coordinated subsets

To build several [`daily_active_sources`](setup-pipeline.md#build-several-subsets-in-one-pass) subsets in one pass, give each its own entry with a `selection` and a `db_path`:

```json
[
  {
    "dataset_id": "dataset-a",
    "root_path": "/path/to/captures",
    "source_ids": ["router-a"],
    "locality": [{ "type": "prefixes", "prefixes": ["198.18.0.0/15"] }],
    "selection": {
      "kind": "daily_active_sources",
      "ip_prefix": "198.18.0.0/16"
    },
    "db_path": "data/dataset-a/netflow.sqlite"
  },
  {
    "dataset_id": "dataset-b",
    "root_path": "/path/to/captures",
    "source_ids": ["router-a"],
    "locality": [{ "type": "prefixes", "prefixes": ["198.18.0.0/15"] }],
    "selection": {
      "kind": "daily_active_sources",
      "ip_prefix": "198.19.0.0/16"
    },
    "db_path": "data/dataset-b/netflow.sqlite"
  }
]
```

## Fields

| Field                | Default                            | Purpose                                                                            |
| -------------------- | ---------------------------------- | ---------------------------------------------------------------------------------- |
| `dataset_id`         | Required                           | ID used in commands and dashboard URLs.                                            |
| `root_path`          | Required                           | Directory that holds the captures.                                                 |
| `label`              | Title-cased `dataset_id`           | Name shown in the dashboard.                                                       |
| `sources`            | None                               | Sources and their collector directories.                                           |
| `source_ids`         | None                               | One simple source per name. Do not combine with `sources`.                         |
| `db_path`            | `data/<dataset-id>/netflow.sqlite` | Output database. Set it for filtered products.                                     |
| `selection`          | All flows                          | Flow filter applied on every run. Needs its own `db_path`.                         |
| `locality`           | None                               | [Internal address rules](#classify-internal-and-external-endpoints).               |
| `maad_internal_side` | `false`                            | Compute MAAD for address sets that hold only internal addresses.                   |
| `default_start_date` | Earliest processed day             | First day the dashboard shows.                                                     |
| `discovery_mode`     | `static`                           | `live` for a dataset that still receives captures, `static` for a finished one.    |
| `sort_order`         | `0`                                | Dashboard order. Lower sorts first.                                                |
| `source_mode`        | `subdirs`                          | `subdirs` reads member directories. `static` declares sources without directories. |

## CSV datasets

Dataset mode reads nfcapd captures only. For CSV, use a [pipeline configuration](setup-pipeline.md#build-from-csv-or-a-config-file) and write the output to `data/<dataset-id>/netflow.sqlite` so the dashboard finds it.

## Use a different registry file

Pass `--datasets /path/to/datasets.json` to a pipeline command, or export `DATASETS_CONFIG_PATH` in the shell. The pipeline does not read `.env`.
