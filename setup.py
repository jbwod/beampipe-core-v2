#!/usr/bin/env python3
"""
FastAPI Boilerplate Setup Script

Automates copying the correct configuration files for different deployment scenarios.
"""

import shutil
import subprocess
import secrets
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent
SCRIPTS_DIR = REPO_ROOT / "scripts"
ENV_TEMPLATE = SCRIPTS_DIR / "setup" / ".env.template"
LOGO_PATH = REPO_ROOT / "logo.txt"

CUSTOM_PRESETS: list[tuple[str, dict[str, int]]] = [
    ("minimal",         {"web": 1, "workers": 1, "restate": 1, "beamcore": 1}),
    ("production-like", {"web": 2, "workers": 3, "restate": 3, "beamcore": 2}),
]

DEPLOYMENTS: dict[str, dict] = {
    "local": {
        "label": "Local development",
        "description": "Single instance of every service, no nginx, no Restate cluster.",
        "source": SCRIPTS_DIR / "local_dev",
        "files": [
            ("Dockerfile", "Dockerfile"),
            ("docker-compose.yml", "docker-compose.yml"),
        ],
        "scalable": False,
    },
    "production": {
        "label": "Production (HA)",
        "description": "nginx LB, 2 web, 3 worker, 3 restate (cluster), 2 beamcore_rs. ",
        "source": SCRIPTS_DIR / "production",
        "files": [
            ("Dockerfile", "Dockerfile"),
            ("docker-compose.yml", "docker-compose.yml"),
            ("default.conf", "default.conf"),
            ("restate-config/restate.toml", "restate-config/restate.toml"),
        ],
        "scalable": False,
    },
    "custom": {
        "label": "Custom",
        "description": "Pick replica counts for web / worker / restate / beamcore_rs.",
        "source": SCRIPTS_DIR / "custom_base",
        "files": [
            ("Dockerfile", "Dockerfile"),
            ("docker-compose.yml", "docker-compose.yml"),
            ("default.conf", "default.conf"),
            ("restate-config/restate.toml", "restate-config/restate.toml"),
        ],
        "scalable": True,
    },
}


def info(msg: str) -> None: print(f"==> {msg}")
def ok(msg: str) -> None:   print(f"  ok  {msg}")
def warn(msg: str) -> None: print(f"warn  {msg}")
def err(msg: str) -> None:  print(f" err  {msg}", file=sys.stderr)

def _logo_lines() -> list[str] | None:
    if not LOGO_PATH.is_file():
        return None
    try:
        text = LOGO_PATH.read_text()
    except OSError:
        return None
    lines = text.rstrip("\n").splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    if not lines:
        return None
    lead = min(len(line) - len(line.lstrip()) for line in lines if line.strip())
    return [line[lead:].rstrip() for line in lines]


def print_logo() -> bool:
    lines = _logo_lines()
    if not lines:
        return False
    print()
    for line in lines:
        print(line)
    return True


def print_banner(ui: "BeamSetup | None" = None) -> None:
    if not print_logo():
        title = "beampipe-core setup"
        print(title)
        print("=" * len(title))
    print("  Ctrl+C cancels | Enter accepts [brackets] | ? shows help")
    print()
    print("You're a setup-wizard 'Harry' 🧙" )

def print_section(title: str, *, ui: "BeamSetup | None" = None) -> None:
    print()
    if ui is not None:
        ui._step += 1
        label = f"Step {ui._step}/{ui.total_steps}: {title}"
    else:
        label = title
    print(label)


@dataclass
class BeamSetup:
    total_steps: int = 8
    _step: int = field(default=0, repr=False)

    def note(self, msg: str) -> None:
        print(f"  {msg}")

    def hint(self, msg: str) -> None:
        print(f"  ! {msg}")

    def done(self, msg: str) -> None:
        ok(msg)

    def skip(self, msg: str) -> None:
        print(f"  skip | {msg}")

    def set_total_steps(self, n: int) -> None:
        self.total_steps = max(1, n)

    def menu(
        self,
        *,
        title: str,
        options: list[tuple[str, str, str]],
        help_text: str = "",
    ) -> str:
        print()
        if title:
            self.note(title)
        for i, (key, label, detail) in enumerate(options, 1):
            print(f"  {i}. {label}  ({key})")
            if detail:
                print(f"     {detail}")
            print()
        valid_keys = [o[0] for o in options]
        names = "|".join(valid_keys)
        while True:
            raw = input(f"  Choice [1-{len(options)}] or name (? for help): ").strip().lower()
            if raw == "?" and help_text:
                print()
                for line in help_text.splitlines():
                    print(f"    {line}")
                print()
                continue
            if raw in valid_keys:
                return raw
            if raw.isdigit() and 1 <= int(raw) <= len(options):
                return options[int(raw) - 1][0]
            print(f"    try again — 1-{len(options)}, {names}, or ?")


def detect_engine() -> str | None:
    for engine in ("docker", "podman"):
        if shutil.which(engine) is None:
            continue
        try:
            res = subprocess.run(
                [engine, "compose", "version"],
                capture_output=True, text=True, timeout=5,
            )
            if res.returncode == 0:
                return engine
        except (subprocess.SubprocessError, OSError):
            pass
    return None


@dataclass
class SetupAnswers:
    admin_name: str = "admin"
    admin_username: str = "admin"
    admin_email: str = ""
    admin_password: str = ""
    contact_name: str = ""
    contact_email: str = ""
    casda_username: str = ""
    casda_password: str = ""
    restate_snapshot_destination: str = ""
    restate_snapshot_aws_region: str = ""
    restate_aws_access_key_id: str = ""
    restate_aws_secret_access_key: str = ""
    slurm_ssh_status: str = "skipped"


def _status_text(label: str, configured: bool, *, applicable: bool = True,
                 detail: str = "") -> str:
    if not applicable:
        body = "not applicable"
    elif configured:
        body = "configured"
    else:
        body = "skipped"
    suffix = f" ({detail})" if detail else ""
    return f"{label}: {body}{suffix}"


@dataclass
class Counts:
    web: int = 1
    workers: int = 1
    restate: int = 1
    beamcore: int = 1

    def validate(self) -> list[str]:
        errors = []
        if self.web < 1: errors.append("web replicas must be >= 1")
        if self.workers < 1: errors.append("worker replicas must be >= 1")
        if self.restate < 1: errors.append("restate nodes must be >= 1")
        if self.beamcore < 1: errors.append("beamcore_rs replicas must be >= 1")
        if self.restate == 2:
            errors.append("restate=2 is invalid (default-replication=2 needs >=2 surviving "
                          "peers; pick 1 for single-node or 3+ for HA).")
        return errors


def _counts_summary(c: Counts) -> str:
    return (f"web * {c.web} worker * {c.workers} restate * {c.restate} "
            f"beamcore * {c.beamcore}")


def _estimate_scalable_services(c: Counts) -> int:
    base = 1  # scheduler
    return c.web + c.workers + c.restate + c.beamcore + base + 1


def prompt_counts(ui: BeamSetup) -> Counts:
    preset_opts: list[tuple[str, str, str]] = []
    for name, vals in CUSTOM_PRESETS:
        c = Counts(**vals)
        preset_opts.append((
            name,
            name.replace("-", " ").title(),
            f"{_counts_summary(c)} · ~{_estimate_scalable_services(c)} app containers",
        ))
    preset_opts.append(("custom", "Custom", "Enter each replica count yourself"))

    help_text = (
        "minimal: laptop / single-machine trials\n"
        "production-like: mirrors the production HA template\n"
        "custom: full control; restate must be 1 (single-node) or 3+ (cluster)\n"
        "restate=2 is never valid (replication factor needs surviving peers)"
    )
    pick = ui.menu(
        title="Pick a replica preset or choose custom:",
        options=preset_opts,
        help_text=help_text,
    )

    if pick != "custom":
        for name, vals in CUSTOM_PRESETS:
            if name == pick:
                c = Counts(**vals)
                ui.done(f"replicas: {_counts_summary(c)}")
                return c
        return Counts(**CUSTOM_PRESETS[0][1])

    def _ask(label: str, default: int) -> int:
        while True:
            raw = input(f"  {label} [{default}]: ").strip()
            if not raw:
                return default
            if raw.isdigit() and int(raw) >= 1:
                return int(raw)
            print("    invalid — must be a positive integer")

    initial = Counts()
    ui.hint("Enter replica counts (Enter keeps the value in [brackets]).")
    web = _ask("web replicas", initial.web)
    workers = _ask("worker replicas", initial.workers)
    while True:
        restate = _ask("restate nodes (1 = single-node, 3+ = HA cluster)", initial.restate)
        if restate != 2:
            break
        warn("restate=2 is invalid — pick 1 or 3+")
    beamcore = _ask("beamcore_rs replicas", initial.beamcore)
    c = Counts(web=web, workers=workers, restate=restate, beamcore=beamcore)
    ui.done(f"replicas: {_counts_summary(c)} · ~{_estimate_scalable_services(c)} app containers")
    return c


def _yaml():
    try:
        import yaml  # type: ignore
        return yaml
    except ImportError:
        err("custom mode requires PyYAML. Install with: pip install pyyaml")
        sys.exit(2)

def _names(base: str, n: int) -> list[str]:
    if n == 1:
        return [base]
    return [f"{base}-{i}" for i in range(1, n + 1)]


def _expand_compose(compose: dict, counts: Counts) -> dict:
    services = compose["services"]

    web_names = _names("web", counts.web)
    worker_names = _names("worker", counts.workers)
    restate_names = _names("restate", counts.restate)
    beamcore_names = _names("beamcore_rs", counts.beamcore)

    if counts.web > 1:
        seed = services.pop("web")
        for name in web_names:
            services[name] = _deepcopy(seed)

    if counts.workers > 1:
        seed = services.pop("worker")
        for name in worker_names:
            services[name] = _deepcopy(seed)

    if counts.beamcore > 1:
        seed = services.pop("beamcore_rs")
        for name in beamcore_names:
            services[name] = _deepcopy(seed)

    addresses = [f"http://{n}:5122" for n in restate_names]
    addresses_json = "[" + ",".join(f'"{a}"' for a in addresses) + "]"

    seed = services.pop("restate")
    for idx, name in enumerate(restate_names, start=1):
        block = _deepcopy(seed)
        env = block.setdefault("environment", {})
        env["RESTATE_NODE_NAME"] = name
        env["RESTATE_ADVERTISED_ADDRESS"] = f"http://{name}:5122"
        env["RESTATE_METADATA_CLIENT__ADDRESSES"] = addresses_json
        env["RESTATE_AUTO_PROVISION"] = "true" if idx == 1 else "false"
        if idx == 1:
            block["ports"] = ["9070:9070"]
            block.pop("depends_on", None)
        else:
            block.pop("ports", None)
            block["depends_on"] = {restate_names[0]: {"condition": "service_started"}}
        services[name] = block

    if "nginx" in services:
        services["nginx"]["depends_on"] = list(web_names)

    for wname in worker_names:
        block = services[wname]
        deps = block.get("depends_on", {})
        if isinstance(deps, list):
            deps = {k: {"condition": "service_started"} for k in deps}
        for old in ("restate", "beamcore_rs"):
            deps.pop(old, None)
        for rn in restate_names:
            deps[rn] = {"condition": "service_healthy"}
        for bn in beamcore_names:
            deps[bn] = {"condition": "service_healthy"}
        block["depends_on"] = deps

    if "restate_register" in services:
        rr = services["restate_register"]
        rr["environment"] = {"RESTATE_ADMIN_URL": f"http://{restate_names[0]}:9070"}
        deps: dict = {}
        for rn in restate_names:
            deps[rn] = {"condition": "service_healthy"}
        for bn in beamcore_names:
            deps[bn] = {"condition": "service_healthy"}
        rr["depends_on"] = deps
        register_cmds = "\n".join(
            f"restate deployments register http://{bn}:9080 --use-http1.1 --force --yes"
            for bn in beamcore_names
        )
        rr["command"] = ["sleep 2\n" + register_cmds + "\n"]

    return compose


def _deepcopy(obj):
    import copy
    return copy.deepcopy(obj)


def _render_default_conf(counts: Counts) -> str:
    web_names = _names("web", counts.web)
    restate_names = _names("restate", counts.restate)
    web_lines = "\n".join(f"    server {n}:8000;" for n in web_names)
    restate_lines = "\n".join(f"    server {n}:8080;" for n in restate_names)
    return f"""# Generated by `python setup.py` (custom template). Do not hand-edit; re-run setup.py.

upstream api_backend {{
{web_lines}
}}
upstream restate_ingress {{
{restate_lines}
}}

server {{
    listen 80;

    location / {{
        proxy_pass http://api_backend;
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }}
}}

server {{
    listen 8080;

    location / {{
        proxy_pass http://restate_ingress;
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }}
}}
"""


_SNAPSHOT_URL_SCHEMES = ("s3://", "gs://", "az://")


def _snapshot_configured(answers: SetupAnswers) -> bool:
    return bool(
        answers.restate_snapshot_destination.strip()
        and answers.restate_snapshot_aws_region.strip()
        and answers.restate_aws_access_key_id.strip()
        and answers.restate_aws_secret_access_key.strip()
    )


def _patch_restate_toml(
    toml_text: str,
    *,
    restate_count: int,
    answers: SetupAnswers | None = None,
) -> str:
    replication = 1 if restate_count == 1 else 2
    text = re.sub(
        r"^default-replication\s*=.*$",
        f"default-replication = {replication}",
        toml_text,
        count=1,
        flags=re.MULTILINE,
    )
    if answers is None or not _snapshot_configured(answers):
        return text
    dest = answers.restate_snapshot_destination.strip().replace('"', '\\"')
    region = answers.restate_snapshot_aws_region.strip().replace('"', '\\"')
    text = re.sub(
        r'^destination\s*=.*$',
        f'destination = "{dest}"',
        text,
        count=1,
        flags=re.MULTILINE,
    )
    if dest.lower().startswith("s3://"):
        text = re.sub(
            r'^aws-region\s*=.*$',
            f'aws-region = "{region}"',
            text,
            count=1,
            flags=re.MULTILINE,
        )
    return text



def _gen_secret_key() -> str:
    return secrets.token_hex(32)

def _gen_password() -> str:
    return secrets.token_urlsafe(24)


def _env_quote(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def render_env(template_text: str, *, environment: str, answers: SetupAnswers) -> str:
    prompt_map = {
        "ADMIN_NAME":                    answers.admin_name,
        "ADMIN_USERNAME":                answers.admin_username,
        "ADMIN_EMAIL":                   answers.admin_email,
        "ADMIN_PASSWORD":                answers.admin_password,
        "CONTACT_NAME":                  answers.contact_name,
        "CONTACT_EMAIL":                 answers.contact_email,
        "CASDA_USERNAME":                answers.casda_username,
        "CASDA_PASSWORD":                answers.casda_password,
        "RESTATE_AWS_ACCESS_KEY_ID":     answers.restate_aws_access_key_id,
        "RESTATE_AWS_SECRET_ACCESS_KEY": answers.restate_aws_secret_access_key,
    }

    out_lines: list[str] = []
    for line in template_text.splitlines():
        if line.startswith("SECRET_KEY=__GENERATE__"):
            out_lines.append(f"SECRET_KEY={_gen_secret_key()}")
            continue
        if line.startswith("POSTGRES_PASSWORD=__GENERATE__"):
            out_lines.append(f'POSTGRES_PASSWORD="{_env_quote(_gen_password())}"')
            continue
        if line.startswith("ENVIRONMENT=__ENVIRONMENT__"):
            out_lines.append(f'ENVIRONMENT="{environment}"')
            continue

        replaced = False
        for key, value in prompt_map.items():
            prompt_prefix = f"{key}=__PROMPT__"
            optional_prefix = f"{key}=__OPTIONAL__"
            if line.startswith(prompt_prefix):
                out_lines.append(f'{key}="{_env_quote(value)}"')
                replaced = True
                break
            if line.startswith(optional_prefix):
                out_lines.append(f'{key}="{_env_quote(value)}"' if value else f"{key}=")
                replaced = True
                break
        if replaced:
            continue

        out_lines.append(line)
    return "\n".join(out_lines) + "\n"

def _planned_paths(template_key: str, counts: Counts | None) -> list[Path]:
    cfg = DEPLOYMENTS[template_key]
    paths = [REPO_ROOT / dst for _, dst in cfg["files"]]
    paths.append(REPO_ROOT / ".env")
    return paths


def _confirm_overwrite(existing: list[Path], *, ui: BeamSetup | None = None) -> bool:
    if not existing:
        if ui is not None:
            ui.done("no existing files to overwrite")
        return True
    print()
    warn(f"{len(existing)} file(s) already exist and will be overwritten:")
    for p in existing:
        print(f"    {p.relative_to(REPO_ROOT)}")
    print()
    return _prompt_yes_no("Overwrite these files?", default=False)


def _copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    ok(f"wrote {dst.relative_to(REPO_ROOT)}")


def _write_text(dst: Path, text: str, *, mode: int | None = None) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(text)
    if mode is not None:
        dst.chmod(mode)
    ok(f"wrote {dst.relative_to(REPO_ROOT)}")


_TEMPLATE_HINTS = {
    "local":      "~7 services",
    "production": "HA stack (nginx LB + Restate cluster) then Slurm SSH",
    "custom":     "Choose your own adventure!",
}


def _interactive_template_pick(ui: BeamSetup, *, engine: str | None) -> str:
    options: list[tuple[str, str, str]] = []
    for key in DEPLOYMENTS:
        cfg = DEPLOYMENTS[key]
        hint = _TEMPLATE_HINTS.get(key, "")
        detail = f"{cfg['description']} — {hint}" if hint else cfg["description"]
        options.append((key, cfg["label"], detail))

    help_lines = [
        "local: fastest for development",
        "production: full HA stack for production",
        "custom: same layout as production but you choose replica counts",
    ]
    if engine:
        help_lines.insert(0, f"Detected {engine} — any template will work.")
    else:
        help_lines.insert(0, "No compose detected — you can still write files but they won't do much...")

    key = ui.menu(
        title="Which deployment shape do you want?",
        options=options,
        help_text="\n".join(help_lines),
    )
    ui.done(f"template: {DEPLOYMENTS[key]['label']} ({key})")
    return key

_EMAIL_RE = re.compile(r"^[^@\s]+@[^@\s]+\.[^@\s]+$")


def _is_email(value: str) -> bool:
    return bool(_EMAIL_RE.match(value.strip()))


def _prompt_text(label: str, *, default: str = "") -> str:
    suffix = f" [{default}]" if default else ""
    while True:
        raw = input(f"  {label}{suffix}: ").strip()
        if raw:
            return raw
        if default:
            return default
        print("    value required; try again")


def _prompt_email(label: str, *, default: str = "") -> str:
    suffix = f" [{default}]" if default else ""
    for _ in range(3):
        raw = input(f"  {label}{suffix}: ").strip() or default
        if _is_email(raw):
            return raw
        print("    that doesn't look like an email (need user@host.tld); try again")
    if default:
        warn(f"using fallback {label}: {default}")
        return default
    return ""


def _prompt_yes_no(question: str, *, default: bool = False) -> bool:
    hint = "[Y/n]" if default else "[y/N]"
    while True:
        raw = input(f"  {question} {hint}: ").strip().lower()
        if not raw:
            return default
        if raw in ("y", "yes"):
            return True
        if raw in ("n", "no"):
            return False
        print("    please answer yes or no")


def _prompt_secret(label: str, *, allow_empty: bool = False) -> str:
    while True:
        p1 = getpass.getpass(f"  {label} (input hidden): ")
        if not p1:
            if allow_empty:
                return ""
            print("    value required; try again")
            continue
        p2 = getpass.getpass(f"  {label} (again):       ")
        if p1 == p2:
            return p1
        print("    values did not match; try again")

def gather_admin_identity(answers: SetupAnswers, *, ui: BeamSetup | None = None) -> None:
    if ui is not None:
        ui.hint("This account is used for the first superuser seed (make beampipe-new-admin).")

    answers.admin_name = _prompt_text("admin display name", default="admin")
    answers.admin_username = _prompt_text("admin username", default="admin")

    email = _prompt_email("admin email", default="admin@example.com")
    if not email:
        email = "admin@example.com"
        warn(f"using fallback admin email: {email}")
    answers.admin_email = email

    password = _prompt_secret("admin password", allow_empty=True)
    if not password:
        password = _gen_password()
        warn(f"no admin password supplied; generated random one: {password}")
    elif len(password) < 8:
        warn("admin password is shorter than 8 chars — fine for dev, weak for production")
    answers.admin_password = password

    if ui is not None:
        ui.done(f"admin: {answers.admin_username} <{answers.admin_email}>")


def gather_app_contact(answers: SetupAnswers, *, ui: BeamSetup | None = None) -> None:
    if ui is not None:
        ui.hint("Shown in OpenAPI /docs as the API owner contact.")
    if _prompt_yes_no("Use the same name and email as the admin account?", default=True):
        answers.contact_name = answers.admin_name
        answers.contact_email = answers.admin_email
        if ui is not None:
            ui.done(f"contact: {answers.contact_name} <{answers.contact_email}>")
        return

    default_name = answers.admin_name or "Ops"
    default_email = answers.admin_email or "ops@example.com"
    answers.contact_name = _prompt_text("contact name", default=default_name)
    answers.contact_email = _prompt_email("contact email", default=default_email) or default_email
    if ui is not None:
        ui.done(f"contact: {answers.contact_name} <{answers.contact_email}>")


def gather_casda_credentials(answers: SetupAnswers, *, ui: BeamSetup | None = None) -> None:
    if ui is not None:
        ui.hint("Only needed for wallaby_hires / CASDA archive discovery.")

    if not _prompt_yes_no("Set up CASDA / OPAL credentials now?", default=False):
        if ui is not None:
            ui.skip("CASDA / OPAL — skipped (set CASDA_USERNAME / CASDA_PASSWORD in .env later)")
        return

    answers.casda_username = _prompt_text("CASDA / OPAL username")
    answers.casda_password = _prompt_secret("CASDA / OPAL password", allow_empty=True)
    if ui is not None:
        ui.done(f"CASDA / OPAL — configured ({answers.casda_username})")


def _restate_aws_applicable(template_key: str, counts: Counts | None) -> bool:
    if template_key == "production":
        return True
    if template_key == "custom" and counts is not None and counts.restate >= 2:
        return True
    return False


def gather_restate_aws_credentials(answers: SetupAnswers, *,
                                   template_key: str, counts: Counts | None,
                                   ui: BeamSetup | None = None) -> None:
    if not _restate_aws_applicable(template_key, counts):
        if ui is not None:
            ui.skip("Restate S3 snapshots — not applicable (single-node / local template)")
        return

    if ui is not None:
        ui.hint(
            "Multi-node Restate requires a shared snapshot store (S3, GCS, or Azure). "
            "See https://docs.restate.dev/server/snapshots"
        )

    while True:
        dest = _prompt_text(
            "snapshot destination URL (e.g. s3://my-bucket/beampipe/cluster)",
        ).strip()
        if any(dest.lower().startswith(s) for s in _SNAPSHOT_URL_SCHEMES):
            answers.restate_snapshot_destination = dest
            break
        print("    use s3://, gs://, or az:// — see https://docs.restate.dev/server/snapshots")

    answers.restate_snapshot_aws_region = _prompt_text(
        "AWS region for S3 snapshots (e.g. ap-southeast-2)",
    ).strip()
    answers.restate_aws_access_key_id = _prompt_text("Restate AWS access key id").strip()
    answers.restate_aws_secret_access_key = _prompt_secret(
        "Restate AWS secret access key",
    )
    if ui is not None:
        ui.done(f"Restate S3 snapshots — {answers.restate_snapshot_destination}")


def gather_slurm_ssh(answers: SetupAnswers, *, template_key: str,
                     ui: BeamSetup | None = None) -> None:
    key_path = REPO_ROOT / "deploy" / "ssh" / "id_slurm"
    pub_path = key_path.with_suffix(".pub")
    known_hosts_path = REPO_ROOT / "deploy" / "ssh" / "known_hosts"

    if template_key not in ("production", "custom"):
        answers.slurm_ssh_status = "not_applicable"
        if ui is not None:
            ui.skip("Slurm SSH — not needed for local template")
        return

    if key_path.exists():
        answers.slurm_ssh_status = "existing"
        if ui is not None:
            ui.done(f"Slurm SSH — using existing key at {key_path.relative_to(REPO_ROOT)}")
        return

    if ui is not None:
        ui.hint(f"Creates {key_path.relative_to(REPO_ROOT)} (ed25519, no passphrase).")
    if not _prompt_yes_no("Generate a Slurm SSH key now?", default=True):
        answers.slurm_ssh_status = "skipped"
        if ui is not None:
            ui.skip("Slurm SSH — skipped (run ssh-keygen later)")
        return

    if shutil.which("ssh-keygen") is None:
        warn("ssh-keygen not found on PATH; skipping Slurm bot key generation.")
        answers.slurm_ssh_status = "skipped"
        return

    key_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run(
            ["ssh-keygen", "-t", "ed25519", "-f", str(key_path), "-N", "", "-q",
             "-C", "beampipe-slurm-bot"],
            check=True,
        )
    except (subprocess.SubprocessError, OSError) as exc:
        err(f"ssh-keygen failed: {exc}")
        answers.slurm_ssh_status = "skipped"
        return

    try:
        key_path.chmod(0o600)
    except OSError:
        pass

    if not known_hosts_path.exists():
        known_hosts_path.parent.mkdir(parents=True, exist_ok=True)
        known_hosts_path.write_text("# Populate with: make slurm-known-hosts-sync\n")

    answers.slurm_ssh_status = "created"
    if ui is not None:
        ui.done(f"Slurm SSH — created {key_path.relative_to(REPO_ROOT)} (chmod 600)")
        if pub_path.exists():
            ui.hint(f"Add {pub_path.relative_to(REPO_ROOT)} to ~/.ssh/authorized_keys on the head node")


def _slurm_ssh_detail(status: str) -> tuple[bool, bool, str]:
    if status == "not_applicable":
        return (False, False, "")
    if status == "created":
        return (True, True, "key generated")
    if status == "existing":
        return (True, True, "existing key kept")
    return (False, True, "")

def _engine_text(engine: str | None) -> str:
    return engine if engine else "not found (install docker or podman)"


def _summary_lines(template_key: str, engine: str | None, counts: Counts | None,
                   answers: SetupAnswers, planned_paths: list[Path]) -> list[str]:
    cfg = DEPLOYMENTS[template_key]
    lines = [
        f"template : {cfg['label']} ({template_key})",
        f"engine   : {_engine_text(engine)}",
    ]
    if counts is not None:
        lines.append(f"replicas : web={counts.web} workers={counts.workers} "
                     f"restate={counts.restate} beamcore_rs={counts.beamcore}")
    lines.append(f"admin    : {answers.admin_username} <{answers.admin_email}>")
    lines.append(f"contact  : {answers.contact_name} <{answers.contact_email}>")
    lines.append("  " + _status_text(
        "casda    ", bool(answers.casda_username),
        detail=(answers.casda_username if answers.casda_username else ""),
    ))
    lines.append("  " + _status_text(
        "restate s3", _snapshot_configured(answers),
        applicable=_restate_aws_applicable(template_key, counts),
        detail=answers.restate_snapshot_destination if _snapshot_configured(answers) else "",
    ))
    configured, applicable, detail = _slurm_ssh_detail(answers.slurm_ssh_status)
    lines.append("  " + _status_text(
        "slurm ssh ", configured, applicable=applicable, detail=detail,
    ))
    lines.append("files    :")
    for p in planned_paths:
        lines.append(f"  - {p.relative_to(REPO_ROOT)}")
    return lines


def _existing_postgres_password() -> str | None:
    env_path = REPO_ROOT / ".env"
    if not env_path.exists():
        return None
    try:
        for raw in env_path.read_text().splitlines():
            line = raw.strip()
            if line.startswith("POSTGRES_PASSWORD="):
                return line.split("=", 1)[1].strip()
    except OSError:
        return None
    return None


def print_setup_summary(template_key: str, engine: str | None, counts: Counts | None,
                        answers: SetupAnswers, planned_paths: list[Path]) -> None:
    for line in _summary_lines(template_key, engine, counts, answers, planned_paths):
        print(f"  {line}")
    print()
    print("  .env will be regenerated with a fresh SECRET_KEY and POSTGRES_PASSWORD.")
    print("  Secrets above are stored in .env (gitignored); none are printed here.")
    if _existing_postgres_password():
        print()
        warn("An existing .env was found. The new POSTGRES_PASSWORD will not match")
        warn("any existing 'postgres-data' volume; Postgres will fail to start.")
        warn("If this is a fresh setup, remove the old volume first:")
        warn("    docker compose down -v   # or: podman compose down -v")


def print_next_steps(template_key: str, engine: str | None, counts: Counts | None,
                     answers: SetupAnswers) -> None:
    print_section("Setup complete")
    cfg = DEPLOYMENTS[template_key]
    print("Configured")
    print(f"template : {cfg['label']} ({template_key})")
    print(f"engine   : {engine or 'not found'}")
    print(f"admin    : {answers.admin_username} <{answers.admin_email}>")
    print(f"contact  : {answers.contact_name} <{answers.contact_email}>")
    if counts is not None:
        print(f"  replicas : web={counts.web} workers={counts.workers} "
              f"restate={counts.restate} beamcore_rs={counts.beamcore}")
    aws_applicable = _restate_aws_applicable(template_key, counts)
    print(f"  {_status_text('casda    ', bool(answers.casda_username))}")
    print(f"  {_status_text('restate s3', _snapshot_configured(answers), applicable=aws_applicable, detail=answers.restate_snapshot_destination if _snapshot_configured(answers) else '')}")
    configured, applicable, detail = _slurm_ssh_detail(answers.slurm_ssh_status)
    print(f"  {_status_text('slurm ssh ', configured, applicable=applicable, detail=detail)}")
    print()
    print("Next steps:")
    if template_key == "local":
        print("  1. Start the stack (compose up + migrate + seed admin + URLs):")
        print("       make dev")
        print()
        print("  Useful URLs once it's up:")
        print("       API docs:     http://127.0.0.1:8000/docs")
        print("       Sources UI:   http://127.0.0.1:8000/sources")
        print("       Readiness:    http://127.0.0.1:8000/api/v1/ready")
        print("       Restate:      http://127.0.0.1:9070")
        print()
        print(f"Log in at /docs (POST /api/v1/login) with {answers.admin_email}")
        print("  and the password you set above to generate a Key.")
        print()
        print("  Other targets:  make logs   make ps   make beampipe-start make beampipe-stop")
    else:
        steps: list[str] = []
        if answers.slurm_ssh_status in ("created", "existing"):
            steps.append(
                "Copy the public key to the head node:\n"
                "       cat ./deploy/ssh/id_slurm.pub  # add to ~/.ssh/authorized_keys on the head node"
            )
        else:
            steps.append(
                "Generate the dedicated Slurm SSH key (one-time):\n"
                "       ssh-keygen -t ed25519 -f ./deploy/ssh/id_slurm -N \"\"\n"
                "       cat ./deploy/ssh/id_slurm.pub  # add to ~/.ssh/authorized_keys on the head node"
            )
        steps.append("Populate ./deploy/ssh/known_hosts (Slurm SSH):\n       make slurm-known-hosts-sync")
        missing: list[str] = []
        if not answers.casda_username:
            missing.append("CASDA_USERNAME / CASDA_PASSWORD (wallaby_hires module)")
        if aws_applicable and not _snapshot_configured(answers):
            missing.append(
                "restate-config/restate.toml snapshot destination + RESTATE_AWS_* in .env"
            )
        if missing:
            steps.append("Fill these in .env when ready:\n       - " + "\n       - ".join(missing))
        if template_key != "production":
            steps.append("Review the generated docker-compose.yml and default.conf.")
        steps.append("Bring everything up:\n       make beampipe-start")
        steps.append("Seed the admin user once:\n       make beampipe-new-admin")
        steps.append("Open http://localhost/docs  (Restate admin: http://localhost:9070)")
        for i, s in enumerate(steps, 1):
            print(f"  {i}. {s}")
    print()
    warn("Your admin password and DB password live in .env.")


def _apply_custom(cfg: dict, counts: Counts, answers: SetupAnswers) -> None:
    yaml = _yaml()
    src = cfg["source"]
    compose = yaml.safe_load((src / "docker-compose.yml").read_text())
    expanded = _expand_compose(compose, counts)
    expanded_text = yaml.safe_dump(expanded, sort_keys=False, width=120)
    _write_text(REPO_ROOT / "docker-compose.yml", expanded_text)
    _copy_file(src / "Dockerfile", REPO_ROOT / "Dockerfile")
    _write_text(REPO_ROOT / "default.conf", _render_default_conf(counts))
    toml_text = (src / "restate-config" / "restate.toml").read_text()
    _write_text(
        REPO_ROOT / "restate-config" / "restate.toml",
        _patch_restate_toml(toml_text, restate_count=counts.restate, answers=answers),
    )

def run() -> int:
    ui = BeamSetup(total_steps=8)

    print_banner(ui=ui)

    print_section("Environment", ui=ui)
    info("Checking for Docker / Podman Compose…")
    engine = detect_engine()
    if engine:
        ok(f"container engine: {engine} (compose available)")
    else:
        warn("no docker / podman compose engine found on PATH; files will still be written.")

    print_section("Template", ui=ui)
    template_key = _interactive_template_pick(ui, engine=engine)

    cfg = DEPLOYMENTS[template_key]
    if cfg["scalable"]:
        ui.set_total_steps(9)

    if not cfg["source"].is_dir():
        err(f"template directory missing: {cfg['source'].relative_to(REPO_ROOT)}")
        return 1
    if not ENV_TEMPLATE.is_file():
        err(f"env template missing: {ENV_TEMPLATE.relative_to(REPO_ROOT)}")
        return 1

    counts: Counts | None = None
    if cfg["scalable"]:
        print_section("Replica counts", ui=ui)
        counts = prompt_counts(ui)
        errors = counts.validate()
        if errors:
            for e in errors:
                err(e)
            return 2

    answers = SetupAnswers()

    print_section("Admin account", ui=ui)
    gather_admin_identity(answers, ui=ui)

    print_section("Application contact", ui=ui)
    gather_app_contact(answers, ui=ui)

    print_section("CASDA / OPAL credentials", ui=ui)
    gather_casda_credentials(answers, ui=ui)

    print_section("Restate S3 snapshots", ui=ui)
    gather_restate_aws_credentials(answers, template_key=template_key, counts=counts, ui=ui)
    if _restate_aws_applicable(template_key, counts) and not _snapshot_configured(answers):
        err("Restate S3 snapshot destination, region, and credentials are required for this template.")
        return 2

    print_section("Slurm SSH bot key", ui=ui)
    gather_slurm_ssh(answers, template_key=template_key, ui=ui)

    targets = _planned_paths(template_key, counts)
    print_section("Review", ui=ui)
    print_setup_summary(template_key, engine, counts, answers, targets)

    existing = [p for p in targets if p.exists()]
    if not _confirm_overwrite(existing, ui=ui):
        return 1

    print_section("Write files", ui=ui)
    ui.hint(f"Writing {len(cfg['files']) + 1} file(s) to the repo root…")

    if cfg["scalable"]:
        _apply_custom(cfg, counts, answers)
    else:
        for src_rel, dst_rel in cfg["files"]:
            src_path = cfg["source"] / src_rel
            dst_path = REPO_ROOT / dst_rel
            if dst_rel == "restate-config/restate.toml":
                toml_text = src_path.read_text()
                _write_text(
                    dst_path,
                    _patch_restate_toml(
                        toml_text,
                        restate_count=3,
                        answers=answers,
                    ),
                )
            else:
                _copy_file(src_path, dst_path)

    rendered = render_env(
        ENV_TEMPLATE.read_text(),
        environment=("local" if template_key == "local" else
                     ("production" if template_key == "production" else "custom")),
        answers=answers,
    )
    _write_text(REPO_ROOT / ".env", rendered, mode=0o600)
    ui.done("all files written")

    print_next_steps(template_key, engine, counts, answers)

    if template_key == "local" and engine:
        print()
        if _prompt_yes_no("Start the stack now with 'make dev'?", default=True):
            print()
            try:
                rc = subprocess.run(["make", "dev"], cwd=REPO_ROOT, check=False).returncode
            except FileNotFoundError:
                err("'make' was not found on PATH; run `make dev` manually.")
                return 0
            if rc != 0:
                warn(f"'make dev' exited with status {rc}; check the output above.")
            return rc
    return 0


def main() -> int:
    try:
        return run()
    except KeyboardInterrupt:
        print()
        print("Setup cancelled.")
        return 130


if __name__ == "__main__":
    sys.exit(main())
