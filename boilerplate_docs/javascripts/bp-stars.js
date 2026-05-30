(function () {
  "use strict";

  var canvas = null;
  var ctx = null;
  var stars = [];
  var raf = null;
  var reduced = false;

  function prefersReducedMotion() {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  function rand(min, max) {
    return min + Math.random() * (max - min);
  }

  function buildStars(count, w, h) {
    var out = [];
    for (var i = 0; i < count; i++) {
      out.push({
        x: Math.random() * w,
        y: Math.random() * h,
        r: Math.random() < 0.08 ? rand(1.2, 2) : rand(0.4, 1),
        base: rand(0.15, 0.65),
        twinkle: rand(0.002, 0.012),
        phase: rand(0, Math.PI * 2),
      });
    }
    return out;
  }

  function resize() {
    if (!canvas) return;
    var dpr = Math.min(window.devicePixelRatio || 1, 2);
    var w = window.innerWidth;
    var h = window.innerHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = w + "px";
    canvas.style.height = h + "px";
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    stars = buildStars(Math.floor((w * h) / 9000), w, h);
  }

  function draw(t) {
    if (!ctx || !canvas) return;
    var w = canvas.width / (window.devicePixelRatio || 1);
    var h = canvas.height / (window.devicePixelRatio || 1);
    ctx.clearRect(0, 0, w, h);

    /* faint nebula wash */
    var grd = ctx.createRadialGradient(w * 0.5, h * 0.08, 0, w * 0.5, h * 0.08, w * 0.55);
    grd.addColorStop(0, "rgba(56, 139, 253, 0.07)");
    grd.addColorStop(0.45, "rgba(163, 113, 247, 0.04)");
    grd.addColorStop(1, "transparent");
    ctx.fillStyle = grd;
    ctx.fillRect(0, 0, w, h);

    for (var i = 0; i < stars.length; i++) {
      var s = stars[i];
      var alpha = reduced
        ? s.base
        : s.base + Math.sin(t * s.twinkle + s.phase) * 0.25;
      ctx.beginPath();
      ctx.arc(s.x, s.y, s.r, 0, Math.PI * 2);
      ctx.fillStyle = "rgba(230, 237, 243, " + Math.max(0.08, Math.min(1, alpha)) + ")";
      ctx.fill();
    }
    raf = requestAnimationFrame(draw);
  }

  function mount() {
    if (document.getElementById("bp-starfield")) return;
    reduced = prefersReducedMotion();
    canvas = document.createElement("canvas");
    canvas.id = "bp-starfield";
    canvas.setAttribute("aria-hidden", "true");
    document.body.prepend(canvas);
    ctx = canvas.getContext("2d");
    resize();
    window.addEventListener("resize", resize);
    if (reduced) {
      draw(0);
    } else {
      raf = requestAnimationFrame(draw);
    }
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

  onReady(mount);
})();
