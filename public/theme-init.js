// Runs before first paint: applies the cached theme setting so the window never flashes
// the wrong theme. Mirrors resolveTheme in src/lib/theme.ts. A plain file (not inline) to
// satisfy the app's Content-Security-Policy.
(function () {
  var setting = null;
  try {
    setting = localStorage.getItem("gg-theme");
  } catch (e) {}
  var dark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme =
    setting === "light" || setting === "dark" ? setting : dark ? "dark" : "light";
})();
