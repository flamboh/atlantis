# Dataset configuration

The pipeline reads local dataset definitions from `datasets.json`.

The pipeline copies public dataset metadata into each output database. The web application reads that database metadata.

## Create the file

1. Copy the example file.

   ```bash
   cp datasets.json.example datasets.json
   ```

2. If `.env` does not exist, copy the environment template.

   ```bash
   cp .env.example .env
   ```

3. Set `root_path` to the directory that contains your nfcapd captures.

4. List one entry in `sources` for each collector directory under `root_path`.

5. Set `DEFAULT_DATASET` in `.env` to your `dataset_id`.

The repository ignores `datasets.json`. Do not commit paths that are specific to your computer.

## Minimal dataset

This dataset reads captures from one collector directory:

```json
[
  {
    "dataset_id": "example",
    "root_path": "/data/netflow/example",
    "sources": [{ "source_id": "router-a", "members": ["router-a"] }]
  }
]
```

The pipeline writes the database to `data/<dataset-id>/netflow.sqlite`. The dashboard automatically discovers databases at that location.

## Dataset with logical sources

A logical source combines the captures from more than one collector directory. Each name in `members` is a directory under `root_path`. This dataset shows two collectors and their combination:

```json
{
  "dataset_id": "example",
  "label": "Example",
  "root_path": "/data/netflow/example",
  "sources": [
    { "source_id": "router-a", "members": ["router-a"] },
    { "source_id": "router-b", "members": ["router-b"] },
    { "source_id": "all-routers", "members": ["router-a", "router-b"] }
  ],
  "discovery_mode": "live",
  "sort_order": 10
}
```

## Classify internal and external endpoints

`locality` decides which flow endpoints belong to your network. The pipeline marks an endpoint
`internal` when any rule matches it. Every other endpoint is `external`. Rule order does not matter.

```json
{
  "dataset_id": "campus",
  "root_path": "/data/netflow/campus",
  "sources": [{ "source_id": "router-a", "members": ["router-a"] }],
  "locality": [
    { "type": "prefixes", "prefixes": ["192.0.2.0/24", "2001:db8::/32"] },
    { "type": "addresses", "addresses": ["198.51.100.53"] },
    { "type": "addresses", "path": "~/private/internal-addresses.txt" }
  ]
}
```

| Rule type        | Fields                | Matches                                                                                                             |
| ---------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `prefixes`       | `prefixes`            | Endpoints inside any listed IPv4 or IPv6 CIDR. Host bits must be zero.                                              |
| `addresses`      | `addresses` or `path` | Exact IPv4 or IPv6 addresses. Use exactly one field.                                                                |
| `tos_anonymized` | None                  | Endpoints that the UOregon anonymizer flagged in the two low source-ToS bits. Other networks do not set these bits. |

An address file lists one address per line. Blank lines and text after `#` are ignored. A relative
`path` uses the repository root. Keep private address lists outside the repository.

Validation is strict. The pipeline rejects unknown rule types, unknown fields, empty lists, and values
that do not parse.

Each flow gets a direction from its source and destination localities:

| Source   | Destination | Direction |
| -------- | ----------- | --------- |
| internal | external    | Egress    |
| external | internal    | Ingress   |
| internal | internal    | Lateral   |
| external | external    | Transit   |

Transit traffic should be rare for a border collector. A large transit share usually means the rules
miss part of your address space.

Without `locality`, every endpoint is external and all traffic is transit. Selections that filter on
locality, including `daily_active_sources`, require rules.

The rules are part of the product identity. Adding, removing, or editing a rule, including the
contents of an address file, requires a new database. The identity stores only a count and digest of
each address and prefix list, so the lists do not appear in the database.

## Define coordinated subsets

Give each subset its own registry entry. Repeat those dataset IDs in one pipeline command. Each entry
defines its own logical sources and `daily_active_sources` selection.

```json
[
  {
    "dataset_id": "campus-a",
    "root_path": "/data/netflow/campus",
    "source_ids": ["router-a"],
    "locality": [{ "type": "prefixes", "prefixes": ["192.0.2.0/24"] }],
    "selection": {
      "kind": "daily_active_sources",
      "ip_prefix": "192.0.2.0/25"
    },
    "db_path": "data/campus-a/netflow.sqlite"
  },
  {
    "dataset_id": "campus-b",
    "root_path": "/data/netflow/campus",
    "source_ids": ["router-a"],
    "locality": [{ "type": "prefixes", "prefixes": ["192.0.2.0/24"] }],
    "selection": {
      "kind": "daily_active_sources",
      "ip_prefix": "192.0.2.128/25"
    },
    "db_path": "data/campus-b/netflow.sqlite"
  }
]
```

The entries share a capture root and the same logical source layout. Their active sets remain independent,
so one flow may publish to both products when the selections overlap. Do not add a parent or
`source_dataset` relation. The multi-dataset command infers each subset from the selected entry and
uses each entry's `db_path`.

## Required fields

| Field        | Purpose                                         |
| ------------ | ----------------------------------------------- |
| `dataset_id` | The stable ID for routes and pipeline commands. |
| `root_path`  | The directory that contains the input data.     |

## Optional fields

| Field                | Default                            | Purpose                                                                                                                        |
| -------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `label`              | A title from `dataset_id`          | The user-visible name in the dashboard.                                                                                        |
| `db_path`            | `data/<dataset-id>/netflow.sqlite` | The SQLite output path.                                                                                                        |
| `default_start_date` | The earliest day that has data     | The first date that the dashboard shows.                                                                                       |
| `source_mode`        | `subdirs`                          | `subdirs` reads member directories under `root_path`. `static` declares sources without directories.                           |
| `sources`            | None                               | Logical sources and their physical member directories.                                                                         |
| `source_ids`         | None                               | Simple source names for datasets without member directories.                                                                   |
| `discovery_mode`     | `static`                           | `live` marks a dataset that continues to receive new captures. `static` marks a complete dataset.                              |
| `sort_order`         | `0`                                | The dataset order in the dashboard. Lower values sort first.                                                                   |
| `selection`          | All flows                          | A normalized flow-selection object applied automatically by dataset-mode pipeline runs.                                        |
| `locality`           | No rules; every endpoint external  | Rules that mark endpoints internal. See [Classify internal and external endpoints](#classify-internal-and-external-endpoints). |

Set `db_path` only for a database that must stay separate, such as a [flow selection](setup-pipeline.md#select-flows) product.
Persist `selection` with a dedicated `db_path` when the dataset is itself a selected product. Command-line
selection flags may only override it when `--database-path` names a different output product.

Each pipeline run calculates `default_start_date` again. A run that adds earlier days moves the date back. Set the field to hold the dashboard at one date.

Do not define `sources` and `source_ids` in the same dataset.

## Input directory layout

Native nfcapd data uses this layout:

```text
<root_path>/
  <member-id>/
    YYYY/
      MM/
        DD/
          nfcapd.YYYYMMddHHmm
```

Each member in `sources` must have a top-level directory. The pipeline stops if a member directory does not exist.

## CSV datasets

Dataset mode reads nfcapd directories only. To build a database from CSV input, use a [pipeline configuration](setup-pipeline.md#use-a-pipeline-configuration) with an explicit `--database-path`. A CSV-built database at `data/<dataset-id>/netflow.sqlite` appears in the dashboard like any other database.

## Other configuration locations

Set `DATASETS_CONFIG_PATH` to use a different registry file.

The web application reads it from `.env`:

```dotenv
DATASETS_CONFIG_PATH=/absolute/path/to/datasets.json
```

The pipeline reads it from the process environment only. Export it in the shell before a pipeline command:

```bash
export DATASETS_CONFIG_PATH=/absolute/path/to/datasets.json
```

Relative `db_path` values use the repository root. The web application also scans `data/*/netflow.sqlite` for local databases.
