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
assert.doesNotMatch(homepage, /id="bp-install-admin-password"/);
assert.doesNotMatch(homepage, /type="password"/);

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
  builder.validationError(
    state({ projectMode: "custom", projectConfig: "/tmp/project.yaml" })
  ),
  ""
);
assert.equal(
  builder.validationError(state({ apiPort: "65536" })),
  "API port must be a whole number from 1 to 65535."
);

console.log("install command builder ok");
