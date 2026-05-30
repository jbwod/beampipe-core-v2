# --------- Builder Stage ---------
FROM ghcr.io/astral-sh/uv:python3.11-bookworm-slim AS builder

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy
ENV UV_CACHE_DIR=/tmp/uv-cache

WORKDIR /app

COPY pyproject.toml uv.lock README.md ./
COPY beampipe-wallaby /app/beampipe-wallaby

RUN uv venv --seed \
    && /app/.venv/bin/python -m pip install --no-cache-dir --upgrade setuptools wheel hatchling

RUN --mount=type=cache,target=/tmp/uv-cache,sharing=locked \
    uv sync --preview-features extra-build-dependencies --locked --no-install-project --all-groups --extra wallaby --no-build-isolation

# --------- Final Stage ---------
FROM python:3.11-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 1000 app \
    && useradd --uid 1000 --gid app --shell /bin/bash --create-home app

COPY --from=builder --chown=app:app /app/.venv /app/.venv
ENV PATH="/app/.venv/bin:$PATH"

USER app
WORKDIR /code


CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--reload"]
