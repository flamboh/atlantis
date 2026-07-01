"""Package entrypoint for extract-window commands."""

from __future__ import annotations

import sys
from pathlib import Path


if __package__:
    from .cli import main
else:
    netflow_db_tools = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(netflow_db_tools))
    from extract_window.cli import main


if __name__ == "__main__":
    main()
