# Glossary

Use these terms consistently across project configuration, API responses, the console, and incident notes.

| Term | Meaning |
|---|---|
| Adapter | Typed boundary between Beampipe and an external archive, scheduler, translator, or deployment manager. |
| Admission | Decision that a discovered source is eligible to become an execution under automation and concurrency limits. |
| Artifact | Immutable manifest, logical graph, patched graph, translated graph, or run record tied to an execution. |
| CASDA | CSIRO ASKAP Science Data Archive, queried through TAP for source and dataset metadata. |
| Claim | Time-bounded worker ownership of a durable job. Claims carry a fencing token. |
| Control phase | Precise internal stage of an execution, from discovery through output verification. |
| DALiuGE | Data Activated Liu Graph Engine, used to translate and execute scientific graphs. |
| Deployment profile | Versioned translator, manager, scheduler, resource, TLS, and facility configuration pinned by an execution. |
| Diagnostic | Structured message with `path`, `severity`, `code`, `message`, and optional `hint`. |
| Discovery signature | Stable digest of prepared archive metadata used to decide whether relevant source state changed. |
| External axis | Independently observed submission, scheduler, DALiuGE, or output-verification state. |
| Fencing token | Monotonic claim value that prevents a stale worker from committing effects after ownership changed. |
| Graph patch | Validated deterministic mutation applied to a logical DALiuGE graph before translation. |
| Manifest | Project-shaped source and dataset document generated from prepared discovery metadata. |
| Provenance | Append-only narrative of meaningful source, execution, worker, backend, and operator events. |
| Reconciliation | Comparison of durable intent with external facts to derive the next safe action. |
| Run record | Persisted backend detail, identifiers, poll history, and excerpts associated with an execution. |
| Source | Stable project identity for an astronomical target or other unit of discovery. |
| Submission uncertainty | State where Beampipe attempted external submission but cannot yet prove whether it succeeded. |

The [architecture map](../architecture/index.md) shows how these concepts relate. The [execution state model](../architecture/state-machine.md) defines exact state values.
