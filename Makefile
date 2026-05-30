COMPOSE ?= $(shell command -v docker >/dev/null 2>&1 && echo docker compose || echo podman compose)
COMPOSE_FILE ?= docker-compose.yml
ENV_FILE ?= .env

BEAMPIPE_BUILD ?=

SLURM_SSH_PRIVATE_KEY_FILE ?= ./deploy/ssh/id_slurm

API_URL ?= http://127.0.0.1:8000
RESTATE_ADMIN_URL ?= http://127.0.0.1:9070

.DEFAULT_GOAL := help

COMPOSE_UP_FLAGS := --remove-orphans -d
ifeq ($(BEAMPIPE_BUILD),1)
COMPOSE_UP_FLAGS += --build
endif

.PHONY: help dev logs ps urls preflight \
	compose-up compose-build compose-down \
	beampipe-start beampipe-stop beampipe-new-admin \
	migrate restate-start restate-stop \
	slurm-known-hosts-sync slurm-key-check openapi docs-copy docs-build docs-serve

help:
	@echo "Beampipe Core
	@echo ""
	@echo "Local development:"
	@echo "  dev                  compose up + create first superuser + print URLs"
	@echo "  logs                 Tail logs from all services"
	@echo "  ps                   Show running services and ports"
	@echo "  urls                 Print useful URLs (API docs, sources UI, Restate admin)"
	@echo ""
	@echo "Production:"
	@echo "  beampipe-start       "
	@echo "  beampipe-stop        "
	@echo "  beampipe-new-admin   Run init job to create the first superuser"
	@echo ""
	@echo "Compose:"
	@echo "  compose-up           Start compose (no init jobs)"
	@echo "  compose-build        Build compose images (also: BEAMPIPE_BUILD=1)"
	@echo "  compose-down         Stop compose"
	@echo "  migrate              Run Alembic migration"
	@echo ""
	@echo "Restate / Slurm:"
	@echo "  restate-start        Start Restate node(s); auto-detects single vs cluster"
	@echo "  restate-stop         Stop Restate node(s)"
	@echo "  slurm-known-hosts-sync  Copy ~/.ssh/known_hosts -> ./deploy/ssh/known_hosts (644)"
	@echo "  slurm-key-check      Verify $(SLURM_SSH_PRIVATE_KEY_FILE) exists"
	@echo "  openapi              Regenerate openapi.json"
	@echo "  docs-copy            Copy openapi.json into boilerplate_docs/ for ReDoc"
	@echo "  docs-build           docs-copy + mkdocs build --strict"
	@echo "  docs-serve           docs-copy + mkdocs serve"
	@echo ""
	@echo "VAR: BEAMPIPE_BUILD=1 forces a rebuild on compose-up / dev / beampipe-start."

# ---------------------------------------------------------------------------
# Local development
# ---------------------------------------------------------------------------

dev: preflight compose-up migrate beampipe-new-admin urls

preflight:
	@if [ ! -f "$(ENV_FILE)" ]; then \
		echo "Missing $(ENV_FILE) - run: python setup.py local" >&2; \
		exit 1; \
	fi
	@if [ ! -f "$(COMPOSE_FILE)" ]; then \
		echo "Missing $(COMPOSE_FILE) - run: python setup.py local" >&2; \
		exit 1; \
	fi

logs:
	$(COMPOSE) logs -f --tail=100

ps:
	$(COMPOSE) ps

urls:
	@echo ""
	@echo "Beampipe Core is up:"
	@echo "  API docs:      $(API_URL)/docs"
	@echo "  Sources UI:    $(API_URL)/sources       (only when ENVIRONMENT=local)"
	@echo "  Readiness:     $(API_URL)/api/v1/ready"
	@echo "  Restate admin: $(RESTATE_ADMIN_URL)"
	@echo ""
	@echo "Log in with the admin and password you set during 'python setup.py'."

# ---------------------------------------------------------------------------
# Production
# ---------------------------------------------------------------------------

beampipe-start: preflight
	@if grep -q "slurm_ssh_key" "$(COMPOSE_FILE)" 2>/dev/null; then \
		$(MAKE) slurm-known-hosts-sync; \
	else \
		echo "Slurm SSH not wired in $(COMPOSE_FILE); skipping known_hosts sync."; \
	fi
	$(COMPOSE) up $(COMPOSE_UP_FLAGS)

beampipe-stop:
	$(COMPOSE) down

beampipe-new-admin:
	$(COMPOSE) --profile init run --rm create_superuser

# ---------------------------------------------------------------------------
# Compose
# ---------------------------------------------------------------------------

compose-up:
	$(COMPOSE) up $(COMPOSE_UP_FLAGS)

compose-build:
	$(COMPOSE) build

compose-down:
	$(COMPOSE) down

migrate:
	$(COMPOSE) --profile init run --rm migrate

# ---------------------------------------------------------------------------
# Restate / Slurm
# ---------------------------------------------------------------------------

# Detect them dynamically from the compose file rather than hardcoding.
restate-start:
	@svcs=$$( $(COMPOSE) config --services 2>/dev/null | grep -E '^restate(-[0-9]+)?$$' | tr '\n' ' '); \
	if [ -z "$$svcs" ]; then echo "no restate services found in $(COMPOSE_FILE)" >&2; exit 1; fi; \
	echo "Starting: $$svcs"; \
	$(COMPOSE) up -d $$svcs

restate-stop:
	@svcs=$$( $(COMPOSE) config --services 2>/dev/null | grep -E '^restate(-[0-9]+)?$$' | tr '\n' ' '); \
	if [ -z "$$svcs" ]; then echo "no restate services found in $(COMPOSE_FILE)" >&2; exit 1; fi; \
	echo "Stopping: $$svcs"; \
	$(COMPOSE) stop $$svcs

slurm-known-hosts-sync:
	@$(CURDIR)/scripts/sync-slurm-known-hosts.sh

slurm-key-check:
	@key="$(SLURM_SSH_PRIVATE_KEY_FILE)"; \
	if [ ! -f "$$key" ]; then \
		echo "Slurm SSH key missing: $$key" >&2; \
		echo "Generate with:  ssh-keygen -t ed25519 -f $$key -N \"\"" >&2; \
		echo "Then add $$key.pub to ~/.ssh/authorized_keys on the head end." >&2; \
		exit 1; \
	fi; \
	mode=$$(stat -f '%Lp' "$$key" 2>/dev/null || stat -c '%a' "$$key" 2>/dev/null); \
	case "$$mode" in \
		400|600) echo "Slurm bot key OK ($$key, mode $$mode)";; \
		*) echo "Slurm bot key has unsafe perms ($$key, mode $$mode); run: chmod 600 $$key" >&2; exit 1;; \
	esac

openapi:
	uv run python scripts/export_openapi.py

docs-copy:
	cp openapi.json boilerplate_docs/openapi.json

docs-build: docs-copy
	uv run mkdocs build --strict

docs-serve: docs-copy
	uv run mkdocs serve

