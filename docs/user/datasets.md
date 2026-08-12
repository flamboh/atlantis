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

3. Replace each placeholder path with a path on your computer.

4. Make sure that each `dataset_id` is unique.

5. Set `DEFAULT_DATASET` to one of these IDs in `.env`.

The repository ignores `datasets.json`. Do not commit paths that are specific to your computer.

## Dataset example

```json
[
  {
    "dataset_id": "uoregon",
    "label": "UONet-in",
    "root_path": "/data/netflow/uoregon",
    "db_path": "./data/uoregon/netflow.sqlite",
    "default_start_date": "2025-02-11",
    "source_mode": "subdirs",
    "sources": [
      { "source_id": "router-a", "members": ["router-a"] },
      { "source_id": "router-b", "members": ["router-b"] },
      { "source_id": "all-routers", "members": ["router-a", "router-b"] }
    ],
    "discovery_mode": "live",
    "sort_order": 10
  }
]
```

## Required fields

| Field        | Purpose                                               |
| ------------ | ----------------------------------------------------- |
| `dataset_id` | Gives the stable ID for routes and pipeline commands. |
| `root_path`  | Gives the directory that contains the input data.     |
| `db_path`    | Gives the SQLite output path.                         |

## Optional fields

| Field                | Default                   | Purpose                                               |
| -------------------- | ------------------------- | ----------------------------------------------------- |
| `label`              | A title from `dataset_id` | Gives the user-visible name.                          |
| `default_start_date` | `2025-02-01`              | Gives the first date that the dashboard shows.        |
| `source_mode`        | `subdirs`                 | Identifies a directory-based or static source layout. |
| `sources`            | None                      | Defines logical sources and their physical members.   |
| `source_ids`         | None                      | Defines simple sources without logical member groups. |
| `discovery_mode`     | `static`                  | Identifies the dataset as `static` or `live`.         |
| `sort_order`         | `0`                       | Controls the dataset order in the dashboard.          |

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
