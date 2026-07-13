(function () {
  "use strict";

  let clockTimer;
  let progressFrame;

  function pad(value) {
    return String(value).padStart(2, "0");
  }

  function statusTime() {
    const now = new Date();
    return pad(now.getHours()) + ":" + pad(now.getMinutes()) + ":" + pad(now.getSeconds());
  }

  function updateStatusBar() {
    const time = statusTime();
    document.querySelectorAll(".md-footer-meta, .md-header__inner").forEach((element) => {
      element.setAttribute("data-bp-time", time);
    });
  }

  function seededRandom(seed) {
    let state = seed >>> 0;
    return function random() {
      state = (state * 1664525 + 1013904223) >>> 0;
      return state / 4294967296;
    };
  }

  function createStarfield() {
    if (document.querySelector(".bp-starfield")) {
      return;
    }

    const random = seededRandom(0xbea2);
    const field = document.createElement("div");
    const glyphs = [".", ".", ".", ".", "*", "+"];
    field.className = "bp-starfield";
    field.setAttribute("aria-hidden", "true");

    for (let index = 0; index < 96; index += 1) {
      const star = document.createElement("span");
      star.textContent = glyphs[Math.floor(random() * glyphs.length)];
      star.style.setProperty("--bp-star-x", (random() * 100).toFixed(2) + "vw");
      star.style.setProperty("--bp-star-y", (random() * 100).toFixed(2) + "%");
      star.style.setProperty("--bp-star-alpha", (0.12 + random() * 0.26).toFixed(2));
      star.style.setProperty("--bp-star-size", (0.46 + random() * 0.36).toFixed(2) + "rem");
      star.style.setProperty("--bp-star-speed", (3.8 + random() * 5).toFixed(2) + "s");
      star.style.setProperty("--bp-star-delay", (-random() * 8).toFixed(2) + "s");
      field.appendChild(star);
    }

    document.body.prepend(field);
  }

  function pagePath() {
    let path = decodeURIComponent(window.location.pathname)
      .replace(/\/index\.html$/, "/")
      .replace(/\.html$/, "")
      .replace(/^\/+|\/+$/g, "");

    if (!path) {
      return "~/docs/home";
    }
    return "~/docs/" + path;
  }

  function addPageContext() {
    const article = document.querySelector(".md-content__inner");
    if (!article) {
      return;
    }

    article.setAttribute("data-bp-path", pagePath());
    const heading = Array.from(article.children).find((element) => element.tagName === "H1");
    if (!heading || article.querySelector(":scope > .bp-page-meta")) {
      return;
    }

    const words = article.textContent.trim().split(/\s+/).filter(Boolean).length;
    const minutes = Math.max(1, Math.round(words / 210));
    const meta = document.createElement("div");
    meta.className = "bp-page-meta";
    meta.setAttribute("aria-label", "Article context");
    meta.innerHTML =
      "<span>[ ARTICLE ]</span><span>" + minutes + " min read</span><span>api / v2</span>";
    heading.insertAdjacentElement("afterend", meta);
  }

  function controlsFor(container) {
    return Array.from(container.querySelectorAll("[data-bp-target]"));
  }

  function activate(container, targetId, focus) {
    const controls = controlsFor(container);
    const panels = Array.from(container.querySelectorAll("[data-bp-panel]"));

    controls.forEach((control) => {
      const active = control.getAttribute("data-bp-target") === targetId;
      if (control.getAttribute("role") === "tab") {
        control.setAttribute("aria-selected", String(active));
        control.tabIndex = active ? 0 : -1;
      } else {
        control.setAttribute("aria-pressed", String(active));
      }
      if (active && focus) {
        control.focus();
      }
    });

    panels.forEach((panel) => {
      panel.hidden = panel.id !== targetId;
    });
  }

  function initInteractive(container) {
    if (container.hasAttribute("data-bp-ready")) {
      return;
    }
    container.setAttribute("data-bp-ready", "true");

    const controls = controlsFor(container);
    controls.forEach((control, index) => {
      control.addEventListener("click", () => {
        activate(container, control.getAttribute("data-bp-target"), false);
      });

      if (control.getAttribute("role") !== "tab") {
        return;
      }

      control.addEventListener("keydown", (event) => {
        let nextIndex = index;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") {
          nextIndex = (index + 1) % controls.length;
        } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
          nextIndex = (index - 1 + controls.length) % controls.length;
        } else if (event.key === "Home") {
          nextIndex = 0;
        } else if (event.key === "End") {
          nextIndex = controls.length - 1;
        } else {
          return;
        }

        event.preventDefault();
        const next = controls[nextIndex];
        activate(container, next.getAttribute("data-bp-target"), true);
      });
    });
  }

  function initInteractions() {
    document.querySelectorAll("[data-bp-switcher], [data-bp-explorer]").forEach(initInteractive);
  }

  function initCopyFeedback() {
    document.querySelectorAll(".md-clipboard").forEach((button) => {
      if (button.hasAttribute("data-bp-ready")) {
        return;
      }
      button.setAttribute("data-bp-ready", "true");
      button.addEventListener("click", () => {
        button.setAttribute("data-bp-copy-state", "copied");
        window.setTimeout(() => button.removeAttribute("data-bp-copy-state"), 1600);
      });
    });
  }

  function ensureProgressBar() {
    let bar = document.querySelector(".bp-reading-progress");
    if (!bar) {
      bar = document.createElement("div");
      bar.className = "bp-reading-progress";
      bar.setAttribute("aria-hidden", "true");
      document.body.appendChild(bar);
    }
    return bar;
  }

  function updateProgress() {
    progressFrame = undefined;
    const article = document.querySelector(".md-content__inner");
    const bar = ensureProgressBar();
    if (!article) {
      bar.style.setProperty("--bp-reading-progress", "0");
      return;
    }

    const start = article.offsetTop;
    const distance = Math.max(1, article.offsetHeight - window.innerHeight * 0.65);
    const progress = Math.min(1, Math.max(0, (window.scrollY - start) / distance));
    bar.style.setProperty("--bp-reading-progress", progress.toFixed(4));
  }

  function scheduleProgress() {
    if (progressFrame === undefined) {
      progressFrame = window.requestAnimationFrame(updateProgress);
    }
  }

  function boot() {
    document.body.classList.toggle("bp-home", pagePath() === "~/docs/home");
    createStarfield();
    addPageContext();
    initInteractions();
    initCopyFeedback();
    updateStatusBar();
    updateProgress();

    if (!clockTimer) {
      clockTimer = window.setInterval(updateStatusBar, 1000);
      window.addEventListener("scroll", scheduleProgress, { passive: true });
      window.addEventListener("resize", scheduleProgress, { passive: true });
    }
  }

  if (typeof document$ !== "undefined") {
    document$.subscribe(boot);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
