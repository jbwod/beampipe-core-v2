# Documentation map

This page records the documentation information architecture, article ownership, and intended reading flow. Existing public URLs remain stable; navigation labels and section placement carry the refinement.

## Information architecture

| Section | Reader question | Entry article |
|---|---|---|
| Start | How do I get a safe working system? | [Choose a path](../getting-started/index.md) |
| Operate | What should I inspect or do during a shift? | [Operator handbook](../operations/index.md) |
| Configure projects | How does a survey define discovery, manifests, and graphs? | [Project config YAML](../project-configs/index.md) |
| Understand | Why is the system shaped this way? | [Architecture map](../architecture/index.md) |
| Reference | What is the exact command, term, or API contract? | [CLI commands](../reference/cli.md) |
| Engineering | How do we change and maintain Beampipe? | [Contributing](../contributing.md) |

## Article map

| Existing article | Refined role | Primary audience |
|---|---|---|
| Home | Product boundary, route selection, shortest safe command path | Everyone |
| Five-minute local start | Mock-backed proof of the real control plane | New operator |
| Installation | Release, source, and container installation choices | Installer |
| First run | One complete source-to-execution walkthrough | New operator |
| Configuration | Environment variables and precedence reference | Operator |
| Operator guide | Normal daily workflow and role contract | Operator |
| Terminal console | Views, state sources, and controls | Operator |
| Workers and scheduling | Claims, pools, fairness, scaling, and drain | Platform operator |
| DALiuGE and Setonix | REST, SSH, Slurm, trust, and live inspection | HPC operator |
| Observability | Metrics, alerts, logs, and debug order | On-call operator |
| Recovery | Investigation, retry, cancellation, and worker recovery | On-call operator |
| Production runbook | Promotion and incident decision procedure | Release operator |
| Upgrades, backups, secrets | Lifecycle maintenance and restore rehearsal | Platform operator |
| Project config YAML | Canonical v2 model overview | Project maintainer |
| Transforms | Typed transform reference and recipes | Project maintainer |
| DALiuGE graph patches | Deterministic graph mutation reference | Workflow author |
| WASM hooks | Extension boundary and upload flow | Extension author |
| Architecture map | Interactive system orientation | Everyone |
| Control plane | Ownership and persistence boundaries | Engineer / operator |
| Lifecycle | Discovery and execution data flow | Engineer / operator |
| State model | Exact internal and external state semantics | Engineer / on-call |
| Adapters | External contract boundaries | Integrator |
| Deployment profiles | Backend resource and connection schema | HPC operator |
| API workflow | Task-oriented HTTP sequence | Integrator |
| API schema | Generated endpoint and object contract | Integrator |
| CLI reference | Command families and conventions | Operator / automation author |
| Glossary | Stable vocabulary | Everyone |
| Overhaul design record | Historical decisions and migration sequence | Maintainer |

## Reading paths

1. **New operator:** Choose a path -> Five-minute local -> First workflow -> Operator handbook.
2. **HPC operator:** Installation -> Deployment profiles -> DALiuGE and Setonix -> Production runbook.
3. **Project maintainer:** Project config YAML -> Transforms -> Graph patches -> validation and dry run.
4. **Integrator:** Architecture map -> Adapters -> API workflow -> API schema.
5. **On-call:** Operator handbook -> Observability -> State model -> Recovery.

## Editorial contract

- Task articles answer a concrete operator question and link to exact reference material.
- Concept articles explain boundaries and invariants without duplicating command procedures.
- Reference articles enumerate stable fields, commands, and contracts.
- Generated API material remains generated; prose explains workflow rather than copying schemas.
- Every live-backend procedure distinguishes observation, dry run, and side effect.
- Diagrams use the same `input -> durable control -> leased effect -> external fact` visual grammar.
- Historical assessments remain available to maintainers but do not interrupt the operator journey.

When adding an article, assign it one reader question, one section, and one source-of-truth owner before adding it to navigation.
