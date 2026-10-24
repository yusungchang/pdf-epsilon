// Runs before the page is drawn: a saved Light or Dark choice applies at once, without a
// flash of the other theme. app.js does the rest (and System, the default, needs nothing).
try {
  const theme = JSON.parse(localStorage.getItem("theme"));
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
} catch { /* storage unavailable */ }
