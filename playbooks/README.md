# Beampipe demonstration playbooks

The canonical demonstration is [REST no-downloads](rest_remote_no_downloads.md). It is a terminal-first runbook using the compiled `beampipe` binary, `curl`, and `jq`; no notebook or Python orchestration is required.

The playbook uses these reviewable inputs:

- `config/rest_remote.local.json`: REST deployment profile template.
- `config/wallaby_hires_no_downloads_rest.v2.yaml`: project-defined discovery, metadata, graph, and automation policy.

Run every command from the repository root. Runtime files and evidence are written under the ignored `playbook-runs/` directory.
