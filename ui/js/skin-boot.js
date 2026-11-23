// Runs before the first paint so a saved theme does not flash the default one.
(function () {
  try {
    if (localStorage.getItem('linger.skin') === 'fresh') document.documentElement.dataset.skin = 'fresh';
  } catch (_) { /* storage unavailable */ }
})();
