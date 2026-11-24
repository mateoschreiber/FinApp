// Perceptual Theme Studio engine: OKLCH -> in-gamut sRGB derivation + WCAG contrast checks.

export const THEME_CONFIG_KEY = 'misfinanzas:theme-config';
export const THEME_VARS_KEY = 'misfinanzas:theme-vars';

export const PRESETS = [
  {id: 'emerald', name: 'Esmeralda', subtitle: 'Private Wealth', accent: '#143729'},
  {id: 'sapphire', name: 'Zafiro', subtitle: 'Banca Corporativa', accent: '#153b6e'},
  {id: 'indigo', name: 'Índigo', subtitle: 'Fintech Moderna', accent: '#3730a3'},
  {id: 'amber', name: 'Ámbar Oro', subtitle: 'Patrimonio Clásico', accent: '#8c4a12'},
  {id: 'burgundy', name: 'Borgoña', subtitle: 'Club Privado', accent: '#741c34'},
  {id: 'graphite', name: 'Grafito', subtitle: 'Monocromo', accent: '#2d383d'},
];

export const DEFAULT_THEME = Object.freeze({
  mode: 'light',
  preset: 'emerald',
  accent: '#143729',
  tint: 'subtle',
  surface: 'glass',
  radius: 'balanced',
  font: 'editorial',
  density: 'comfortable',
});

const VALID = {
  mode: new Set(['light', 'dark', 'auto']),
  preset: new Set([...PRESETS.map(p => p.id), 'custom']),
  tint: new Set(['neutral', 'subtle', 'rich']),
  surface: new Set(['glass', 'solid', 'editorial']),
  radius: new Set(['sharp', 'balanced', 'soft']),
  font: new Set(['editorial', 'geometric']),
  density: new Set(['comfortable', 'compact']),
};

export function normalizeHex(raw, fallback = DEFAULT_THEME.accent) {
  const s = String(raw ?? '').trim().toLowerCase();
  if (/^#[0-9a-f]{6}$/.test(s)) return s;
  if (/^[0-9a-f]{6}$/.test(s)) return '#' + s;
  return fallback;
}

export function normalizeThemeConfig(raw) {
  const src = raw && typeof raw === 'object' ? raw : {};
  const preset = VALID.preset.has(src.preset) ? src.preset : DEFAULT_THEME.preset;
  const presetObj = PRESETS.find(p => p.id === preset);
  const accent = preset === 'custom'
    ? normalizeHex(src.accent, DEFAULT_THEME.accent)
    : presetObj ? presetObj.accent : normalizeHex(src.accent, DEFAULT_THEME.accent);
  return {
    mode: VALID.mode.has(src.mode) ? src.mode : DEFAULT_THEME.mode,
    preset,
    accent,
    tint: VALID.tint.has(src.tint) ? src.tint : DEFAULT_THEME.tint,
    surface: VALID.surface.has(src.surface) ? src.surface : DEFAULT_THEME.surface,
    radius: VALID.radius.has(src.radius) ? src.radius : DEFAULT_THEME.radius,
    font: VALID.font.has(src.font) ? src.font : DEFAULT_THEME.font,
    density: VALID.density.has(src.density) ? src.density : DEFAULT_THEME.density,
  };
}

// --- sRGB <-> Linear <-> OKLab / OKLCH math ---
const srgbToLin = c => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
const linToSrgb = c => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(Math.max(0, c), 1 / 2.4) - 0.055);
const clamp01 = v => Math.max(0, Math.min(1, v));

function hexToRgb(hex) {
  const clean = normalizeHex(hex).slice(1);
  return [
    parseInt(clean.slice(0, 2), 16) / 255,
    parseInt(clean.slice(2, 4), 16) / 255,
    parseInt(clean.slice(4, 6), 16) / 255,
  ];
}

function rgbToHex(r, g, b) {
  const toHex = v => Math.round(clamp01(v) * 255).toString(16).padStart(2, '0');
  return '#' + toHex(r) + toHex(g) + toHex(b);
}

function oklchToLinearRgb(L, C, H) {
  const rad = (H * Math.PI) / 180;
  const a = C * Math.cos(rad);
  const b = C * Math.sin(rad);
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.2914855480 * b;
  const l = l_ * l_ * l_;
  const m = m_ * m_ * m_;
  const s = s_ * s_ * s_;
  return [
    +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
  ];
}

const inGamut = ([r, g, b]) => r >= -0.0005 && r <= 1.0005 && g >= -0.0005 && g <= 1.0005 && b >= -0.0005 && b <= 1.0005;

export function hexToOklch(hex) {
  const [r, g, b] = hexToRgb(hex).map(srgbToLin);
  const l_ = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m_ = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s_ = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
  const a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
  const bk = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;
  const C = Math.hypot(a, bk);
  const H = C < 1e-5 ? 160 : ((Math.atan2(bk, a) * 180) / Math.PI + 360) % 360;
  return {L, C, H};
}

export function oklchToHex(L, C, H) {
  let lin = oklchToLinearRgb(L, C, H);
  if (!inGamut(lin)) {
    let lo = 0, hi = C;
    for (let i = 0; i < 12; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(oklchToLinearRgb(L, mid, H))) lo = mid;
      else hi = mid;
    }
    lin = oklchToLinearRgb(L, lo, H);
  }
  return rgbToHex(linToSrgb(lin[0]), linToSrgb(lin[1]), linToSrgb(lin[2]));
}

function relativeLuminance(hex) {
  const [r, g, b] = hexToRgb(hex).map(srgbToLin);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(hexA, hexB) {
  const a = relativeLuminance(hexA);
  const b = relativeLuminance(hexB);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function rgbTriplet(hex) {
  const [r, g, b] = hexToRgb(hex);
  return `${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)}`;
}

export function isSystemDark() {
  return Boolean(globalThis.matchMedia?.('(prefers-color-scheme: dark)').matches);
}

export function resolveDarkMode(mode) {
  return mode === 'dark' || (mode === 'auto' && isSystemDark());
}

export function harmoniousRandomHex() {
  const h = Math.floor(Math.random() * 360);
  const c = 0.08 + Math.random() * 0.09;
  const l = 0.34 + Math.random() * 0.08;
  return oklchToHex(l, c, h);
}

export function buildThemeTokens(rawConfig) {
  const config = normalizeThemeConfig(rawConfig);
  const dark = resolveDarkMode(config.mode);
  const seed = hexToOklch(config.accent);
  const H = seed.H;
  const isMono = seed.C < 0.022;
  const baseC = isMono ? seed.C * 0.6 : Math.max(0.055, Math.min(0.165, seed.C * 1.08));
  const accentC = isMono ? seed.C * 0.7 : Math.max(0.09, Math.min(0.185, seed.C * 1.28));
  const tintScale = config.tint === 'neutral' ? 0 : config.tint === 'rich' ? 1.85 : 1;
  const surfC = isMono ? 0.004 * tintScale : Math.min(0.026, baseC * 0.14 * tintScale);

  // Adaptive Light & Dark brand colors guaranteed to pass WCAG AA (>= 4.5:1)
  let brandLight = oklchToHex(0.36, baseC, H);
  for (let L = 0.36; L >= 0.22 && contrastRatio(brandLight, '#ffffff') < 5.2; L -= 0.02) {
    brandLight = oklchToHex(L, baseC, H);
  }
  const brandHoverLight = oklchToHex(Math.max(0.18, hexToOklch(brandLight).L - 0.06), baseC, H);
  const brandMidLight = oklchToHex(Math.min(0.48, hexToOklch(brandLight).L + 0.09), Math.min(0.18, baseC * 1.15), H);
  const vibrantLight = oklchToHex(0.58, accentC, H);

  let brandDark = oklchToHex(0.79, Math.min(0.16, accentC), H);
  const darkSurfaceRef = oklchToHex(0.20, surfC * 0.9, H);
  for (let L = 0.79; L <= 0.88 && contrastRatio(brandDark, darkSurfaceRef) < 6.5; L += 0.02) {
    brandDark = oklchToHex(L, Math.min(0.16, accentC), H);
  }
  const brandHoverDark = oklchToHex(Math.min(0.92, hexToOklch(brandDark).L + 0.07), Math.min(0.15, accentC), H);
  const brandMidDark = oklchToHex(0.67, Math.min(0.16, accentC), H);

  const brand = dark ? brandDark : brandLight;
  const onBrandWhite = contrastRatio(brand, '#ffffff');
  const onBrandDark = contrastRatio(brand, '#07140f');
  const onBrand = onBrandWhite >= onBrandDark ? '#ffffff' : '#07140f';
  const brandContrast = Math.max(onBrandWhite, onBrandDark);

  const canvas = dark
    ? oklchToHex(config.tint === 'rich' ? 0.15 : 0.14, surfC * 1.1, H)
    : oklchToHex(config.tint === 'rich' ? 0.962 : 0.975, surfC * 0.85, H);
  const surface = dark
    ? oklchToHex(config.tint === 'rich' ? 0.21 : 0.195, surfC * 1.15, H)
    : oklchToHex(config.tint === 'rich' ? 0.992 : 1.0, surfC * 0.22, H);
  const surfaceSubtle = dark
    ? oklchToHex(0.245, surfC * 1.35, H)
    : oklchToHex(0.952, surfC * 1.1, H);
  const ink = dark
    ? oklchToHex(0.955, surfC * 0.35, H)
    : oklchToHex(0.18, surfC * 0.8, H);
  const muted = dark
    ? oklchToHex(0.74, surfC * 0.7, H)
    : oklchToHex(0.46, surfC * 0.8, H);
  const line = dark
    ? oklchToHex(0.285, surfC * 1.2, H)
    : oklchToHex(0.905, surfC * 1.1, H);
  const lineStrong = dark
    ? oklchToHex(0.365, surfC * 1.3, H)
    : oklchToHex(0.835, surfC * 1.25, H);
  const controlLine = dark
    ? oklchToHex(0.47, surfC * 1.2, H)
    : oklchToHex(0.71, surfC * 1.1, H);

  const brandTint = dark
    ? oklchToHex(0.25, Math.min(0.055, baseC * 0.45), H)
    : oklchToHex(0.952, Math.min(0.036, baseC * 0.28), H);
  const brandTintBorder = dark
    ? oklchToHex(0.36, Math.min(0.075, baseC * 0.6), H)
    : oklchToHex(0.865, Math.min(0.055, baseC * 0.45), H);

  // Vault Hero & Metallic Credit Card harmonized gradients
  const vaultA = oklchToHex(0.20, Math.min(0.07, baseC * 0.65), H);
  const vaultB = oklchToHex(0.31, Math.min(0.095, baseC * 0.88), H);
  const vaultC = oklchToHex(0.23, Math.min(0.075, baseC * 0.72), H);
  const cardA = oklchToHex(0.23, Math.min(0.045, baseC * 0.42), H);
  const cardB = oklchToHex(0.16, Math.min(0.035, baseC * 0.32), H);
  const cardC = oklchToHex(0.28, Math.min(0.055, baseC * 0.52), H);
  const sparkStroke = oklchToHex(0.84, Math.min(0.13, accentC), H);
  const sparkFill = oklchToHex(0.72, Math.min(0.14, accentC), H);
  const vaultLabel = oklchToHex(0.83, Math.min(0.05, baseC * 0.4), H);

  const glassAlpha = config.surface === 'solid' ? 1 : config.surface === 'editorial' ? 0.96 : dark ? 0.84 : 0.82;
  const surfaceGlass = `rgb(${rgbTriplet(surface)} / ${glassAlpha})`;
  const auroraAlpha1 = config.surface === 'glass' ? (dark ? 0.16 : 0.11) : config.surface === 'editorial' ? 0.05 : 0;
  const auroraAlpha2 = config.surface === 'glass' ? (dark ? 0.10 : 0.07) : 0;
  const aurora = auroraAlpha1 > 0
    ? `radial-gradient(62rem 36rem at 10% -6%, rgb(${rgbTriplet(vibrantLight)} / ${auroraAlpha1}), transparent 60%), radial-gradient(48rem 32rem at 92% 4%, rgb(${rgbTriplet(brandLight)} / ${auroraAlpha2}), transparent 58%)`
    : 'none';

  // Harmonized 6-stop Donut chart palette seeded from user's hue
  const chartHueStops = [0, 42, 195, 275, 335, 130];
  const chartColors = chartHueStops.map((offset, idx) => {
    const ch = (H + offset) % 360;
    const c = idx === 0 && isMono ? 0.04 : idx === 5 ? 0.05 : 0.145;
    const l = dark ? 0.73 : 0.62;
    return oklchToHex(l, c, ch);
  });

  const vars = {
    '--canvas': canvas,
    '--surface': surface,
    '--surface-glass': surfaceGlass,
    '--surface-subtle': surfaceSubtle,
    '--surface-elevated': surface,
    '--ink': ink,
    '--muted': muted,
    '--line': line,
    '--line-strong': lineStrong,
    '--control-line': controlLine,
    '--forest': brandLight,
    '--forest-deep': brandHoverLight,
    '--emerald': dark ? brandDark : vibrantLight,
    '--emerald-soft': sparkStroke,
    '--brand': brand,
    '--brand-hover': dark ? brandHoverDark : brandHoverLight,
    '--brand-mid': dark ? brandMidDark : brandMidLight,
    '--brand-tint': brandTint,
    '--brand-tint-border': brandTintBorder,
    '--on-brand': onBrand,
    '--focus': dark ? brandHoverDark : brandLight,
    '--focus-ring': `rgb(${rgbTriplet(brand)} / ${dark ? 0.26 : 0.18})`,
    '--spotlight': `rgb(${rgbTriplet(dark ? brandDark : vibrantLight)} / ${dark ? 0.14 : 0.095})`,
    '--aurora': aurora,
    '--brand-gradient': dark
      ? `linear-gradient(135deg, ${brandMidDark} 0%, ${brandDark} 100%)`
      : `linear-gradient(135deg, ${brandLight} 0%, ${brandMidLight} 100%)`,
    '--vault-bg': `linear-gradient(142deg, ${vaultA} 0%, ${vaultB} 54%, ${vaultC} 100%)`,
    '--vault-border': `rgb(${rgbTriplet(sparkStroke)} / 0.28)`,
    '--vault-label': vaultLabel,
    '--vault-pill-bg': `rgb(${rgbTriplet(sparkStroke)} / 0.16)`,
    '--vault-pill-border': `rgb(${rgbTriplet(sparkStroke)} / 0.34)`,
    '--vault-pill-ink': oklchToHex(0.91, Math.min(0.08, accentC * 0.6), H),
    '--card-metallic-bg': `linear-gradient(135deg, ${cardA} 0%, ${cardB} 58%, ${cardC} 100%)`,
    '--spark-stroke': sparkStroke,
    '--spark-fill': sparkFill,
    '--chart-1': chartColors[0],
    '--chart-2': chartColors[1],
    '--chart-3': chartColors[2],
    '--chart-4': chartColors[3],
    '--chart-5': chartColors[4],
    '--chart-6': chartColors[5],
  };

  return {
    config,
    dark,
    vars,
    metrics: {
      brand,
      onBrand,
      brandContrast: Number(brandContrast.toFixed(2)),
      surfaceContrast: Number(contrastRatio(ink, surface).toFixed(2)),
      grade: brandContrast >= 7 ? 'AAA' : 'AA',
    },
  };
}

export function loadStoredThemeConfig() {
  try {
    const raw = localStorage.getItem(THEME_CONFIG_KEY);
    if (raw) return normalizeThemeConfig(JSON.parse(raw));
    const legacy = localStorage.getItem('misfinanzas:theme');
    if (legacy) return normalizeThemeConfig({...DEFAULT_THEME, mode: legacy});
  } catch {}
  return normalizeThemeConfig(DEFAULT_THEME);
}

export function applyThemeConfig(rawConfig, {persist = true} = {}) {
  const result = buildThemeTokens(rawConfig);
  const {config, dark, vars} = result;
  const root = document.documentElement;

  // Map meta color-scheme per modern-web-guidance (dark-mode-toggle)
  const metaScheme = document.querySelector('meta[name="color-scheme"]');
  if (metaScheme) metaScheme.content = config.mode === 'auto' ? 'light dark' : config.mode;
  const metaTheme = document.querySelector('meta[name="theme-color"]:not([media])');
  if (metaTheme) metaTheme.content = vars['--canvas'];

  root.dataset.theme = config.mode;
  root.dataset.resolvedTheme = dark ? 'dark' : 'light';
  root.dataset.surface = config.surface;
  root.dataset.radius = config.radius;
  root.dataset.font = config.font;
  root.dataset.density = config.density;

  for (const [key, value] of Object.entries(vars)) {
    root.style.setProperty(key, value);
  }

  if (persist) {
    try {
      localStorage.setItem(THEME_CONFIG_KEY, JSON.stringify(config));
      localStorage.setItem(THEME_VARS_KEY, JSON.stringify({
        mode: config.mode,
        dark,
        surface: config.surface,
        radius: config.radius,
        font: config.font,
        density: config.density,
        vars,
      }));
      localStorage.removeItem('misfinanzas:theme');
    } catch {}
  }
  return result;
}

export function watchThemeChanges(onExternalUpdate) {
  const mq = globalThis.matchMedia?.('(prefers-color-scheme: dark)');
  if (mq?.addEventListener) {
    mq.addEventListener('change', () => onExternalUpdate('system'));
  }
  window.addEventListener('storage', event => {
    if (event.key === THEME_CONFIG_KEY) onExternalUpdate('storage');
  });
}
