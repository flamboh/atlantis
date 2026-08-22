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

## Required fields

| Field        | Purpose                                         |
| ------------ | ----------------------------------------------- |
| `dataset_id` | The stable ID for routes and pipeline commands. |
| `root_path`  | The directory that contains the input data.     |

## Optional fields

| Field                | Default                            | Purpose                                                                                              |
| -------------------- | ---------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `label`              | A title from `dataset_id`          | The user-visible name in the dashboard.                                                              |
| `db_path`            | `data/<dataset-id>/netflow.sqlite` | The SQLite output path.                                                                              |
| `default_start_date` | The earliest day that has data     | The first date that the dashboard shows.                                                             |
| `source_mode`        | `subdirs`                          | `subdirs` reads member directories under `root_path`. `static` declares sources without directories. |
| `sources`            | None                               | Logical sources and their physical member directories.                                               |
| `source_ids`         | None                               | Simple source names for datasets without member directories.                                         |
| `discovery_mode`     | `static`                           | `live` marks a dataset that continues to receive new captures. `static` marks a complete dataset.    |
| `sort_order`         | `0`                                | The dataset order in the dashboard. Lower values sort first.                                         |

Set `db_path` only for a database that must stay separate, such as a [flow selection](setup-pipeline.md#select-flows) product.

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
