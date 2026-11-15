// Runs before first paint: applies a theme the visitor picked earlier. Without one, the page
// follows the system (prefers-color-scheme in site.css).
(function () {
  var t;
  try {
    t = localStorage.getItem("gg-theme");
  } catch (e) {}
  if (t === "light" || t === "dark") document.documentElement.setAttribute("data-theme", t);
})();
