(function () {
  "use strict";

  const svgNamespace = "http://www.w3.org/2000/svg";
  const schemaRequests = new Map();

  const tableMetadata = {
    alert_deliveries: ["alerts", "Notification attempts and their terminal delivery outcomes."],
    alert_rules: ["alerts", "Operator-defined alert conditions, severity, and cooldown policy."],
    archive_metadata: ["discovery", "Normalized archive facts grouped by project-defined identity."],
    batch_execution_record: ["ledger", "Authoritative execution ledger and pinned runtime evidence."],
    daliuge_deployment_profile: ["work", "Versioned translation, deployment, and concurrency policy."],
    execution_artifacts: ["ledger", "Content-addressed manifests, graphs, receipts, and inventories."],
    execution_observations: ["ledger", "Timestamped scheduler, DALiuGE, and output observations."],
    job_claim_history: ["work", "Append-only claim, lease, recovery, and fencing history."],
    jobs: ["work", "Durable asynchronous work queue with retry and lease state."],
    notification_channels: ["alerts", "Named webhook and email delivery targets."],
    project_config_wasm: ["config", "Content-addressed WASM extensions pinned to project revisions."],
    project_configs: ["config", "Immutable project specifications with one active revision."],
    provenance_events: ["ledger", "Append-only audit stream for operator and system actions."],
    source_registry: ["discovery", "Enrolled archive sources, discovery claims, and workflow intent."],
    token_blacklist: ["identity", "Hashes of revoked access tokens until their expiry."],
    users: ["identity", "Operator identities, password hashes, and privilege state."],
    worker_instances: ["work", "Worker heartbeats, pools, labels, and advertised capabilities."]
  };

  const groupLabels = {
    alerts: "alerting",
    config: "project config",
    discovery: "discovery",
    identity: "identity",
    ledger: "execution ledger",
    work: "distributed work"
  };

  const graphPositions = {
    users: [24, 28],
    token_blacklist: [24, 88],
    project_configs: [24, 174],
    project_config_wasm: [24, 234],
    source_registry: [24, 326],
    archive_metadata: [24, 386],
    notification_channels: [24, 520],
    daliuge_deployment_profile: [286, 28],
    batch_execution_record: [286, 112],
    jobs: [286, 278],
    worker_instances: [286, 368],
    alert_rules: [286, 520],
    execution_observations: [548, 72],
    execution_artifacts: [548, 132],
    provenance_events: [548, 192],
    job_claim_history: [548, 338],
    alert_deliveries: [548, 520]
  };

  function element(tagName, className, text) {
    const item = document.createElement(tagName);
    if (className) {
      item.className = className;
    }
    if (text !== undefined) {
      item.textContent = text;
    }
    return item;
  }

  function svgElement(tagName, attributes) {
    const item = document.createElementNS(svgNamespace, tagName);
    Object.entries(attributes || {}).forEach(([name, value]) => {
      item.setAttribute(name, String(value));
    });
    return item;
  }

  function metadataFor(table) {
    const metadata = tableMetadata[table.name] || ["other", table.description || "PostgreSQL catalog table."];
    return { group: metadata[0], purpose: table.description || metadata[1] };
  }

  function compactType(type) {
    return type
      .replace("character varying", "varchar")
      .replace("timestamp with time zone", "timestamptz")
      .replace("timestamp without time zone", "timestamp");
  }

  function badge(text, tone) {
    const item = element("span", "bp-db-badge", text);
    if (tone) {
      item.setAttribute("data-tone", tone);
    }
    return item;
  }

  function countLabel(count, singular, plural) {
    return count + " " + (count === 1 ? singular : (plural || singular + "s"));
  }

  function linkedRelationships(schema, tableName) {
    return schema.relationships.filter((relationship) =>
      relationship.from_table === tableName || relationship.to_table === tableName
    );
  }

  function tableSearchText(table) {
    const metadata = metadataFor(table);
    return [
      table.name,
      metadata.group,
      metadata.purpose,
      ...table.columns.flatMap((column) => [column.name, column.type])
    ].join(" ").toLowerCase();
  }

  function schemaRequest(url) {
    if (!schemaRequests.has(url)) {
      schemaRequests.set(url, fetch(url, { credentials: "same-origin" }).then((response) => {
        if (!response.ok) {
          throw new Error("schema snapshot returned HTTP " + response.status);
        }
        return response.json();
      }));
    }
    return schemaRequests.get(url);
  }

  function setMetrics(root, schema) {
    const metrics = {
      tables: schema.table_count,
      columns: schema.column_count,
      relationships: schema.relationship_count,
      indexes: schema.index_count
    };

    Object.entries(metrics).forEach(([name, value]) => {
      const target = root.querySelector('[data-bp-schema-metric="' + name + '"]');
      if (target) {
        target.textContent = String(value);
      }
    });

    root.querySelector("[data-bp-schema-summary]").textContent =
      schema.table_count + " tables / " + schema.column_count + " columns";
    root.querySelector("[data-bp-schema-migration]").textContent =
      "migration " + schema.latest_migration;
  }

  function renderInventory(root, schema, state) {
    const list = root.querySelector("[data-bp-schema-table-list]");
    const query = state.query.trim().toLowerCase();
    const visibleTables = schema.tables.filter((table) => {
      const metadata = metadataFor(table);
      const groupMatches = state.group === "all" || metadata.group === state.group;
      return groupMatches && (!query || tableSearchText(table).includes(query));
    });

    list.replaceChildren();
    visibleTables.forEach((table) => {
      const metadata = metadataFor(table);
      const relationshipCount = linkedRelationships(schema, table.name).length;
      const button = element("button", "bp-db-table");
      button.type = "button";
      button.setAttribute("role", "option");
      button.setAttribute("data-bp-schema-link", table.name);
      button.setAttribute("aria-selected", String(table.name === state.selected));

      const heading = element("span", "bp-db-table__name");
      heading.append(badge(metadata.group.toUpperCase(), metadata.group));
      heading.append(element("code", "", table.name));
      button.append(heading);
      button.append(element(
        "small",
        "",
        table.columns.length + " cols · " + countLabel(relationshipCount, "link")
      ));
      list.append(button);
    });

    if (!visibleTables.length) {
      list.append(element("p", "bp-db-explorer__empty", "No schema objects match this filter."));
    }

    root.querySelector("[data-bp-schema-result-count]").textContent =
      visibleTables.length + " / " + schema.tables.length;
  }

  function tableSection(title, count) {
    const heading = element("div", "bp-db-detail__section-title");
    heading.append(element("h3", "", title));
    heading.append(element("span", "", String(count)));
    return heading;
  }

  function renderColumnTable(table, selectTable) {
    const wrapper = element("div", "bp-db-columns");
    const grid = element("table", "");
    const head = document.createElement("thead");
    const headingRow = document.createElement("tr");
    ["key", "column", "type", "null", "default / reference"].forEach((label) => {
      headingRow.append(element("th", "", label));
    });
    head.append(headingRow);
    grid.append(head);

    const body = document.createElement("tbody");
    table.columns.forEach((column) => {
      const row = document.createElement("tr");
      const keyCell = document.createElement("td");
      if (column.primary_key) {
        keyCell.append(badge("PK", "primary"));
      }
      if (column.references) {
        keyCell.append(badge("FK", "foreign"));
      }
      if (!column.primary_key && !column.references) {
        keyCell.textContent = "·";
      }
      row.append(keyCell);

      const nameCell = document.createElement("td");
      nameCell.append(element("code", "", column.name));
      row.append(nameCell);

      const typeCell = document.createElement("td");
      typeCell.append(element("code", "", compactType(column.type)));
      row.append(typeCell);
      row.append(element("td", "", column.nullable ? "yes" : "no"));

      const contractCell = document.createElement("td");
      if (column.references) {
        const reference = element(
          "button",
          "bp-db-reference",
          "→ " + column.references.table + "." + column.references.column
        );
        reference.type = "button";
        reference.title = "ON DELETE " + column.references.on_delete;
        reference.addEventListener("click", () => selectTable(column.references.table, true));
        contractCell.append(reference);
        contractCell.append(element("small", "", "on delete " + column.references.on_delete.toLowerCase()));
      } else if (column.default !== null) {
        contractCell.append(element("code", "", column.default));
      } else {
        contractCell.textContent = "—";
      }
      row.append(contractCell);
      body.append(row);
    });
    grid.append(body);
    wrapper.append(grid);
    return wrapper;
  }

  function renderRelationshipList(schema, table, selectTable) {
    const relationships = linkedRelationships(schema, table.name);
    const list = element("div", "bp-db-relationships");

    relationships.forEach((relationship) => {
      const outgoing = relationship.from_table === table.name;
      const peerTable = outgoing ? relationship.to_table : relationship.from_table;
      const source = relationship.from_table + "." + relationship.from_column;
      const target = relationship.to_table + "." + relationship.to_column;
      const item = element("button", "");
      item.type = "button";
      item.addEventListener("click", () => selectTable(peerTable, true));
      item.append(badge(outgoing ? "OUT" : "IN", outgoing ? "foreign" : "incoming"));
      item.append(element("code", "", source + " → " + target));
      item.append(element("small", "", "ON DELETE " + relationship.on_delete));
      list.append(item);
    });

    if (!relationships.length) {
      list.append(element("p", "bp-db-explorer__empty", "No declared foreign-key relationships."));
    }
    return list;
  }

  function renderDefinitions(items, kind) {
    const wrapper = element("div", "bp-db-definitions");
    items.forEach((item) => {
      const details = document.createElement("details");
      const summary = document.createElement("summary");
      summary.append(element("code", "", item.name));
      if (kind === "index") {
        summary.append(badge(item.method.toUpperCase(), item.unique ? "primary" : "index"));
        if (item.unique) {
          summary.append(badge("UNIQUE", "foreign"));
        }
      }
      details.append(summary);
      const pre = document.createElement("pre");
      pre.append(element("code", "", item.definition));
      details.append(pre);
      wrapper.append(details);
    });
    return wrapper;
  }

  function renderDetail(root, schema, state, selectTable) {
    const table = schema.tables.find((candidate) => candidate.name === state.selected);
    if (!table) {
      return;
    }

    const metadata = metadataFor(table);
    const detail = root.querySelector("[data-bp-schema-detail]");
    detail.replaceChildren();

    const header = element("header", "bp-db-detail__header");
    const label = element("div", "bp-db-detail__label");
    label.append(badge(metadata.group.toUpperCase(), metadata.group));
    label.append(element("span", "", groupLabels[metadata.group] || metadata.group));
    header.append(label);
    const title = element("h2", "");
    title.append(element("code", "", table.name));
    header.append(title);
    header.append(element("p", "", metadata.purpose));

    const summary = element("div", "bp-db-detail__stats");
    summary.append(element("span", "", countLabel(table.columns.length, "column")));
    summary.append(element("span", "", countLabel(table.indexes.length, "index", "indexes")));
    summary.append(element("span", "", countLabel(table.checks.length, "check")));
    summary.append(element(
      "span",
      "",
      countLabel(linkedRelationships(schema, table.name).length, "link")
    ));
    header.append(summary);
    detail.append(header);

    detail.append(tableSection("Columns", table.columns.length));
    detail.append(renderColumnTable(table, selectTable));

    const relationships = linkedRelationships(schema, table.name);
    detail.append(tableSection("Relationships", relationships.length));
    detail.append(renderRelationshipList(schema, table, selectTable));

    if (table.indexes.length) {
      detail.append(tableSection("Indexes", table.indexes.length));
      detail.append(renderDefinitions(table.indexes, "index"));
    }

    if (table.checks.length) {
      detail.append(tableSection("Check constraints", table.checks.length));
      detail.append(renderDefinitions(table.checks, "check"));
    }
  }

  function renderMap(root, schema, state, selectTable) {
    const target = root.querySelector("[data-bp-schema-map]");
    const map = svgElement("svg", {
      class: "bp-db-map",
      viewBox: "0 0 800 590",
      role: "img",
      "aria-label": "Interactive map of declared database foreign keys"
    });
    const title = svgElement("title");
    title.textContent = "Beampipe Core PostgreSQL foreign-key map";
    map.append(title);

    const definitions = svgElement("defs");
    const marker = svgElement("marker", {
      id: "bp-db-arrow",
      viewBox: "0 0 10 10",
      refX: "8",
      refY: "5",
      markerWidth: "5",
      markerHeight: "5",
      orient: "auto-start-reverse"
    });
    marker.append(svgElement("path", { d: "M 0 0 L 10 5 L 0 10 z" }));
    definitions.append(marker);
    map.append(definitions);

    const connectedTables = new Set([state.selected]);
    linkedRelationships(schema, state.selected).forEach((relationship) => {
      connectedTables.add(relationship.from_table);
      connectedTables.add(relationship.to_table);
    });

    schema.relationships.forEach((relationship) => {
      const source = graphPositions[relationship.from_table];
      const destination = graphPositions[relationship.to_table];
      if (!source || !destination) {
        return;
      }

      const sourceX = source[0] + 114;
      const sourceY = source[1] + 23;
      const targetX = destination[0] + 114;
      const targetY = destination[1] + 23;
      const middleX = (sourceX + targetX) / 2;
      const active = relationship.from_table === state.selected || relationship.to_table === state.selected;
      const path = svgElement("path", {
        d: "M " + sourceX + " " + sourceY + " C " + middleX + " " + sourceY + ", " + middleX + " " + targetY + ", " + targetX + " " + targetY,
        class: active ? "bp-db-map__edge is-active" : "bp-db-map__edge",
        "marker-end": "url(#bp-db-arrow)"
      });
      const edgeTitle = svgElement("title");
      edgeTitle.textContent = relationship.from_table + "." + relationship.from_column +
        " → " + relationship.to_table + "." + relationship.to_column;
      path.append(edgeTitle);
      map.append(path);
    });

    schema.tables.forEach((table) => {
      const position = graphPositions[table.name];
      if (!position) {
        return;
      }
      const metadata = metadataFor(table);
      const node = svgElement("g", {
        class: "bp-db-map__node" +
          (table.name === state.selected ? " is-active" : "") +
          (!connectedTables.has(table.name) ? " is-muted" : ""),
        transform: "translate(" + position[0] + " " + position[1] + ")",
        role: "button",
        tabindex: "0",
        "data-bp-schema-link": table.name,
        "aria-label": "Inspect table " + table.name
      });
      node.append(svgElement("rect", { width: "228", height: "46", rx: "0" }));
      const groupText = svgElement("text", { x: "10", y: "14", class: "bp-db-map__group" });
      groupText.textContent = metadata.group.toUpperCase();
      node.append(groupText);
      const nameText = svgElement("text", { x: "10", y: "31", class: "bp-db-map__name" });
      nameText.textContent = table.name;
      node.append(nameText);
      const countText = svgElement("text", { x: "218", y: "31", "text-anchor": "end", class: "bp-db-map__count" });
      countText.textContent = table.columns.length + "c";
      node.append(countText);
      node.addEventListener("click", () => selectTable(table.name, true));
      node.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectTable(table.name, true);
        }
      });
      map.append(node);
    });

    target.replaceChildren(map);
  }

  function updateHash(tableName) {
    const target = window.location.pathname + window.location.search +
      "#schema-" + encodeURIComponent(tableName);
    window.history.replaceState(null, "", target);
  }

  function initialiseExplorer(root, schema) {
    const hashTable = decodeURIComponent(window.location.hash.replace(/^#schema-/, ""));
    const defaultTable = schema.tables.some((table) => table.name === hashTable)
      ? hashTable
      : "batch_execution_record";
    const state = { group: "all", query: "", selected: defaultTable };

    function selectTable(tableName, changeHash) {
      if (!schema.tables.some((table) => table.name === tableName)) {
        return;
      }
      state.selected = tableName;
      renderInventory(root, schema, state);
      renderDetail(root, schema, state, selectTable);
      renderMap(root, schema, state, selectTable);
      if (changeHash) {
        updateHash(tableName);
      }
    }

    const search = root.querySelector("[data-bp-schema-search]");
    search.addEventListener("input", () => {
      state.query = search.value;
      renderInventory(root, schema, state);
    });
    search.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && search.value) {
        search.value = "";
        state.query = "";
        renderInventory(root, schema, state);
      }
    });

    root.querySelectorAll("[data-bp-schema-group]").forEach((button) => {
      button.addEventListener("click", () => {
        state.group = button.getAttribute("data-bp-schema-group");
        root.querySelectorAll("[data-bp-schema-group]").forEach((candidate) => {
          candidate.setAttribute("aria-pressed", String(candidate === button));
        });
        renderInventory(root, schema, state);
      });
    });

    root.addEventListener("click", (event) => {
      const link = event.target.closest("[data-bp-schema-link]");
      if (link && !link.closest("[data-bp-schema-map]")) {
        selectTable(link.getAttribute("data-bp-schema-link"), true);
      }
    });

    setMetrics(root, schema);
    selectTable(defaultTable, false);
    root.setAttribute("aria-busy", "false");
    root.setAttribute("data-bp-ready", "true");
  }

  function showError(root, error) {
    const message = element(
      "p",
      "bp-db-explorer__error",
      "Could not load the database schema snapshot: " + error.message
    );
    root.querySelector("[data-bp-schema-table-list]").replaceChildren(message.cloneNode(true));
    root.querySelector("[data-bp-schema-detail]").replaceChildren(message.cloneNode(true));
    root.querySelector("[data-bp-schema-map]").replaceChildren(message);
    root.setAttribute("aria-busy", "false");
  }

  function boot() {
    document.querySelectorAll("[data-bp-database-explorer]").forEach((root) => {
      if (root.hasAttribute("data-bp-loading") || root.hasAttribute("data-bp-ready")) {
        return;
      }
      root.setAttribute("data-bp-loading", "true");
      const url = new URL(root.getAttribute("data-schema-url"), window.location.href).href;
      schemaRequest(url)
        .then((schema) => initialiseExplorer(root, schema))
        .catch((error) => showError(root, error));
    });
  }

  if (typeof document$ !== "undefined") {
    document$.subscribe(boot);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
