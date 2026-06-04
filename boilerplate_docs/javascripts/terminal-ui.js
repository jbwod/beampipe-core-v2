(function () {
  "use strict";

  function pad(value) {
    return String(value).padStart(2, "0");
  }

  function statusTime() {
    const now = new Date();
    return pad(now.getHours()) + ":" + pad(now.getMinutes()) + ":" + pad(now.getSeconds());
  }

  function updateStatusBar() {
    const footer = document.querySelector(".md-footer-meta");
    const header = document.querySelector(".md-header__inner");
    const time = statusTime();

    if (footer) {
      footer.setAttribute("data-bp-time", time);
    }
    if (header) {
      header.setAttribute("data-bp-time", time);
    }
  }

  function createStarfield() {
    document.querySelectorAll(".bp-starfield").forEach((field) => field.remove());

    const field = document.createElement("div");
    const glyphs = [".", ".", ".", ".", ".", "*", "*", "+", "'"];
    field.className = "bp-starfield";
    field.setAttribute("aria-hidden", "true");

    for (let index = 0; index < 320; index += 1) {
      const star = document.createElement("span");
      const x = (Math.random() * 100).toFixed(2) + "vw";
      const y = (Math.random() * 100).toFixed(2) + "%";
      const alpha = (0.28 + Math.random() * 0.47).toFixed(2);
      const size = (0.5 + Math.random() * 0.58).toFixed(2) + "rem";
      const speed = (1.8 + Math.random() * 5.2).toFixed(2) + "s";
      const delay = (-Math.random() * 7).toFixed(2) + "s";

      star.textContent = glyphs[Math.floor(Math.random() * glyphs.length)];
      star.style.setProperty("--bp-star-x", x);
      star.style.setProperty("--bp-star-y", y);
      star.style.setProperty("--bp-star-alpha", alpha);
      star.style.setProperty("--bp-star-size", size);
      star.style.setProperty("--bp-star-speed", speed);
      star.style.setProperty("--bp-star-delay", delay);
      field.appendChild(star);
    }

    document.body.prepend(field);
  }

  function boot() {
    createStarfield();
    updateStatusBar();
    window.setInterval(updateStatusBar, 1000);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
