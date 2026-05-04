#!/usr/bin/env python3

import argparse
import json
import os
import sys
from pathlib import Path

import httpx


def partitioned_pgt_for_dlg_deploy(pgt_json: object, lg_name: str) -> list[object]:
    if (
        isinstance(pgt_json, list)
        and len(pgt_json) == 2
        and isinstance(pgt_json[0], str)
        and isinstance(pgt_json[1], list)
    ):
        return pgt_json
    base = lg_name.rsplit("/", 1)[-1]
    if base.endswith(".graph"):
        pgt_filename = base[: -len(".graph")] + "_pgt.graph"
    else:
        pgt_filename = f"{base}.pgt.graph"
    return [pgt_filename, pgt_json]


def main() -> int:
    parser = argparse.ArgumentParser(description="LG → PGT via Translation Manager")
    parser.add_argument(
        "lg_path",
        type=Path,
        nargs="?",
        default=Path("wallaby-hires_deploy-setonix.graph"),
        help="Path to EAGLE logical graph JSON",
    )
    parser.add_argument(
        "-p",
        "--profile",
        type=Path,
        default=Path("src/app/core/orchestration/slurm_client/exampleprofile.json"),
        help="exampleprofile",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output PGT",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=300.0,
        help="timeout seconds",
    )
    args = parser.parse_args()

    lg_path = args.lg_path.resolve()
    if not lg_path.is_file():
        print(f"ERROR: LG not found: {lg_path}", file=sys.stderr)
        return 1

    profile_path = args.profile.resolve()
    if not profile_path.is_file():
        print(f"ERROR: profile not found: {profile_path}", file=sys.stderr)
        return 1

    prof = json.loads(profile_path.read_text())
    tr = prof.get("translation") or {}
    tm_url = os.environ.get("TM_URL") or tr.get("tm_url") or "http://dlg-tm.desk"
    algo = os.environ.get("TM_ALGO") or tr.get("algo") or "metis"
    num_par = int(os.environ.get("TM_NUM_PAR") or tr.get("num_par") or 1)
    num_islands = int(os.environ.get("TM_NUM_ISLANDS") or tr.get("num_islands") or 1)
    if num_par < 1:
        num_par = 1
    if num_islands < 1:
        num_islands = 1

    lg = json.loads(lg_path.read_text())
    data = {
        "lg_content": json.dumps(lg),
        "num_partitions": str(num_par),
        "num_islands": str(num_islands),
        "algorithm": algo,
    }
    base = tm_url.rstrip("/")
    url = f"{base}/unroll_and_partition"
    print(
        f"POST {url}  algo={algo}  num_partitions={num_par}  num_islands={num_islands}",
        file=sys.stderr,
    )

    resp = httpx.post(url, data=data, timeout=args.timeout)
    resp.raise_for_status()
    raw = resp.json()

    lg_name = lg_path.name
    pgt_json = partitioned_pgt_for_dlg_deploy(raw, lg_name)
    out_name = pgt_json[0] if isinstance(pgt_json[0], str) else None
    if args.output:
        out_path = args.output.resolve()
    else:
        out_path = (lg_path.parent / out_name) if out_name else lg_path.with_name(
            lg_path.stem + "_pgt.graph"
        )

    out_path.write_text(json.dumps(pgt_json, indent=2))
    print(str(out_path))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
