import csv
import re
from collections import Counter, defaultdict

from astroquery.utils.tap.core import TapPlus

CASDA_TAP_URL = "https://casda.csiro.au/casda_vo_tools/tap"
OUT_CSV = "dev/hipass_source_sbid_beam_counts.csv"

ADQL = """
SELECT o.obs_id, o.filename, o.obs_collection, o.t_max
FROM ivoa.obscore o
WHERE o.filename LIKE 'HIPASS%'
  AND o.obs_collection IN ('ASKAP Pilot Survey for WALLABY', 'WALLABY')
""".strip()

_BEAM = re.compile(r"(?:^|_)beam(?P<n>\d+)(?:_|\b)", re.I)
_BXX = re.compile(r"(?:^|_)B(?P<n>\d{1,3})(?:_|\b)")


def sbid(obs_id: str) -> str | None:
    s = (obs_id or "").strip().removeprefix("ASKAP-")
    return s if s.isdigit() else None


def source_id(filename: str) -> str | None:
    s = (filename or "").strip()
    return s.split("_", 1)[0] if s else None


def beam_id(filename: str) -> str | None:
    for pat in (_BEAM, _BXX):
        m = pat.search(filename or "")
        if m:
            return f"beam{int(m.group('n')):02d}"
    return None


def main() -> None:
    res = TapPlus(url=CASDA_TAP_URL, verbose=False).launch_job_async(ADQL).get_results()

    counts: Counter[tuple[str, str]] = Counter()
    beams: dict[tuple[str, str], set[str]] = defaultdict(set)
    collections: dict[tuple[str, str], set[str]] = defaultdict(set)
    latest_tmax: dict[tuple[str, str], float] = {}

    for r in res:
        sid = sbid(str(r["obs_id"]))
        src = source_id(str(r["filename"]))
        if not sid or not src:
            continue
        key = (src, sid)
        counts[key] += 1
        if b := beam_id(str(r["filename"])):
            beams[key].add(b)
        if col := str(r["obs_collection"] or "").strip():
            collections[key].add(col)
        if r["t_max"] is not None:
            t = float(r["t_max"])
            if t > latest_tmax.get(key, float("-inf")):
                latest_tmax[key] = t

    with open(OUT_CSV, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(
            [
                "source_identifier",
                "sbid",
                "n_visibilities",
                "n_unique_beams",
                "beam_ids",
                "collections",
                "latest_t_max",
            ]
        )
        for src, sid in sorted(counts, key=lambda k: (-counts[k], k[0], k[1])):
            b = sorted(beams.get((src, sid), ()))
            w.writerow(
                [
                    src,
                    sid,
                    counts[(src, sid)],
                    len(b),
                    "|".join(b),
                    "|".join(sorted(collections.get((src, sid), ()))),
                    latest_tmax.get((src, sid)),
                ]
            )


if __name__ == "__main__":
    main()