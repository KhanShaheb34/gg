// The theme toggle, theme-matched screenshots, the release details from /latest.json, and the
// copy button. The page works without any of it.
(function () {
  "use strict";

  var root = document.documentElement;
  var systemDark = window.matchMedia("(prefers-color-scheme: dark)");

  function theme() {
    var t = root.getAttribute("data-theme");
    if (t === "light" || t === "dark") return t;
    return systemDark.matches ? "dark" : "light";
  }

  // Each <source data-scheme> applies only in its theme; data-base keeps any other condition
  // (such as a narrow screen). Without this script the media queries follow the system.
  function applyPictures() {
    var t = theme();
    var sources = document.querySelectorAll("source[data-scheme]");
    for (var i = 0; i < sources.length; i++) {
      var s = sources[i];
      s.media = s.getAttribute("data-scheme") === t ? s.getAttribute("data-base") || "all" : "not all";
    }
  }

  var toggle = document.querySelector(".theme-toggle");

  function applyLabel() {
    if (toggle) toggle.setAttribute("aria-label", theme() === "dark" ? "Switch to light theme" : "Switch to dark theme");
  }

  function store(value) {
    try {
      if (value) localStorage.setItem("gg-theme", value);
      else localStorage.removeItem("gg-theme");
    } catch (e) {}
  }

  if (toggle) {
    toggle.addEventListener("click", function () {
      var next = theme() === "dark" ? "light" : "dark";
      // Picking what the system already shows goes back to following the system.
      if (next === (systemDark.matches ? "dark" : "light")) {
        root.removeAttribute("data-theme");
        store(null);
      } else {
        root.setAttribute("data-theme", next);
        store(next);
      }
      applyPictures();
      applyLabel();
    });
  }

  systemDark.addEventListener("change", function () {
    applyPictures();
    applyLabel();
  });

  applyPictures();
  applyLabel();

  // Release details. A release script rewrites /latest.json; the HTML holds the same values as
  // a fallback.
  function setAll(name, fn) {
    var els = document.querySelectorAll('[data-latest="' + name + '"]');
    for (var i = 0; i < els.length; i++) fn(els[i]);
  }

  fetch("/latest.json", { cache: "no-cache" })
    .then(function (r) {
      return r.ok ? r.json() : null;
    })
    .then(function (d) {
      if (!d) return;
      if (typeof d.version === "string" && d.version) {
        setAll("version", function (el) {
          el.textContent = d.version;
        });
      }
      if (typeof d.file === "string" && /^[\w.-]+$/.test(d.file)) {
        var link = document.getElementById("download");
        if (link) link.setAttribute("href", "/download/" + d.file);
      }
      if (typeof d.sizeMb === "number" && d.sizeMb > 0) {
        setAll("size", function (el) {
          el.textContent = (Math.round(d.sizeMb * 10) / 10).toString() + " MB";
          el.hidden = false;
        });
      }
      if (typeof d.sha256 === "string" && /^[0-9a-f]{64}$/i.test(d.sha256)) {
        setAll("sha256", function (el) {
          el.textContent = d.sha256.toLowerCase();
        });
        setAll("sha", function (el) {
          el.hidden = false;
        });
      }
    })
    .catch(function () {});

  // Copy buttons: data-copy names the element whose text is copied.
  document.addEventListener("click", function (e) {
    var btn = e.target.closest && e.target.closest("button[data-copy]");
    if (!btn || !navigator.clipboard) return;
    var src = document.getElementById(btn.getAttribute("data-copy"));
    if (!src) return;
    navigator.clipboard.writeText(src.textContent).then(function () {
      btn.textContent = "Copied";
      setTimeout(function () {
        btn.textContent = "Copy";
      }, 1600);
    });
  });
})();
