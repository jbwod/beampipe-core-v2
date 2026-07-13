# Integration adapters

Integration code belongs behind explicit boundaries so archive, workflow-engine, and
scheduler failures can be normalized without leaking vendor assumptions into project or
execution state.

## Archive adapter

`beampipe-adapters` defines `TapClient::query_rows` and `health`. Add an archive client
there, return `AdapterError` variants that distinguish transient, permanent, timeout,
empty-result, and invalid-row failures, and keep service-specific parsing in its own
module. Register the adapter in the discovery runner rather than branching generic
manifest or scheduling code.

Required tests:

- recorded or simulated success and empty responses;
- timeout, unavailable service, HTTP failure, and malformed VOTable/JSON;
- deterministic row normalization and health behavior;
- project-query validation through a v2 config fixture.

## Scheduler adapter

`beampipe-orchestration::SchedulerAdapter` covers connectivity, resource requests,
submit, status, batch status, cancellation, accounting, queue/capacity information, and
log paths. New schedulers must return normalized `SchedulerState`, typed external IDs,
and a `SchedulerAdapterError` with retry classification.

Keep submission intent and stable external identity in PostgreSQL before I/O. A timeout
after submission must become `SubmissionUncertain`, followed by reconciliation; it must
not be inferred as either success or failure from a missing response.

## DALiuGE adapter

The typed `DaliugeTranslator` and `DaliugeManager` traits wrap the lower-level translator
and manager clients. Implement health/compatibility, translation, session lifecycle,
inspection, cancellation, and bounded error excerpts. Contract tests must use a mock
Translator Manager/Data Island Manager and fixtures verified against a pinned upstream
DALiuGE revision.

Run before review:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
