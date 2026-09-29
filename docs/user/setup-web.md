# Web setup

Install the project and run the dashboard locally. You need the tools in [Requirements](requirements.md) (Git, Bun, Node.js).

## Install

```bash
git clone https://github.com/flamboh/atlantis.git
cd atlantis
bun install --frozen-lockfile
```

## Run the dashboard

You need a database first: [configure a dataset](datasets.md), then [build it](setup-pipeline.md#build-a-database).

```bash
cp -n .env.example .env
bun run dev:web
```

Open `http://localhost:5173`. The dashboard shows one card per database found at `data/<dataset-id>/netflow.sqlite`. Select a card to see the charts.

`.env` settings, all optional:

| Variable            | Purpose                                                      |
| ------------------- | ------------------------------------------------------------ |
| `DEFAULT_DATASET`   | Dataset opened by default. Otherwise the first one found.    |
| `LOCAL_SQLITE_PATH` | Serve one database at this path instead of scanning `data/`. |
| `LOCAL_DATA_DIR`    | Scan this directory instead of `data/`.                      |

`bun run dev` also starts the landing site at `http://localhost:4321`.

## Use a remote development host

Run the dashboard on the remote host, then tunnel it:

```bash
ssh -L 5173:localhost:5173 user@remote-host
```

Open `http://localhost:5173` on your computer.

To change code and run the project checks, read [Development](../code/development.md).
