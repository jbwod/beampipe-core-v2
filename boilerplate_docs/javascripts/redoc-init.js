(function () {
  const REDOC_CDN =
    "https://cdn.redoc.ly/redoc/v2.1.5/bundles/redoc.standalone.js";
  const CONTAINER_ID = "redoc-container";

  let redocScriptPromise = null;

  function loadRedocScript() {
    if (window.Redoc) {
      return Promise.resolve();
    }
    if (redocScriptPromise) {
      return redocScriptPromise;
    }
    redocScriptPromise = new Promise(function (resolve, reject) {
      const script = document.createElement("script");
      script.src = REDOC_CDN;
      script.async = true;
      script.onload = function () {
        resolve();
      };
      script.onerror = function () {
        redocScriptPromise = null;
        reject(new Error("Failed to load ReDoc from CDN"));
      };
      document.head.appendChild(script);
    });
    return redocScriptPromise;
  }

  function specUrl() {
    return new URL("../openapi.json", window.location.href).href;
  }

  function mountRedoc() {
    const container = document.getElementById(CONTAINER_ID);
    if (!container) {
      return;
    }
    container.innerHTML = "";
    loadRedocScript()
      .then(function () {
        if (!document.getElementById(CONTAINER_ID)) {
          return;
        }
        window.Redoc.init(
          specUrl(),
          {
            scrollYOffset: 64,
            hideDownloadButton: false,
            expandResponses: "200,201",
            theme: {
              colors: {
                primary: { main: "#2f81f7" },
                success: { main: "#3fb950" },
                warning: { main: "#d29922" },
                error: { main: "#f85149" },
                text: { primary: "#e6edf3", secondary: "#7d8590" },
                border: { dark: "#30363d", light: "#21262d" },
                http: {
                  get: "#3fb950",
                  post: "#2f81f7",
                  put: "#d29922",
                  patch: "#a371f7",
                  delete: "#f85149",
                },
              },
              typography: {
                fontFamily:
                  '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif',
                headings: {
                  fontFamily:
                    '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif',
                },
                code: {
                  color: "#e6edf3",
                  backgroundColor: "#161b22",
                  fontFamily:
                    'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace',
                },
                links: { color: "#2f81f7" },
              },
              sidebar: {
                backgroundColor: "#0d1117",
                textColor: "#e6edf3",
                activeTextColor: "#58a6ff",
              },
              rightPanel: {
                backgroundColor: "#010409",
                textColor: "#e6edf3",
              },
              schema: {
                nestedBackground: "#161b22",
                typeNameColor: "#7d8590",
                typeTitleColor: "#e6edf3",
              },
            },
          },
          container
        );
      })
      .catch(function (err) {
        container.innerHTML =
          '<p class="redoc-error">Could not load ReDoc: ' +
          String(err.message || err) +
          "</p>";
      });
  }

  function onReady(fn) {
    if (typeof document$ !== "undefined" && document$.subscribe) {
      document$.subscribe(fn);
    } else if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", fn);
    } else {
      fn();
    }
  }

  onReady(mountRedoc);
})();
