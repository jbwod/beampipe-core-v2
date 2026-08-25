"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const builder = require("../boilerplate_docs/javascripts/install-builder.js");

function state(overrides) {
  return Object.assign(
    {
      unattended: false,
      runtime: "docker",
      start: true,
      dashboard: false,
      directory: "",
      projectMode: "none",
      projectConfig: "",
      adminUser: "",
      adminEmail: "",
      adminPasswordFile: "",
      apiPort: "",
      postgresPort: "",
      metricsPort: "",
    },
    overrides
  );
}

assert.equal(
  builder.buildCommand(state()),
  "curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --runtime docker"
);

const homepage = fs.readFileSync("boilerplate_docs/index.md", "utf8");
assert.match(homepage, /id="bp-install-admin-password-file"/);
assert.match(homepage, /id="bp-install-password-note"/);
assert.match(homepage, /id="bp-install-start-note"/);
assert.match(homepage, /id="bp-install-status-command"/);
assert.match(homepage, /id="bp-install-doctor-command"/);
assert.doesNotMatch(homepage, /id="bp-install-admin-password"/);
assert.doesNotMatch(homepage, /type="password"/);
assert.doesNotMatch(homepage, /placeholder="generated if empty"/);

const unattended = builder.buildCommand(
  state({
    unattended: true,
    runtime: "host",
    start: false,
    directory: "/srv/beampipe control",
    projectMode: "custom",
    projectConfig: "/srv/policy/my project's config.yaml",
    adminPasswordFile: "/run/secrets/beampipe admin",
    apiPort: "18081",
  })
);
assert.match(unattended, /--yes --runtime host --no-start/);
assert.match(unattended, /--directory '\/srv\/beampipe control'/);
assert.match(unattended, /--project-config '\/srv\/policy\/my project'\\''s config\.yaml'/);
assert.match(unattended, /--admin-password-file '\/run\/secrets\/beampipe admin'/);
assert.match(unattended, /--api-port 18081/);
assert.doesNotMatch(unattended, /--admin-password(?:\s|$)/);

const specialHome = "/srv/beam pipe's $HOME $(touch nope) `touch nope2` \\data";
assert.equal(
  builder.operatorCommand(state({ directory: specialHome }), "status"),
  "beampipe --home '/srv/beam pipe'\\''s $HOME $(touch nope) `touch nope2` \\data' status"
);
assert.equal(
  builder.operatorCommand(state({ directory: specialHome }), "doctor"),
  "beampipe --home '/srv/beam pipe'\\''s $HOME $(touch nope) `touch nope2` \\data' doctor"
);
assert.equal(
  builder.operatorCommand(state({ directory: "~/custom home" }), "status"),
  'beampipe --home "$HOME"/' + "'custom home' status"
);
assert.match(
  builder.buildCommand(state({ directory: "~/custom home" })),
  /--directory "\$HOME"\/'custom home'/
);

assert.match(builder.passwordNote(state()), /guided wizard prompts securely/);
assert.match(
  builder.passwordNote(state({ unattended: true })),
  /credentials\/admin\/password with mode 0600/
);
assert.match(
  builder.passwordNote(state({ adminPasswordFile: "/run/secrets/admin" })),
  /reads that file without copying its secret value/
);
assert.match(builder.startNote(state()), /Docker starts Core automatically/);
assert.match(
  builder.startNote(state({ runtime: "host", directory: "/srv/beampipe control" })),
  /beampipe --home '\/srv\/beampipe control' start.*foreground Core process/
);
assert.match(
  builder.startNote(state({ runtime: "host", start: false })),
  /Services stay stopped.*host process stays in the foreground/
);

const wallaby = builder.buildCommand(
  state({ projectMode: "wallaby-hires", dashboard: true })
);
assert.match(wallaby, /--dashboard/);
assert.match(wallaby, /--sample wallaby-hires/);
assert.doesNotMatch(wallaby, /--project-config/);

assert.equal(
  builder.validationError(state({ projectMode: "custom" })),
  "Enter the path to your project YAML before copying the command."
);
assert.equal(
  builder.validationError(state({ directory: "relative/home" })),
  "Install directory must be an absolute path or start with ~/."
);
assert.equal(builder.validationError(state({ directory: "~/custom home" })), "");
assert.equal(
  builder.validationError(
    state({ projectMode: "custom", projectConfig: "/tmp/project.yaml" })
  ),
  ""
);
assert.equal(
  builder.validationError(state({ apiPort: "65536" })),
  "API port must be a whole number from 1 to 65535."
);

const quickStart = fs.readFileSync(
  "boilerplate_docs/getting-started/index.md",
  "utf8"
);
const installation = fs.readFileSync(
  "boilerplate_docs/getting-started/installation.md",
  "utf8"
);
assert.match(quickStart, /beampipe --home .* status/);
assert.match(quickStart, /wizard prompts securely/);
assert.match(installation, /Docker setup starts Core automatically/);
assert.match(installation, /host.*foreground/i);

console.log("install command builder ok");
