(function () {
  "use strict";

  var INSTALL_URL =
    "https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh";
  var CURL = "curl -fsSL " + INSTALL_URL + " | sh";
  var DEFAULT_API_PORT = "18080";
  var DEFAULT_POSTGRES_PORT = "5432";
  var DEFAULT_METRICS_PORT = "9090";
  var DEFAULT_HOME = "~/beampipe";

  function shellQuote(value) {
    if (/^[A-Za-z0-9_./~:@+-]+$/.test(value)) {
      return value;
    }
    return "'" + String(value).replace(/'/g, "'\\''") + "'";
  }

  function buildCommand(state) {
    var flags = [];
    if (state.unattended) {
      flags.push("--yes");
    }
    flags.push("--runtime", state.runtime);
    if (!state.start) {
      flags.push("--no-start");
    }
    if (state.dashboard && state.runtime === "docker") {
      flags.push("--dashboard");
    }
    if (state.directory) {
      flags.push("--directory", shellQuote(state.directory));
    }
    if (state.projectMode === "wallaby-hires") {
      flags.push("--sample", "wallaby-hires");
    } else if (state.projectMode === "custom" && state.projectConfig) {
      flags.push("--project-config", shellQuote(state.projectConfig));
    }
    if (state.adminUser) {
      flags.push("--admin-user", shellQuote(state.adminUser));
    }
    if (state.adminEmail) {
      flags.push("--admin-email", shellQuote(state.adminEmail));
    }
    if (state.adminPasswordFile) {
      flags.push("--admin-password-file", shellQuote(state.adminPasswordFile));
    }
    addPortFlag(flags, "--api-port", state.apiPort);
    addPortFlag(flags, "--postgres-port", state.postgresPort);
    addPortFlag(flags, "--metrics-port", state.metricsPort);
    if (!flags.length) {
      return CURL;
    }
    return CURL + " -s -- " + flags.join(" ");
  }

  function addPortFlag(flags, name, value) {
    if (!value) {
      return;
    }
    flags.push(name, value);
  }

  function selectedValue(root, name, fallback) {
    var selected = root.querySelector('input[name="' + name + '"]:checked');
    return selected ? selected.value : fallback;
  }

  function selectedRuntime(root) {
    var radio = root.querySelector('input[name="runtime"]:checked');
    var select = root.querySelector("#bp-install-runtime");
    var value = radio ? radio.value : select ? select.value : "docker";
    return value === "host" ? "host" : "docker";
  }

  function readState(root) {
    var directory = root.querySelector("#bp-install-directory");
    var start = root.querySelector("#bp-install-start");
    var dashboard = root.querySelector("#bp-install-dashboard");
    var adminUser = root.querySelector("#bp-install-admin-user");
    var adminEmail = root.querySelector("#bp-install-admin-email");
    var adminPasswordFile = root.querySelector("#bp-install-admin-password-file");
    var projectConfig = root.querySelector("#bp-install-project-config");
    return {
      runtime: selectedRuntime(root),
      directory: directory ? directory.value.trim() : "",
      unattended: selectedValue(root, "setup-mode", "guided") === "unattended",
      start: !start || start.checked,
      dashboard: Boolean(dashboard && dashboard.checked),
      projectMode: selectedValue(root, "project-mode", "none"),
      projectConfig: projectConfig ? projectConfig.value.trim() : "",
      adminUser: adminUser ? adminUser.value.trim() : "",
      adminEmail: adminEmail ? adminEmail.value.trim() : "",
      adminPasswordFile: adminPasswordFile ? adminPasswordFile.value.trim() : "",
      apiPort: fieldValue(root, "#bp-install-api-port"),
      postgresPort: fieldValue(root, "#bp-install-postgres-port"),
      metricsPort: fieldValue(root, "#bp-install-metrics-port"),
    };
  }

  function syncProject(root, state) {
    var input = root.querySelector("#bp-install-project-config");
    var field = root.querySelector("#bp-install-project-config-field");
    var custom = state.projectMode === "custom";
    if (input) {
      input.disabled = !custom;
    }
    if (field) {
      field.classList.toggle("is-disabled", !custom);
    }
  }

  function validationError(state) {
    if (state.projectMode === "custom" && !state.projectConfig) {
      return "Enter the path to your project YAML before copying the command.";
    }
    var ports = [
      ["API", state.apiPort],
      ["PostgreSQL", state.postgresPort],
      ["Metrics", state.metricsPort],
    ];
    for (var index = 0; index < ports.length; index += 1) {
      var label = ports[index][0];
      var value = ports[index][1];
      if (
        value &&
        (!/^\d+$/.test(value) || Number(value) < 1 || Number(value) > 65535)
      ) {
        return label + " port must be a whole number from 1 to 65535.";
      }
    }
    return "";
  }

  function projectLabel(state) {
    if (state.projectMode === "wallaby-hires") {
      return "WALLABY sample";
    }
    if (state.projectMode === "custom") {
      return "custom project";
    }
    return "neutral Core";
  }

  function fieldValue(root, selector) {
    var input = root.querySelector(selector);
    return input ? input.value.trim() : "";
  }

  function syncDashboard(root, state) {
    var dashboard = root.querySelector("#bp-install-dashboard");
    var label = root.querySelector("#bp-install-dashboard-label");
    var host = state.runtime === "host";
    if (!dashboard) {
      return;
    }
    dashboard.disabled = host;
    if (host) {
      dashboard.checked = false;
    }
    if (label) {
      label.classList.toggle("is-disabled", host);
    }
  }

  function render(root) {
    var state = readState(root);
    syncDashboard(root, state);
    syncProject(root, state);
    state = readState(root);
    var command = buildCommand(state);
    var error = validationError(state);
    var output = root.querySelector("#bp-install-command");
    if (output) {
      output.textContent = command;
    }
    var apiUrl = root.querySelector("#bp-install-api-url");
    if (apiUrl) {
      apiUrl.textContent =
        "http://127.0.0.1:" + (state.apiPort || DEFAULT_API_PORT) + "/api/v2";
    }
    var home = root.querySelector("#bp-install-home");
    if (home) {
      home.textContent = state.directory || DEFAULT_HOME;
    }
    var summary = root.querySelector("#bp-install-summary");
    if (summary) {
      summary.textContent =
        (state.unattended ? "Unattended" : "Guided") +
        " " +
        (state.runtime === "docker" ? "Docker" : "host") +
        " setup · " +
        projectLabel(state) +
        " · external execution mocked";
    }
    var modeNote = root.querySelector("#bp-install-mode-note");
    if (modeNote) {
      modeNote.textContent = state.unattended
        ? "Uses explicit defaults and does not prompt; suitable for repeatable automation."
        : "Recommended for a first install. Prompts are read from your terminal even though the script arrives through a pipe.";
    }
    var copy = root.querySelector("#bp-install-copy");
    if (copy) {
      copy.disabled = Boolean(error);
    }
    root.setAttribute("data-bp-install-error", error);
    return { command: command, error: error };
  }

  function copyCommand(root) {
    var result = render(root);
    var status = root.querySelector("#bp-install-status");
    var copy = root.querySelector("#bp-install-copy");
    if (result.error) {
      if (status) {
        status.textContent = result.error;
      }
      return;
    }
    var done = function (ok) {
      if (status) {
        status.textContent = ok ? "Copied." : "Copy failed. Select the command and copy it manually.";
      }
      if (copy) {
        copy.classList.toggle("is-copied", ok);
        copy.textContent = ok ? "Copied" : "Copy";
      }
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(result.command).then(
        function () {
          done(true);
        },
        function () {
          done(false);
        }
      );
      return;
    }
    var range = document.createRange();
    var output = root.querySelector("#bp-install-command");
    if (!output) {
      done(false);
      return;
    }
    range.selectNodeContents(output);
    var selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    try {
      done(document.execCommand("copy"));
    } catch (error) {
      done(false);
    }
    selection.removeAllRanges();
  }

  function bind(root) {
    if (root.getAttribute("data-bp-bound") === "1") {
      render(root);
      return;
    }
    root.setAttribute("data-bp-bound", "1");
    var form = root.querySelector(".bp-install-builder__form");
    if (form) {
      form.addEventListener("submit", function (event) {
        event.preventDefault();
      });
      form.addEventListener("input", function () {
        var status = root.querySelector("#bp-install-status");
        var copy = root.querySelector("#bp-install-copy");
        if (status) {
          status.textContent = "";
        }
        if (copy) {
          copy.classList.remove("is-copied");
          copy.textContent = "Copy";
        }
        render(root);
        var error = root.getAttribute("data-bp-install-error");
        if (status && error) {
          status.textContent = error;
        }
      });
      form.addEventListener("change", function () {
        render(root);
      });
    }
    var copy = root.querySelector("#bp-install-copy");
    if (copy) {
      copy.addEventListener("click", function () {
        copyCommand(root);
      });
    }
    render(root);
  }

  function boot() {
    document.querySelectorAll("[data-bp-install-builder]").forEach(bind);
  }

  if (typeof module === "object" && module.exports) {
    module.exports = {
      buildCommand: buildCommand,
      shellQuote: shellQuote,
      validationError: validationError,
    };
  }

  if (typeof document === "undefined") {
    return;
  }

  if (typeof document$ !== "undefined") {
    document$.subscribe(boot);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
