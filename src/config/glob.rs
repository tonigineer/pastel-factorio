//! Wildcard matching: path globs for the `[classify]` rules, and shell-style
//! patterns for category names (`build --only`, `sheets --category`).
use regex::Regex;

/// Glob with ** (any depth), * (within a folder), ? and {a,b} alternatives.
/// Case-insensitive, matches the whole path.
pub fn glob_to_regex(pattern: &str) -> Regex {
    Regex::new(&format!("(?i)^{}$", glob_body(pattern))).expect("glob compiles to a valid regex")
}

/// The regex for a glob, without anchors or flags.
fn glob_body(pattern: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < pattern.len() {
        let rest = &pattern[i..];
        let c = rest.chars().next().unwrap();
        if rest.starts_with("**/") {
            out.push_str("(?:.*/)?");
            i += 3;
        } else if rest.starts_with("**") {
            out.push_str(".*");
            i += 2;
        } else if c == '*' {
            out.push_str("[^/]*");
            i += 1;
        } else if c == '?' {
            out.push_str("[^/]");
            i += 1;
        } else if c == '{' && rest.contains('}') {
            let j = rest.find('}').unwrap();
            let alts: Vec<String> = rest[1..j].split(',').map(glob_body).collect();
            out.push_str(&format!("(?:{})", alts.join("|")));
            i += j + 1;
        } else {
            out.push_str(&regex::escape(&c.to_string()));
            i += c.len_utf8();
        }
    }
    out
}

/// Shell-style match of a whole name (like Python's fnmatch.fnmatchcase):
/// `*` and `?` match any character, `[abc]` / `[!abc]` are character sets.
pub fn fnmatch(name: &str, pattern: &str) -> bool {
    let mut re = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => match chars[i + 1..].iter().position(|&c| c == ']') {
                Some(n) if n > 0 => {
                    let set: String = chars[i + 1..i + 1 + n].iter().collect();
                    let set = set.replace('\\', "\\\\");
                    match set.strip_prefix('!') {
                        Some(s) => re.push_str(&format!("[^{s}]")),
                        None => re.push_str(&format!("[{set}]")),
                    }
                    i += n + 1;
                }
                _ => re.push_str("\\["),
            },
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&format!("(?s){re}")).is_ok_and(|r| r.is_match(name))
}

/// Predicate for --only: category names matched by comma-separated globs.
/// No patterns (None or "") selects every category.
pub fn only_filter(patterns: Option<&str>) -> impl Fn(&str) -> bool {
    let pats: Vec<String> = patterns
        .unwrap_or("")
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let all = patterns.is_none_or(|p| p.is_empty());
    move |name| all || pats.iter().any(|p| fnmatch(name, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let r = glob_to_regex("base/graphics/entity/**/*-light*.png");
        assert!(r.is_match("base/graphics/entity/lamp/lamp-light.png"));
        assert!(r.is_match("base/graphics/entity/a/b/x-lights.PNG"));
        assert!(!r.is_match("base/graphics/icons/x-light.png"));
        let r = glob_to_regex("**/*mask*.png");
        assert!(r.is_match("x-mask.png") && r.is_match("a/b/mask.png"));
        let r = glob_to_regex("base/graphics/entity/{biter,spitter}/**");
        assert!(r.is_match("base/graphics/entity/spitter/x/y.png"));
        assert!(!r.is_match("base/graphics/entity/worm/y.png"));
    }

    #[test]
    fn fnmatches() {
        assert!(fnmatch("terrain.grass", "terrain.*"));
        assert!(!fnmatch("terrainXgrass", "terrain.g*x"));
        assert!(fnmatch("ore.iron", "ore.[ic]ron"));
        assert!(fnmatch("lut", "lut"));
        assert!(!fnmatch("lut2", "lut"));
    }

    #[test]
    fn only_filters() {
        let all = only_filter(None);
        assert!(all("terrain.grass") && all("lut"));
        let all = only_filter(Some(""));
        assert!(all("shadow"));
        let some = only_filter(Some("terrain.*, lut"));
        assert!(some("terrain.grass") && some("lut"));
        assert!(!some("entity") && !some("lut2"));
    }
}
