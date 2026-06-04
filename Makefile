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

docs-copy: openapi
	cp openapi.json boilerplate_docs/openapi.json

docs-build: docs-copy
	python3 -m mkdocs build --strict

docs-serve: docs-copy
	python3 -m mkdocs serve
