/* ReDoc in an isolated iframe; remount on Material instant navigation */
(function () {
  "use strict";

  const FRAME_ID = "redoc-frame";
  const API_PATH_RE = /\/api\/?(index\.html)?$/;

  let loadScheduled = false;

  function isApiPage() {
    return API_PATH_RE.test(window.location.pathname);
  }

  function frameDocumentUrl() {
    return new URL("redoc.html", window.location.href);
  }

  function contentRoot() {
    return document.querySelector("article.md-content__inner");
  }

  function hasInlineRedoc(root) {
    return Boolean(
      root.querySelector(
        "#redoc-container, .redoc-wrap, [data-section-id], .scrollbar-container"
      )
    );
  }

  function resetApiArticle(root) {
    const h1 = root.querySelector("h1");
    const title = h1 ? h1.cloneNode(true) : null;

    root.replaceChildren();
    if (title) {
      root.appendChild(title);
    }
  }

  function ensureFrame() {
    const root = contentRoot();
    if (!root) {
      return null;
    }

    let frame = document.getElementById(FRAME_ID);

    if (hasInlineRedoc(root) || (frame && frame.parentElement !== root)) {
      resetApiArticle(root);
      frame = null;
    }

    if (!frame) {
      frame = document.createElement("iframe");
      frame.id = FRAME_ID;
      frame.className = "redoc-frame";
      frame.title = "API reference";
      root.appendChild(frame);
    }

    return frame;
  }

  function loadFrame() {
    if (!isApiPage()) {
      return;
    }

    const frame = ensureFrame();
    if (!frame) {
      return;
    }

    const next = frameDocumentUrl();
    next.searchParams.set("_", String(Date.now()));

    if (frame.src !== next.href) {
      frame.src = next.href;
    }
  }

  function scheduleLoadFrame() {
    if (loadScheduled) {
      return;
    }
    loadScheduled = true;
    requestAnimationFrame(function () {
      requestAnimationFrame(function () {
        loadScheduled = false;
        if (!isApiPage()) {
          return;
        }
        loadFrame();
      });
    });
  }

  function hookStream(stream) {
    if (stream && typeof stream.subscribe === "function") {
      stream.subscribe(scheduleLoadFrame);
    }
  }

  function apiLinkUrl(link) {
    try {
      return new URL(link.href, window.location.href);
    } catch (_err) {
      return null;
    }
  }

  function disableInstantNavForApiLinks() {
    document.querySelectorAll('a[href]').forEach(function (link) {
      if (link.target || link.hasAttribute("download")) {
        return;
      }

      const target = apiLinkUrl(link);
      if (target && API_PATH_RE.test(target.pathname)) {
        link.setAttribute("data-md-no-instant", "");
      }
    });
  }

  function navigateToApi(url) {
    url.searchParams.set("_", String(Date.now()));
    window.location.assign(url.href);
  }

  function interceptApiLinks() {
    document.body.addEventListener(
      "click",
      function (event) {
        if (!(event.target instanceof Element)) {
          return;
        }

        const link = event.target.closest("a");
        if (!link || link.target || event.metaKey || event.ctrlKey || event.shiftKey) {
          return;
        }

        const target = apiLinkUrl(link);
        if (!target || !API_PATH_RE.test(target.pathname) || isApiPage()) {
          return;
        }

        event.preventDefault();
        event.stopImmediatePropagation();
        navigateToApi(target);
      },
      true
    );
  }

  function hookNavigation() {
    hookStream(window.document$);
    hookStream(window.location$);

    if (window.document$ && typeof window.document$.subscribe === "function") {
      window.document$.subscribe(disableInstantNavForApiLinks);
    }
  }

  function boot() {
    hookNavigation();
    interceptApiLinks();
    disableInstantNavForApiLinks();
    scheduleLoadFrame();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
