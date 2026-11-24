// Synchronous early theme hydrator (self-hosted, CSP-compliant, prevents FOUC before first paint).
(function () {
  try {
    var raw = localStorage.getItem('misfinanzas:theme-vars');
    if (!raw) return;
    var snap = JSON.parse(raw);
    if (!snap || typeof snap !== 'object') return;
    var root = document.documentElement;
    var mode = snap.mode === 'dark' || snap.mode === 'light' || snap.mode === 'auto' ? snap.mode : 'light';
    var meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) meta.content = mode === 'auto' ? 'light dark' : mode;
    root.dataset.theme = mode;
    var sysDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
    var isDark = mode === 'dark' || (mode === 'auto' && sysDark);
    root.dataset.resolvedTheme = isDark ? 'dark' : 'light';
    if (snap.surface) root.dataset.surface = snap.surface;
    if (snap.radius) root.dataset.radius = snap.radius;
    if (snap.font) root.dataset.font = snap.font;
    if (snap.density) root.dataset.density = snap.density;
    // Apply cached vars only when resolved dark mode matches the cached snapshot.
    if (snap.vars && typeof snap.vars === 'object' && Boolean(snap.dark) === isDark) {
      var keys = Object.keys(snap.vars);
      for (var i = 0; i < keys.length; i++) {
        root.style.setProperty(keys[i], snap.vars[keys[i]]);
      }
    }
  } catch (e) {}
})();
