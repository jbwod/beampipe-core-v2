(function () {
  "use strict";

  var INSTALL_URL =
    "https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh";
  var CURL = "curl -fsSL " + INSTALL_URL + " | sh";
  var DEFAULT_API_PORT = "18080";
  var DEFAULT_POSTGRES_PORT = "5432";
  var DEFAULT_METRICS_PORT = "9090";

  function shellQuote(value) {
    if (/^[A-Za-z0-9_./~:@+-]+$/.test(value)) {
      return value;
    }
    return "'" + String(value).replace(/'/g, "'\\''") + "'";
  }

  function buildCommand(state) {
    var flags = [];
    if (state.yes) {
      flags.push("--yes");
      flags.push("--runtime", state.runtime);
    }
    if (!state.start) {
      flags.push("--no-start");
    }
    if (state.dashboard && state.runtime === "docker") {
      flags.push("--dashboard");
    }
    if (state.directory) {
      flags.push("--directory", shellQuote(state.directory));
    }
    if (state.adminUser) {
      flags.push("--admin-user", shellQuote(state.adminUser));
    }
    if (state.adminEmail) {
      flags.push("--admin-email", shellQuote(state.adminEmail));
    }
    if (state.adminPassword) {
      flags.push("--admin-password", shellQuote(state.adminPassword));
    }
    addPortFlag(flags, "--api-port", state.apiPort, DEFAULT_API_PORT, state.yes);
    addPortFlag(flags, "--postgres-port", state.postgresPort, DEFAULT_POSTGRES_PORT, state.yes);
    addPortFlag(flags, "--metrics-port", state.metricsPort, DEFAULT_METRICS_PORT, state.yes);
    if (!flags.length) {
      return CURL;
    }
    return CURL + " -s -- " + flags.join(" ");
  }

  function addPortFlag(flags, name, value, defaultValue, yes) {
    if (!value) {
      return;
    }
    if (yes && value === defaultValue) {
      return;
    }
    flags.push(name, value);
  }

  function selectedRuntime(root) {
    var radio = root.querySelector('input[name="runtime"]:checked');
    var select = root.querySelector("#bp-install-runtime");
    var value = radio ? radio.value : select ? select.value : "docker";
    return value === "host" ? "host" : "docker";
  }

  function readState(root) {
    var directory = root.querySelector("#bp-install-directory");
    var yes = root.querySelector("#bp-install-yes");
    var start = root.querySelector("#bp-install-start");
    var dashboard = root.querySelector("#bp-install-dashboard");
    var adminUser = root.querySelector("#bp-install-admin-user");
    var adminEmail = root.querySelector("#bp-install-admin-email");
    var adminPassword = root.querySelector("#bp-install-admin-password");
    return {
      runtime: selectedRuntime(root),
      directory: directory ? directory.value.trim() : "",
      yes: !yes || yes.checked,
      start: !start || start.checked,
      dashboard: Boolean(dashboard && dashboard.checked),
      adminUser: adminUser ? adminUser.value.trim() : "",
      adminEmail: adminEmail ? adminEmail.value.trim() : "",
      adminPassword: adminPassword ? adminPassword.value : "",
      apiPort: fieldValue(root, "#bp-install-api-port"),
      postgresPort: fieldValue(root, "#bp-install-postgres-port"),
      metricsPort: fieldValue(root, "#bp-install-metrics-port"),
    };
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
    state = readState(root);
    var command = buildCommand(state);
    var output = root.querySelector("#bp-install-command");
    if (output) {
      output.textContent = command;
    }
    var apiUrl = root.querySelector("#bp-install-api-url");
    if (apiUrl) {
      apiUrl.textContent =
        "http://127.0.0.1:" + (state.apiPort || DEFAULT_API_PORT) + "/api/v2";
    }
    return command;
  }

  function copyCommand(root) {
    var command = render(root);
    var status = root.querySelector("#bp-install-status");
    var copy = root.querySelector("#bp-install-copy");
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
      navigator.clipboard.writeText(command).then(
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

  if (typeof document$ !== "undefined") {
    document$.subscribe(boot);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
