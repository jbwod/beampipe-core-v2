COMPOSE ?= $(shell command -v docker >/dev/null 2>&1 && echo docker compose || echo podman compose)

BRIDGE_SCRIPT := $(CURDIR)/scripts/podman-ssh-agent-bridge.sh
BRIDGE_LOGDIR := $(CURDIR)/.logs
BRIDGE_PIDFILE := $(CURDIR)/.podman-bridge.pid

BEAMPIPE_BUILD ?=

.DEFAULT_GOAL := help

COMPOSE_UP_FLAGS := --remove-orphans -d
ifeq ($(BEAMPIPE_BUILD),1)
COMPOSE_UP_FLAGS += --build
endif

.PHONY: help beampipe-start beampipe-stop slurm-known-hosts-sync \
	podman-bridge podman-bridge-bg podman-bridge-ensure podman-bridge-stop podman-bridge-status \
	compose-up compose-build compose-down compose-logs

help:
	@echo "  beampipe-start       Start known_hosts sync, SSH bridge, and compose"
	@echo "  beampipe-stop        Stop compose and SSH bridge"
	@echo "  podman-bridge        Run SSH bridge in foreground"
	@echo "  podman-bridge-bg     Run SSH bridge in background"
	@echo "  podman-bridge-stop   Stop SSH bridge"
	@echo "  podman-bridge-status Check SSH bridge"
	@echo "  compose-up           Start compose"
	@echo "  compose-build        Build compose"
	@echo "  compose-down         Stop compose"
	@echo "  compose-logs         Tail SSH bridge log"

beampipe-start: slurm-known-hosts-sync podman-bridge-ensure
	$(COMPOSE) up $(COMPOSE_UP_FLAGS)

beampipe-stop:
	$(COMPOSE) down
	@$(MAKE) podman-bridge-stop

podman-bridge:
	@test -n "$$SSH_AUTH_SOCK" || (echo "SSH_AUTH_SOCK is not set." >&2 && exit 1)
	@$(BRIDGE_SCRIPT)

podman-bridge-bg:
	@test -n "$$SSH_AUTH_SOCK" || (echo "SSH_AUTH_SOCK is not set." >&2 && exit 1)
	@mkdir -p "$(BRIDGE_LOGDIR)"
	@if [ -f "$(BRIDGE_PIDFILE)" ] && kill -0 "$$(cat "$(BRIDGE_PIDFILE)")" 2>/dev/null; then \
		echo "Bridge already running (PID $$(cat "$(BRIDGE_PIDFILE")))." >&2; \
		exit 1; \
	fi
	@nohup "$(BRIDGE_SCRIPT)" >"$(BRIDGE_LOGDIR)/podman-bridge.log" 2>&1 & echo $$! >"$(BRIDGE_PIDFILE)"
	@echo "Bridge PID $$(cat "$(BRIDGE_PIDFILE)")"

podman-bridge-ensure:
	@test -n "$$SSH_AUTH_SOCK" || (echo "SSH_AUTH_SOCK is not set." >&2 && exit 1)
	@set -e; \
	mkdir -p "$(BRIDGE_LOGDIR)"; \
	if [ -f "$(BRIDGE_PIDFILE)" ]; then \
		pid=$$(cat "$(BRIDGE_PIDFILE)"); \
		if kill -0 $$pid 2>/dev/null; then \
			echo "Bridge already running (PID $$pid)."; \
			exit 0; \
		fi; \
		rm -f "$(BRIDGE_PIDFILE)"; \
	fi; \
	nohup "$(BRIDGE_SCRIPT)" >"$(BRIDGE_LOGDIR)/podman-bridge.log" 2>&1 & echo $$! >"$(BRIDGE_PIDFILE)"; \
	echo "Started bridge PID $$(cat "$(BRIDGE_PIDFILE)")"; \
	sleep 2; \
	podman machine ssh 'SSH_AUTH_SOCK=/tmp/beampipe-agent-relay.sock ssh-add -l' >/dev/null \
		|| (echo "VM agent check failed; see $(BRIDGE_LOGDIR)/podman-bridge.log" >&2; exit 1)

podman-bridge-stop:
	@if [ ! -f "$(BRIDGE_PIDFILE)" ]; then \
		echo "Bridge not running."; \
		exit 0; \
	fi
	@pid=$$(cat "$(BRIDGE_PIDFILE)"); \
	if kill -0 $$pid 2>/dev/null; then \
		kill $$pid && echo "Stopped bridge PID $$pid"; \
	else \
		echo "Bridge PID $$pid not running"; \
	fi; \
	rm -f "$(BRIDGE_PIDFILE)"

podman-bridge-status:
	@if [ -f "$(BRIDGE_PIDFILE)" ]; then \
		pid=$$(cat "$(BRIDGE_PIDFILE)"); \
		if kill -0 $$pid 2>/dev/null; then \
			echo "Bridge running (PID $$pid)"; \
		else \
			echo "Stale bridge PID file ($$pid)"; \
		fi; \
	else \
		echo "Bridge not running"; \
	fi
	@podman machine ssh 'SSH_AUTH_SOCK=/tmp/beampipe-agent-relay.sock ssh-add -l' 2>/dev/null \
		|| echo "VM agent check failed"

compose-up:
	$(COMPOSE) up -d

compose-build:
	$(COMPOSE) build

compose-down:
	$(COMPOSE) down

compose-logs:
	@test -f "$(BRIDGE_LOGDIR)/podman-bridge.log" || (echo "No bridge log found." >&2 && exit 1)
	tail -f "$(BRIDGE_LOGDIR)/podman-bridge.log"