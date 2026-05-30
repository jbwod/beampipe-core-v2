from collections import OrderedDict
import os
import random

from astroquery.utils.tap.core import TapPlus

CASDA_TAP_URL = "https://casda.csiro.au/casda_vo_tools/tap"

ADQL_QUERY = """
SELECT TOP 20000 filename, t_max, obs_id, obs_publisher_did
FROM ivoa.obscore
WHERE filename LIKE 'HIPASSJ%'
ORDER BY t_max DESC
""".strip()


def main() -> None:
    seed = os.environ.get("SEED")
    rng = random.Random(int(seed)) if seed is not None else random.Random()

    tap = TapPlus(url=CASDA_TAP_URL, verbose=False)
    job = tap.launch_job_async(ADQL_QUERY)
    res = job.get_results()
    print(f"rows {len(res)}")

    latest_by_source = OrderedDict()
    for row in res:
        src = str(row["filename"]).split("_", 1)[0]  # HIPASS source id prefix
        if src not in latest_by_source:
            latest_by_source[src] = row

    pairs = list(latest_by_source.items())
    rng.shuffle(pairs)
    pairs = pairs[:50]

    print(f"unique_sources_in_query {len(latest_by_source)}")
    print(f"random_unique_sources {len(pairs)}")
    for i, (src, row) in enumerate(pairs, start=1):
        print(i, src, row["t_max"], row["obs_id"])


if __name__ == "__main__":
    main()

