# Glossary

Use these terms consistently across project configuration, API responses, the console, and incident notes.

| Term | Meaning |
|---|---|
| Adapter | Named project boundary to an external catalog endpoint, scheduler, translator, or deployment manager. |
| Admission | Decision that a discovered source is eligible to become an execution under automation and concurrency limits. |
| Artifact | Immutable manifest, logical graph, patched graph, translated graph, or run record tied to an execution. |
| CASDA | Optional provider integration for the explicit WALLABY sample; not a Core identity or staging default. |
| Claim | Time-bounded worker ownership of a durable job. Claims carry a fencing token. |
| Control phase | Precise internal stage of an execution, from discovery through terminal reconciliation. |
| DALiuGE | Data Activated Liu Graph Engine, used to translate and execute scientific graphs. |
| Deployment profile | Versioned translator, manager, scheduler, resource, TLS, and facility configuration pinned by an execution. |
| Diagnostic | Structured message with `path`, `severity`, `code`, `message`, and optional `hint`. |
| Discovery signature | Stable digest of prepared archive metadata used to decide whether relevant source state changed. |
| External axis | Independently observed submission, scheduler, DALiuGE, or modeled output state. |
| Fencing token | Monotonic claim value that prevents a stale worker from committing effects after ownership changed. |
| Graph patch | Validated deterministic mutation applied to a logical DALiuGE graph before translation. |
| Group | Project-defined collection of prepared records within one source, identified in Core by `group_key`. |
| Manifest | Project-shaped source/group/record document generated from prepared discovery metadata. |
| Output inventory | Generic `beampipe-output-inventory/v1` report of durable products, hashes, destination, and publication acknowledgement. |
| Provenance | Append-only narrative of meaningful source, execution, worker, backend, and operator events. |
| Record | Smallest prepared discovery item, identified in Core by `record_id` and stored inside one group. |
| Reconciliation | Comparison of durable intent with external facts to derive the next safe action. |
| Run record | Persisted backend detail, identifiers, poll history, and excerpts associated with an execution. |
| Source | Stable project identity for an astronomical target or other unit of discovery. |
| Staging provider | Explicit `staging.provider` implementation; `none` is neutral pass-through and provider-specific protocols must be opted into. |
| Submission uncertainty | State where Beampipe attempted external submission but cannot yet prove whether it succeeded. |
| Runtime contract | Typed Slurm profile declaration of project commands, Python modules, environment requirements, and output/staging directories. |

The [operator handbook](../operations/index.md) shows how these concepts show up in triage. [Recovery and cancellation](../operations/recovery.md) covers uncertain and terminal work.
