//! WP-607: upstream theme definitions, user themes, and ratatui styles.
//!
//! The token table, color grammar (`#hex`, `okhsl(...)`, `oklch(...)`,
//! ANSI index, `""` = terminal default), variable references, and token
//! fallbacks mirror the pinned upstream (`modes/interactive/theme/`).
//! The built-in `dark`/`light` JSONs are copies of the pinned-upstream
//! files. User themes live in `<agentDir>/themes/<name>.json`
//! (upstream `getCustomThemesDir`). The upstream "system" theme (generated
//! from the terminal's reported colors) has no v0 equivalent: the default
//! theme is `dark`, and a `light/dark` auto setting takes its dark side.
//!
//! OKLab/OKHSL conversion is a port of upstream `packages/tui/src/oklab.ts`
//! (Björn Ottosson's reference implementation, MIT); golden RGB vectors are
//! generated from the upstream functions at the pinned commit.
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

// ============================================================================
// OKLab / OKHSL <-> sRGB (port of upstream oklab.ts)
// ============================================================================

const LAB_TO_LMS: [[f64; 3]; 3] = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];
const LMS_TO_LINEAR_SRGB: [[f64; 3]; 3] = [
    [
        4.076_741_636_075_958,
        -3.307_711_539_258_063,
        0.2309699031821043,
    ],
    [
        -1.2684379732850315,
        2.609_757_349_287_688,
        -0.341319376002657,
    ],
    [-0.0041960761386756, -0.7034186179359362, 1.7076146940746117],
];
const K1: f64 = 0.206;
const K2: f64 = 0.03;
const K3: f64 = (1.0 + K1) / (1.0 + K2);

// (half-plane (x, y), polynomial coefficients) per channel.
const SAT_FIT: [((f64, f64), [f64; 5]); 3] = [
    (
        (-1.8817031, -0.80936501),
        [1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245],
    ),
    (
        (1.8144408, -1.19445267),
        [0.73956515, -0.45954404, 0.08285427, 0.12541073, -0.14503204],
    ),
    (
        (0.13110758, 1.81333971),
        [1.35733652, -0.00915799, -1.1513021, -0.50559606, 0.00692167],
    ),
];

fn mat_vec(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn linear_to_srgb(value: f64) -> f64 {
    if value > 0.0031308 {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * value
    }
}

fn oklab_to_linear_srgb(lab: [f64; 3]) -> [f64; 3] {
    let lms = mat_vec(&LAB_TO_LMS, lab);
    let lms3 = [lms[0].powi(3), lms[1].powi(3), lms[2].powi(3)];
    mat_vec(&LMS_TO_LINEAR_SRGB, lms3)
}

fn linear_srgb_to_rgb(linear: [f64; 3]) -> (u8, u8, u8) {
    let ch = |v: f64| {
        let c = linear_to_srgb(v.clamp(0.0, 1.0));
        (c * 255.0).round() as u8
    };
    (ch(linear[0]), ch(linear[1]), ch(linear[2]))
}

fn okhsl_to_oklab_lightness(x: f64) -> f64 {
    (x * x + K1 * x) / (K3 * (x + K2))
}

fn lms_slopes(a: f64, b: f64) -> [f64; 3] {
    [
        LAB_TO_LMS[0][1] * a + LAB_TO_LMS[0][2] * b,
        LAB_TO_LMS[1][1] * a + LAB_TO_LMS[1][2] * b,
        LAB_TO_LMS[2][1] * a + LAB_TO_LMS[2][2] * b,
    ]
}

fn max_saturation(a: f64, b: f64) -> f64 {
    // Upstream findIndex: the first channel whose half-plane test holds.
    let mut channel = 0;
    for (i, sat) in SAT_FIT.iter().enumerate() {
        let (x, y) = sat.0;
        if i == 2 || x * a + y * b > 1.0 {
            channel = i;
            break;
        }
    }
    let [k0, k1, k2, k3, k4] = SAT_FIT[channel].1;
    let weights = LMS_TO_LINEAR_SRGB[channel];
    let saturation = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;
    let slopes = lms_slopes(a, b);
    let base: [f64; 3] = [
        1.0 + saturation * slopes[0],
        1.0 + saturation * slopes[1],
        1.0 + saturation * slopes[2],
    ];
    let dot = |row: &[f64; 3], values: [f64; 3]| {
        row[0] * values[0] + row[1] * values[1] + row[2] * values[2]
    };
    let f = dot(
        &weights,
        [base[0].powi(3), base[1].powi(3), base[2].powi(3)],
    );
    let f1 = dot(
        &weights,
        [
            3.0 * slopes[0] * base[0].powi(2),
            3.0 * slopes[1] * base[1].powi(2),
            3.0 * slopes[2] * base[2].powi(2),
        ],
    );
    let f2 = dot(
        &weights,
        [
            6.0 * slopes[0] * slopes[0] * base[0],
            6.0 * slopes[1] * slopes[1] * base[1],
            6.0 * slopes[2] * slopes[2] * base[2],
        ],
    );
    saturation - (f * f1) / (f1 * f1 - 0.5 * f * f2)
}

fn cusp(a: f64, b: f64) -> (f64, f64) {
    let saturation = max_saturation(a, b);
    let linear = oklab_to_linear_srgb([1.0, saturation * a, saturation * b]);
    let lightness = (1.0 / linear.iter().cloned().fold(f64::NEG_INFINITY, f64::max)).cbrt();
    (lightness, lightness * saturation)
}

fn max_chroma(a: f64, b: f64, lightness: f64, cusp_l: f64, cusp_c: f64) -> f64 {
    if lightness <= cusp_l {
        return (cusp_c * lightness) / cusp_l;
    }
    let t = (cusp_c * (lightness - 1.0)) / (cusp_l - 1.0);
    let slopes = lms_slopes(a, b);
    let lms: [f64; 3] = [
        lightness + t * slopes[0],
        lightness + t * slopes[1],
        lightness + t * slopes[2],
    ];
    let cubes = [lms[0].powi(3), lms[1].powi(3), lms[2].powi(3)];
    let first = [
        3.0 * slopes[0] * lms[0].powi(2),
        3.0 * slopes[1] * lms[1].powi(2),
        3.0 * slopes[2] * lms[2].powi(2),
    ];
    let second = [
        6.0 * slopes[0].powi(2) * lms[0],
        6.0 * slopes[1].powi(2) * lms[1],
        6.0 * slopes[2].powi(2) * lms[2],
    ];
    let dot = |row: &[f64; 3], values: [f64; 3]| {
        row[0] * values[0] + row[1] * values[1] + row[2] * values[2]
    };
    let mut min_step = f64::MAX;
    for row in LMS_TO_LINEAR_SRGB.iter() {
        let f = dot(row, cubes) - 1.0;
        let f1 = dot(row, first);
        let f2 = dot(row, second);
        let u = f1 / (f1 * f1 - 0.5 * f * f2);
        let step = if u >= 0.0 { -f * u } else { f64::MAX };
        min_step = min_step.min(step);
    }
    t + min_step
}

fn chroma_stops(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let (peak_l, peak_c) = cusp(a, b);
    let c_max = max_chroma(a, b, l, peak_l, peak_c);
    let k = c_max / (l * (peak_c / peak_l)).min((1.0 - l) * (peak_c / (1.0 - peak_l)));
    let mid_s = 0.11516993
        + 1.0
            / (7.4477897
                + 4.1590124 * b
                + a * (-2.19557347
                    + 1.75198401 * b
                    + a * (-2.13704948 - 10.02301043 * b
                        + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let mid_t = 0.11239642
        + 1.0
            / (1.6132032 - 0.68124379 * b
                + a * (0.40370612
                    + 0.90148123 * b
                    + a * (-0.27087943
                        + 0.6122399 * b
                        + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    let c_mid = 0.9
        * k
        * (1.0 / (1.0 / (l * mid_s).powi(4) + 1.0 / ((1.0 - l) * mid_t).powi(4)))
            .sqrt()
            .sqrt();
    let c0 = (1.0 / (1.0 / (l * 0.4).powi(2) + 1.0 / ((1.0 - l) * 0.8).powi(2))).sqrt();
    (c0, c_mid, c_max)
}

/// Upstream `okhslToRgb`: hue in degrees, saturation/lightness 0..=1,
/// out-of-gamut channels clipped, rounded to 0-255.
pub fn okhsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let l = okhsl_to_oklab_lightness(lightness);
    let lab = if l > 0.0 && l < 1.0 && saturation > 0.0 {
        let angle = 2.0 * std::f64::consts::PI * (((hue % 360.0) + 360.0) % 360.0) / 360.0;
        let a = angle.cos();
        let b = angle.sin();
        let (c0, c_mid, c_max) = chroma_stops(l, a, b);
        let chroma = if saturation < 0.8 {
            let t = 1.25 * saturation;
            let k1 = 0.8 * c0;
            (t * k1) / (1.0 - (1.0 - k1 / c_mid) * t)
        } else {
            let t = 5.0 * (saturation - 0.8);
            let k1 = (0.2 * c_mid * c_mid * 1.25 * 1.25) / c0;
            c_mid + (t * k1) / (1.0 - (1.0 - k1 / (c_max - c_mid)) * t)
        };
        [l, chroma * a, chroma * b]
    } else {
        [l, 0.0, 0.0]
    };
    linear_srgb_to_rgb(oklab_to_linear_srgb(lab))
}

/// Upstream `oklchColor` -> `oklabToLinearSrgb`: lightness/chroma 0.., hue degrees.
pub fn oklch_to_rgb(l: f64, c: f64, hue: f64) -> (u8, u8, u8) {
    let angle = 2.0 * std::f64::consts::PI * (((hue % 360.0) + 360.0) % 360.0) / 360.0;
    linear_srgb_to_rgb(oklab_to_linear_srgb([l, c * angle.cos(), c * angle.sin()]))
}

// ============================================================================
// Color grammar (upstream `parseColor`)
// ============================================================================

/// Parses one color value: `""` (terminal default), `#rgb`/`#rrggbb`,
/// `okhsl(H S% L%)`, `oklch(L C H)`, or an ANSI index (0-255).
pub fn parse_color(value: &str) -> Result<Color, String> {
    if value.is_empty() {
        return Ok(Color::Reset);
    }
    let trimmed = value.trim();
    if let Ok(n) = trimmed.parse::<u8>() {
        return Ok(Color::Indexed(n));
    }
    if trimmed.starts_with('#') {
        let digits = trimmed.strip_prefix('#').unwrap();
        let expand = |d: char| format!("{d}{d}");
        let digits = if digits.len() == 3 {
            digits.chars().map(expand).collect::<String>()
        } else if digits.len() == 6 {
            digits.to_string()
        } else {
            return Err(format!(
                "invalid hex color {value:?} (expected #rgb or #rrggbb)"
            ));
        };
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid hex color {value:?}"));
        }
        let r = u8::from_str_radix(&digits[0..2], 16).unwrap();
        let g = u8::from_str_radix(&digits[2..4], 16).unwrap();
        let b = u8::from_str_radix(&digits[4..6], 16).unwrap();
        return Ok(Color::Rgb(r, g, b));
    }
    if let Some(rest) = trimmed
        .strip_prefix("okhsl(")
        .and_then(|r| r.strip_suffix(')'))
    {
        // okhsl(H S% L%)
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(format!("invalid okhsl color {value:?}"));
        }
        let h = parts[0]
            .trim_end_matches("deg")
            .parse::<f64>()
            .map_err(|_| format!("invalid okhsl hue in {value:?}"))?;
        let s = parse_pct(parts[1])?;
        let l = parse_pct(parts[2])?;
        if s < 0.0 || s > 1.0 || l < 0.0 || l > 1.0 {
            return Err(format!(
                "okhsl saturation/lightness out of range in {value:?}"
            ));
        }
        let (r, g, b) = okhsl_to_rgb(h, s, l);
        return Ok(Color::Rgb(r, g, b));
    }
    if let Some(rest) = trimmed
        .strip_prefix("oklch(")
        .and_then(|r| r.strip_suffix(')'))
    {
        // oklch(L C H)
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(format!("invalid oklch color {value:?}"));
        }
        let l = parse_pct(parts[0])?;
        let c = parts[1]
            .parse::<f64>()
            .map_err(|_| format!("invalid oklch chroma in {value:?}"))?;
        let h = parts[2]
            .trim_end_matches("deg")
            .parse::<f64>()
            .map_err(|_| format!("invalid oklch hue in {value:?}"))?;
        if l < 0.0 || l > 1.0 || c < 0.0 {
            return Err(format!("oklch components out of range in {value:?}"));
        }
        let (r, g, b) = oklch_to_rgb(l, c, h);
        return Ok(Color::Rgb(r, g, b));
    }
    Err(format!(
		"invalid color value {trimmed:?} (expected \"\", #hex, okhsl(H S% L%), oklch(L C H), or an ANSI index 0-255)"
	))
}

fn parse_pct(s: &str) -> Result<f64, String> {
    let (num, pct) = if let Some(n) = s.strip_suffix('%') {
        (n, true)
    } else {
        (s, false)
    };
    let v = num
        .parse::<f64>()
        .map_err(|_| format!("invalid number {s:?}"))?;
    Ok(if pct { v / 100.0 } else { v })
}

// ============================================================================
// Theme tokens (upstream THEME_TOKENS: name -> slot, fallback)
// ============================================================================

/// Upstream token table: (name, slot, fallback). All 60 tokens.
pub const THEME_TOKENS: &[(&str, &str, Option<&str>)] = &[
    ("accent", "foreground", None),
    ("border", "foreground", None),
    ("borderAccent", "foreground", None),
    ("borderMuted", "foreground", None),
    ("success", "foreground", None),
    ("error", "foreground", None),
    ("warning", "foreground", None),
    ("muted", "foreground", None),
    ("dim", "foreground", None),
    ("text", "foreground", None),
    ("thinkingText", "foreground", None),
    ("scrollbarTrack", "foreground", Some("muted")),
    ("scrollbarThumb", "foreground", Some("text")),
    ("selectedBg", "background", None),
    ("searchMatchBg", "background", Some("selectedBg")),
    ("searchMatchText", "foreground", Some("text")),
    ("userMessageBg", "background", None),
    ("userMessageText", "foreground", None),
    ("customMessageBg", "background", None),
    ("customMessageText", "foreground", None),
    ("customMessageLabel", "foreground", None),
    ("toolPendingBg", "background", None),
    ("toolSuccessBg", "background", None),
    ("toolErrorBg", "background", None),
    ("toolTitle", "foreground", None),
    ("toolOutput", "foreground", None),
    ("mdHeading", "foreground", None),
    ("mdLink", "foreground", None),
    ("mdLinkUrl", "foreground", None),
    ("mdCode", "foreground", None),
    ("mdCodeBlock", "foreground", None),
    ("mdCodeBlockBorder", "foreground", None),
    ("mdQuote", "foreground", None),
    ("mdQuoteBorder", "foreground", None),
    ("mdHr", "foreground", None),
    ("mdListBullet", "foreground", None),
    ("toolDiffAdded", "foreground", None),
    ("toolDiffRemoved", "foreground", None),
    ("toolDiffContext", "foreground", None),
    ("syntaxComment", "foreground", None),
    ("syntaxKeyword", "foreground", None),
    ("syntaxFunction", "foreground", None),
    ("syntaxVariable", "foreground", None),
    ("syntaxString", "foreground", None),
    ("syntaxNumber", "foreground", None),
    ("syntaxType", "foreground", None),
    ("syntaxOperator", "foreground", None),
    ("syntaxPunctuation", "foreground", None),
    ("thinkingOff", "foreground", None),
    ("thinkingMinimal", "foreground", None),
    ("thinkingLow", "foreground", None),
    ("thinkingMedium", "foreground", None),
    ("thinkingHigh", "foreground", None),
    ("thinkingXhigh", "foreground", None),
    ("thinkingMax", "foreground", Some("thinkingXhigh")),
    ("bashMode", "foreground", None),
];

pub fn token_names() -> Vec<&'static str> {
    THEME_TOKENS.iter().map(|(n, _, _)| *n).collect()
}

// ============================================================================
// Theme
// ============================================================================

/// A loaded theme: every token resolved to a concrete ratatui color.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    /// Read by the in-crate theme tests (appearance round-trip).
    #[allow(dead_code)]
    pub appearance: Option<String>,
    /// token -> resolved color (all 60 tokens, fallbacks applied).
    pub colors: BTreeMap<String, Color>,
}

impl Theme {
    /// The token's color; `Color::Reset` for unknown tokens (render paths
    /// must never panic on a theme lookup).
    pub fn color(&self, token: &str) -> Color {
        self.colors.get(token).copied().unwrap_or(Color::Reset)
    }

    /// Foreground style for a token (the render hot path: one lookup).
    pub fn fg(&self, token: &str) -> Style {
        Style::default().fg(self.color(token))
    }

    /// Bold foreground style for a token.
    pub fn fg_bold(&self, token: &str) -> Style {
        Style::default()
            .fg(self.color(token))
            .add_modifier(Modifier::BOLD)
    }

    /// The built-in `dark` or `light` theme (copies of the pinned-upstream
    /// JSON files).
    pub fn builtin(name: &str) -> Result<Self, String> {
        let json = match name {
            "dark" => include_str!("themes/dark.json"),
            "light" => include_str!("themes/light.json"),
            other => {
                return Err(format!(
                    "unknown theme {other:?} (built-in themes: \"dark\", \"light\")"
                ))
            }
        };
        Self::from_json(name, json)
    }

    /// Parse a theme JSON document (upstream `createTheme`): the `colors`
    /// map is required, `vars` resolve name references (cycles and unknown
    /// vars are errors), unknown tokens are errors, missing required tokens
    /// are errors, and token fallbacks fill the optional ones.
    pub fn from_json(name: &str, content: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| format!("theme {name:?}: invalid JSON: {e}"))?;
        let obj = value.as_object().ok_or_else(|| {
            format!("theme {name:?}: expected a JSON object with a \"colors\" map")
        })?;
        let declared = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| name.to_string());
        let appearance = obj
            .get("appearance")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let vars: HashMap<String, serde_json::Value> = obj
            .get("vars")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let colors = obj
            .get("colors")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("theme {name:?}: missing the \"colors\" map"))?;

        let mut resolved: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for (token, raw) in colors {
            let known = THEME_TOKENS.iter().any(|(n, _, _)| *n == token);
            if !known {
                let valid = token_names().join(", ");
                return Err(format!(
                    "theme {name:?}: unknown token {token:?} (valid tokens: {valid})"
                ));
            }
            let value = resolve_var_refs(token, raw, &vars, name)?;
            resolved.insert(token.clone(), value);
        }
        // Required tokens must be present (upstream schema: fallback tokens
        // are optional, everything else is required).
        let mut missing: Vec<&str> = Vec::new();
        for (token, _, fallback) in THEME_TOKENS {
            if !resolved.contains_key(*token) && fallback.is_none() {
                missing.push(token);
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "theme {name:?}: missing tokens: {}",
                missing.join(", ")
            ));
        }
        // Token fallbacks (upstream withThemeTokenFallbacks).
        for (token, _, fallback) in THEME_TOKENS {
            if resolved.contains_key(*token) {
                continue;
            }
            let fb = fallback.unwrap();
            resolved.insert(token.to_string(), resolved[fb].clone());
        }
        let mut colors: BTreeMap<String, Color> = BTreeMap::new();
        for (token, value) in resolved {
            let color_value = match value {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                other => {
                    return Err(format!(
                        "theme {name:?}: token {token:?} must be a string or a number, got {other}"
                    ))
                }
            };
            let c = parse_color(&color_value)
                .map_err(|e| format!("theme {name:?}: token {token:?}: {e}"))?;
            colors.insert(token, c);
        }
        Ok(Self {
            name: declared,
            appearance,
            colors,
        })
    }

    /// Load by name: built-ins first, then `<agentDir>/themes/<name>.json`
    /// (upstream `getCustomThemesDir`).
    pub fn load(agent_dir: &Path, name: &str) -> Result<Self, String> {
        if let Ok(t) = Self::builtin(name) {
            return Ok(t);
        }
        let path = agent_dir.join("themes").join(format!("{name}.json"));
        if !path.exists() {
            let available = Self::available_themes(agent_dir).join(", ");
            return Err(format!(
                "theme not found: {name:?} (available: {available})"
            ));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading theme {}: {e}", path.display()))?;
        Self::from_json(name, &content)
    }

    /// Built-in + custom theme names (upstream `getAvailableThemes`).
    pub fn available_themes(agent_dir: &Path) -> Vec<String> {
        let mut names = vec!["dark".to_string(), "light".to_string()];
        let dir = agent_dir.join("themes");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(n) = entry
                    .file_name()
                    .to_str()
                    .and_then(|f| f.strip_suffix(".json"))
                {
                    if !names.contains(&n.to_string()) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        names
    }
}

/// Upstream `resolveVarRefs`: literals pass through, names resolve through
/// `vars` with cycle detection.
fn resolve_var_refs(
    token: &str,
    value: &serde_json::Value,
    vars: &HashMap<String, serde_json::Value>,
    theme_name: &str,
) -> Result<serde_json::Value, String> {
    let mut current = value.clone();
    let mut visited: Vec<String> = Vec::new();
    loop {
        let s = current.as_str().ok_or_else(|| {
            format!("theme {theme_name:?}: token {token:?} must be a string or a number")
        })?;
        if s.is_empty()
            || s.starts_with('#')
            || s.parse::<u8>().is_ok()
            || s.to_ascii_lowercase().starts_with("ok")
        {
            return Ok(serde_json::Value::String(s.to_string()));
        }
        if let Some(idx) = visited.iter().position(|v| v == s) {
            let mut chain = visited[idx..].join(" -> ");
            chain.push_str(&format!(" -> {s}"));
            return Err(format!(
                "theme {theme_name:?}: circular variable reference: {chain}"
            ));
        }
        visited.push(s.to_string());
        let next = vars.get(s).ok_or_else(|| {
            format!("theme {theme_name:?}: variable {s:?} not found (token {token:?})")
        })?;
        current = next.clone();
    }
}

/// The theme a `theme` settings value selects (upstream
/// `resolveThemeSetting`): a plain name passes through; a `light/dark`
/// auto pair takes the dark side in v0 (no terminal color query); anything
/// else with a slash is ignored (upstream returns undefined).
pub fn resolve_theme_setting(setting: Option<&str>) -> Option<String> {
    let s = setting?.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(slash) = s.find('/') {
        let light = s[..slash].trim();
        let dark = s[slash + 1..].trim();
        if light.is_empty() || dark.is_empty() || s[slash + 1..].contains('/') {
            return None;
        }
        // v0: no system theme / terminal query — the dark side wins.
        return Some(dark.to_string());
    }
    Some(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vectors generated from the pinned-upstream `okhslToRgb` /
    /// `oklabToLinearSrgb` (see /tmp/gen-vectors.mjs at WP-607 time).
    const VECTORS: &[(&str, &str)] = &[
        ("okhsl(295 50% 67%)", "167,152,215"),
        ("okhsl(231 57% 65%)", "95,168,204"),
        ("okhsl(295 53% 64%)", "160,142,213"),
        ("okhsl(229 8% 53%)", "118,129,134"),
        ("okhsl(159 59% 67%)", "104,183,141"),
        ("okhsl(20 72% 67%)", "234,127,129"),
        ("okhsl(83 88% 67%)", "205,154,34"),
        ("okhsl(229 6% 67%)", "157,165,169"),
        ("okhsl(229 8% 56%)", "126,136,142"),
        ("okhsl(234 3% 89%)", "222,224,225"),
        ("okhsl(226 7% 65%)", "150,160,164"),
        ("okhsl(233 41% 24%)", "33,59,73"),
        ("okhsl(237 7% 33%)", "72,78,82"),
        ("okhsl(232 7% 65%)", "151,160,165"),
        ("okhsl(53 51% 24%)", "78,47,27"),
        ("okhsl(295 42% 24%)", "58,48,85"),
        ("okhsl(229 5% 24%)", "52,56,58"),
        ("okhsl(158 46% 25%)", "37,65,49"),
        ("okhsl(19 54% 25%)", "91,40,42"),
        ("okhsl(232 54% 67%)", "105,173,208"),
        ("okhsl(202 58% 67%)", "93,179,186"),
        ("okhsl(52 67% 67%)", "222,141,90"),
        ("okhsl(229 8% 49%)", "108,118,123"),
        ("okhsl(232 20% 52%)", "104,128,141"),
        ("okhsl(232 45% 54%)", "84,137,164"),
        ("okhsl(263 59% 56%)", "97,133,204"),
        ("okhsl(295 73% 59%)", "151,118,229"),
        ("okhsl(337 81% 61%)", "222,84,193"),
        ("okhsl(20 99% 63%)", "254,84,98"),
        ("okhsl(159 64% 65%)", "94,178,134"),
        ("okhsl(295 60% 46%)", "116,89,180"),
        ("okhsl(231 67% 55%)", "61,142,179"),
        ("okhsl(295 59% 55%)", "138,114,203"),
        ("okhsl(235 7% 66%)", "154,162,167"),
        ("okhsl(159 75% 46%)", "51,126,88"),
        ("okhsl(20 91% 47%)", "200,37,61"),
        ("okhsl(83 99% 47%)", "143,104,2"),
        ("okhsl(229 8% 47%)", "103,113,118"),
        ("okhsl(229 7% 59%)", "135,144,149"),
        ("okhsl(225 5% 27%)", "59,63,65"),
        ("okhsl(234 8% 55%)", "124,134,140"),
        ("okhsl(235 19% 91%)", "223,231,236"),
        ("okhsl(248 3% 90%)", "225,227,228"),
        ("okhsl(56 22% 91%)", "237,227,221"),
        ("okhsl(295 25% 91%)", "230,228,238"),
        ("okhsl(248 3% 91%)", "228,229,230"),
        ("okhsl(156 21% 91%)", "222,233,225"),
        ("okhsl(24 23% 91%)", "238,226,225"),
        ("okhsl(231 68% 47%)", "47,120,153"),
        ("okhsl(203 73% 46%)", "40,122,129"),
        ("okhsl(52 84% 46%)", "164,84,23"),
        ("okhsl(223 5% 80%)", "194,200,202"),
        ("okhsl(229 14% 78%)", "181,196,203"),
        ("okhsl(232 33% 76%)", "159,194,213"),
        ("okhsl(264 48% 74%)", "162,183,224"),
        ("okhsl(295 62% 72%)", "181,165,232"),
        ("okhsl(337 74% 70%)", "229,133,205"),
        ("okhsl(20 98% 68%)", "254,116,121"),
        ("okhsl(159 74% 55%)", "64,151,108"),
        ("okhsl(0 100% 100%)", "255,255,255"),
        ("oklch(0.5 0.1 250)", "50,102,154"),
        ("#aabbcc", "170,187,204"),
        ("#abc", "170,187,204"),
    ];

    #[test]
    fn color_parsing_matches_upstream_vectors() {
        for (value, expected) in VECTORS {
            let rgb = match parse_color(value) {
                Ok(Color::Rgb(r, g, b)) => format!("{r},{g},{b}"),
                other => panic!("{value:?} -> {other:?}"),
            };
            assert_eq!(rgb, *expected, "{value}");
        }
    }

    #[test]
    fn color_parsing_forms_and_errors() {
        assert_eq!(parse_color(""), Ok(Color::Reset));
        assert_eq!(parse_color("123"), Ok(Color::Indexed(123)));
        assert_eq!(parse_color("#fff"), Ok(Color::Rgb(255, 255, 255)));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("xyz").is_err());
        assert!(parse_color("okhsl(10)").is_err());
        assert!(parse_color("okhsl(10 120% 50%)").is_err());
        assert!(parse_color("oklch(2 0.1 10)").is_err());
        assert!(parse_color("999").is_err(), "ANSI index 0-255 only");
    }

    #[test]
    fn builtin_themes_resolve_all_tokens() {
        for name in ["dark", "light"] {
            let t = Theme::builtin(name).unwrap();
            assert_eq!(t.name, name);
            assert_eq!(t.colors.len(), THEME_TOKENS.len());
        }
        let dark = Theme::builtin("dark").unwrap();
        // Golden: dark accent is the violet var okhsl(295 50% 67%).
        assert_eq!(dark.color("accent"), Color::Rgb(167, 152, 215));
        // Golden: dark mdHeading is the yellow var okhsl(83 88% 67%).
        assert_eq!(dark.color("mdHeading"), Color::Rgb(205, 154, 34));
        // Golden: dark text is okhsl(234 3% 89%).
        assert_eq!(dark.color("text"), Color::Rgb(222, 224, 225));
        let light = Theme::builtin("light").unwrap();
        // Golden: light accent is the violet var okhsl(295 60% 46%).
        assert_eq!(light.color("accent"), Color::Rgb(116, 89, 180));
        assert_eq!(dark.appearance.as_deref(), Some("dark"));
        assert_eq!(light.appearance.as_deref(), Some("light"));
    }

    #[test]
    fn custom_theme_loads_from_agent_dir() {
        let dir = std::env::temp_dir().join(format!("pi-rust-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        let mut colors: Vec<String> = Vec::new();
        for (token, _, _) in THEME_TOKENS {
            colors.push(format!("\"{token}\": \"#ff0000\""));
        }
        let json = format!(
            "{{\"name\":\"mine\",\"vars\":{{\"red\":\"#123456\"}},\"colors\":{{{}}}}}",
            colors.join(",")
        );
        // Override one token with a var reference and drop a fallback token.
        let json = json.replace("\"accent\": \"#ff0000\"", "\"accent\": \"red\"");
        let json = json.replace("\"scrollbarTrack\": \"#ff0000\", ", "");
        std::fs::write(dir.join("themes/mine.json"), &json).unwrap();
        let t = Theme::load(&dir, "mine").unwrap();
        assert_eq!(t.name, "mine");
        assert_eq!(t.color("accent"), Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(t.color("muted"), Color::Rgb(0xff, 0, 0));
        // Fallback: scrollbarTrack follows muted.
        assert_eq!(t.color("scrollbarTrack"), Color::Rgb(0xff, 0, 0));
        assert!(Theme::available_themes(&dir).contains(&"mine".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_theme_errors_with_available_list() {
        let dir = std::env::temp_dir().join(format!("pi-rust-theme-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = Theme::load(&dir, "nope").unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("dark") && err.contains("light"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_themes_error() {
        // Circular variable.
        let err = Theme::from_json(
            "t",
            r#"{"vars":{"a":"b","b":"a"},"colors":{"accent":"a","muted":"x"}}"#,
        )
        .unwrap_err();
        assert!(err.contains("circular"), "{err}");
        // Unknown variable.
        let err =
            Theme::from_json("t", r##"{"colors":{"accent":"ghost","muted":"#fff"}}"##).unwrap_err();
        assert!(err.contains("ghost"), "{err}");
        // Unknown token.
        let err =
            Theme::from_json("t", r##"{"colors":{"accent":"#fff","nope":"#fff"}}"##).unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("accent"), "lists valid tokens: {err}");
        // Missing required token.
        let err = Theme::from_json("t", r##"{"colors":{"accent":"#fff"}}"##).unwrap_err();
        assert!(err.contains("missing tokens"), "{err}");
        assert!(err.contains("muted"), "{err}");
        // Bad color value.
        let mut colors: Vec<String> = Vec::new();
        for (token, _, _) in THEME_TOKENS {
            colors.push(format!("\"{token}\": \"#fff\""));
        }
        let json = format!("{{\"colors\":{{{}}}}}", colors.join(","));
        let json = json.replace("\"accent\": \"#fff\"", "\"accent\": \"not-a-color\"");
        let err = Theme::from_json("t", &json).unwrap_err();
        assert!(err.contains("accent"), "{err}");
        assert!(err.contains("not-a-color"), "{err}");
    }

    #[test]
    fn resolve_theme_setting_semantics() {
        assert_eq!(resolve_theme_setting(Some("dark")), Some("dark".into()));
        assert_eq!(
            resolve_theme_setting(Some("light/dark")),
            Some("dark".into())
        );
        assert_eq!(
            resolve_theme_setting(Some("mylight/dark")),
            Some("dark".into())
        );
        assert_eq!(
            resolve_theme_setting(Some("a/b/c")),
            None,
            "double slash is ignored"
        );
        assert_eq!(
            resolve_theme_setting(Some("/dark")),
            None,
            "empty side is ignored"
        );
        assert_eq!(resolve_theme_setting(None), None);
        assert_eq!(resolve_theme_setting(Some("")), None);
    }
}
