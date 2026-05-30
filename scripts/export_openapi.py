import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "src") not in sys.path:
    sys.path.insert(0, str(ROOT / "src"))

os.environ.setdefault("ENVIRONMENT", "local")


def main() -> int:
    from app.core.openapi import build_openapi_schema
    from app.main import app

    out = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "openapi.json"
    spec = build_openapi_schema(app)
    out.write_text(json.dumps(spec, indent=2, sort_keys=False) + "\n", encoding="utf-8")
    paths = len(spec.get("paths", {}))
    schemas = len(spec.get("components", {}).get("schemas", {}))
    print(f"Wrote {out} ({paths} paths, {schemas} schemas)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
