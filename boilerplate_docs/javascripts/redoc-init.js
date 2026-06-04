/* ReDoc iframe mount for /api/reference/ only. */
(function () {
  "use strict";

  const FRAME_ID = "redoc-frame";
  const API_REFERENCE_RE = /\/api\/reference\/?(index\.html)?$/;

  function isReferencePage() {
    return API_REFERENCE_RE.test(window.location.pathname);
  }

  function contentRoot() {
    return document.querySelector("article.md-content__inner");
  }

  function redocUrl() {
    const url = new URL("../redoc.html", window.location.href);
    url.searchParams.set("_", String(Date.now()));
    return url.href;
  }

  function mountRedoc() {
    if (!isReferencePage()) {
      return;
    }

    const root = contentRoot();
    if (!root) {
      return;
    }

    let frame = document.getElementById(FRAME_ID);
    if (!frame) {
      frame = document.createElement("iframe");
      frame.id = FRAME_ID;
      frame.className = "redoc-frame";
      frame.title = "API reference";
      root.appendChild(frame);
    }

    frame.src = redocUrl();
  }

  function scheduleMount() {
    requestAnimationFrame(function () {
      requestAnimationFrame(mountRedoc);
    });
  }

  function boot() {
    scheduleMount();
    if (window.document$ && typeof window.document$.subscribe === "function") {
      window.document$.subscribe(scheduleMount);
    }
    if (window.location$ && typeof window.location$.subscribe === "function") {
      window.location$.subscribe(scheduleMount);
    }
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
