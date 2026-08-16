.DEFAULT_GOAL := help

.PHONY: help openapi docs-copy docs-build docs-serve

help:
	@echo "Documentation:"
	@echo "  openapi       Regenerate openapi.json from Rust utoipa spec"
	@echo "  docs-copy     Copy openapi.json into boilerplate_docs/ for ReDoc"
	@echo "  docs-build    docs-copy + mkdocs build --strict"
	@echo "  docs-serve    docs-copy + mkdocs serve"

openapi:
	./scripts/export-openapi.sh

docs-copy:
	@if command -v cargo >/dev/null 2>&1; then \
		$(MAKE) openapi && cp openapi.json boilerplate_docs/openapi.json; \
	elif [ -f boilerplate_docs/openapi.json ]; then \
		echo "cargo unavailable; using committed boilerplate_docs/openapi.json"; \
	else \
		echo "need cargo or a committed boilerplate_docs/openapi.json" >&2; \
		exit 1; \
	fi

docs-build: docs-copy
	@python3 -c "import mkdocs" >/dev/null 2>&1 || python3 -m pip install -r requirements-docs.txt
	python3 -m mkdocs build --strict

docs-serve: docs-copy
	python3 -m mkdocs serve
