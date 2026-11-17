"""Make apifuzz/hooks.py importable however pytest is started (repo root or apifuzz/)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
