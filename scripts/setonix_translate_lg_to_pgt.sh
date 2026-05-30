#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

PROFILE_JSON="${PROFILE_JSON:-src/app/core/orchestration/slurm_client/exampleprofile.json}"
LG_INPUT="${1:-wallaby-hires_deploy-setonix.graph}"
echo "Translation" >&2
PGT_OUT="$(python3 "${ROOT}/scripts/setonix_tm_translate.py" "${LG_INPUT}" --profile "${PROFILE_JSON}")"
echo "PGT: ${PGT_OUT}" >&2

if [[ "${WRITE_INI:-0}" == "1" ]]; then
  dep_root="$(python3 -c "import json; print(json.load(open('${PROFILE_JSON}'))['deployment']['dlg_root'].rstrip('/'))")"
  remote_staging="${dep_root}/staging"
  name="$(basename "${PGT_OUT}")"
  INI_LOCAL="${PGT_OUT%.graph}.ini"
  if [[ "${INI_LOCAL}" == "${PGT_OUT}" ]]; then
    INI_LOCAL="${PGT_OUT%.pgt.graph}.ini"
  fi
  REMOTE_PGT="${remote_staging}/${name}"
  REMOTE_INI="${remote_staging}/$(basename "${INI_LOCAL}")"
  echo "Writing local INI  ${INI_LOCAL} ---" >&2
  PYTHONPATH=src python3 "${ROOT}/scripts/setonix_render_dlg_ini.py" \
    --profile "${PROFILE_JSON}" \
    --pgt-remote "${REMOTE_PGT}" \
    -o "${INI_LOCAL}"
fi
