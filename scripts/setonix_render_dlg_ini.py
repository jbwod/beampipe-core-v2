#!/usr/bin/env python3
import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument(
        "--pgt-remote",
        required=True,
        help="GRAPH PHYSICAL_GRAPH path as seen  on remote",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        required=True,
        help="Local INI file to write",
    )
    args = parser.parse_args()

    try:
        from app.core.orchestration.slurm import _render_generated_ini
    except ImportError as e:
        print(
            "ERROR: need Beampipe on PYTHONPATH (run from repo root):\n"
            "  PYTHONPATH=src python3 scripts/setonix_render_dlg_ini.py ...\n"
            f"  ({e})",
            file=sys.stderr,
        )
        return 1

    prof = json.loads(args.profile.read_text())
    dep = prof.get("deployment") or {}
    user = str(dep.get("remote_user") or "user")
    dlg_root = str(dep.get("dlg_root", "")).rstrip("/")

    ini = _render_generated_ini(
        deployment_config=dep,
        username=user,
        pgt_remote_path=args.pgt_remote,
        dlg_root=dlg_root,
    )
    args.output.write_text(ini)
    print(args.output.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
