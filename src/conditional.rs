//! SVG conditional processing — the `<switch>` element plus the
//! `requiredFeatures` / `requiredExtensions` / `systemLanguage`
//! test attributes (SVG 2 §5.7 / SVG 1.1 §5.8).
//!
//! Per SVG 2 §5.7.1 the `<switch>` element renders the **first** of its
//! direct child elements for which **all** of the conditional
//! processing attributes evaluate to `true`; the rest are bypassed
//! ("treated as if it had a used value of `none` for the `display`
//! property"). If a test attribute is absent it implicitly evaluates to
//! `true`; an explicitly empty value evaluates to `false`.
//!
//! The three test attributes (SVG 2 §5.7.2 keeps the first two; SVG 1.1
//! adds `requiredFeatures`):
//!
//! * **`requiredExtensions`** (§5.7.4) — a *space*-separated list of
//!   URL-reference tokens naming language extensions that go *beyond*
//!   this specification. oxideav supports no proprietary extensions, so
//!   any non-empty list evaluates to `false` (per §5.7.4: "the current
//!   element and its children are skipped" when an extension is not
//!   supported). Absent → `true`; empty string → `false`.
//! * **`systemLanguage`** (§5.7.5) — a *comma*-separated list of BCP 47
//!   language tags. Evaluates to `true` if one of the user's preferred
//!   languages is a case-insensitive match of a listed tag, OR a
//!   case-insensitive prefix such that the first character after the
//!   prefix is `-`. Absent → `true`; empty string → `false`.
//! * **`requiredFeatures`** — *removed in SVG 2* (§5.7.1 records the
//!   deprecation: "poor specification and implementation … made it
//!   unreliable as a test of feature support"). For SVG 1.1 content
//!   compatibility we evaluate a non-empty `requiredFeatures` as `true`
//!   (oxideav implements the SVG static feature set the attribute was
//!   meant to gate), matching modern user agents that always pass it.
//!   An explicitly empty string still evaluates to `false` per the
//!   SVG 1.1 §5.8.6 null/empty rule.
//!
//! The user's language preference list defaults to `["en"]` and can be
//! overridden once at startup via [`set_system_languages`], mirroring
//! the `<text>` font-resolver hook — the SVG crate does not own a
//! locale registry.

use std::sync::OnceLock;

use crate::parser::{attr, tag_local, Element, Node as XmlNode};

/// Global user-language preference list for `systemLanguage` matching.
/// Set once via [`set_system_languages`]; defaults to `["en"]`.
static SYSTEM_LANGUAGES: OnceLock<Vec<String>> = OnceLock::new();

/// The system-language preference is one-shot — only the first
/// [`set_system_languages`] call wins. Subsequent calls return this
/// error without overwriting the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguagesAlreadySet;

impl std::fmt::Display for LanguagesAlreadySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("system languages were already set")
    }
}

impl std::error::Error for LanguagesAlreadySet {}

/// Install the user's preferred-language list used to evaluate
/// `systemLanguage` test attributes (SVG 2 §5.7.5). Tags are matched
/// case-insensitively; the list should be in preference order though
/// order does not affect the boolean result. Returns
/// [`LanguagesAlreadySet`] if the list was already set — the hook is
/// one-shot (applications register at startup, like the `<text>` font
/// resolver).
///
/// When unset, `systemLanguage` is evaluated against the single default
/// preference `"en"`.
pub fn set_system_languages<I, S>(langs: I) -> std::result::Result<(), LanguagesAlreadySet>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let list: Vec<String> = langs.into_iter().map(Into::into).collect();
    SYSTEM_LANGUAGES.set(list).map_err(|_| LanguagesAlreadySet)
}

/// The current user-language preference list (the configured value or
/// the default `["en"]`).
fn current_languages() -> Vec<String> {
    SYSTEM_LANGUAGES
        .get()
        .cloned()
        .unwrap_or_else(|| vec!["en".to_string()])
}

/// Evaluate the `requiredExtensions` test attribute on `el` (SVG 2
/// §5.7.4).
///
/// * absent → `true`
/// * empty / whitespace-only → `false` (§5.7.4 null/empty rule)
/// * any non-empty token list → `false` (no extensions supported)
///
/// Since oxideav supports no proprietary extensions, the test fails
/// whenever the attribute is *present* in any form — a non-empty list
/// names extensions we lack, and an empty value is explicitly "false".
fn eval_required_extensions(el: &Element) -> bool {
    attr(el, "requiredExtensions").is_none()
}

/// Evaluate the `requiredFeatures` test attribute on `el` (SVG 1.1
/// §5.8.5; removed in SVG 2).
///
/// * absent → `true`
/// * empty / whitespace-only → `false` (SVG 1.1 null/empty rule)
/// * any non-empty token list → `true` (oxideav implements the SVG
///   static feature set the attribute gates; modern UAs always pass it)
fn eval_required_features(el: &Element) -> bool {
    match attr(el, "requiredFeatures") {
        None => true,
        // Non-empty: pass. Empty / whitespace-only: fail.
        Some(v) => v.split_whitespace().next().is_some(),
    }
}

/// Case-insensitive BCP 47 match per SVG 2 §5.7.5: `pref` matches
/// `tag` if it equals `tag`, or is a prefix of `tag` whose next
/// character in `tag` is `-`.
fn lang_matches(pref: &str, tag: &str) -> bool {
    if pref.eq_ignore_ascii_case(tag) {
        return true;
    }
    // Prefix match: `pref` must be followed by `-` in `tag`.
    let plen = pref.len();
    tag.len() > plen && tag.as_bytes()[plen] == b'-' && tag[..plen].eq_ignore_ascii_case(pref)
}

/// Evaluate the `systemLanguage` test attribute on `el` against the
/// configured user-language preferences (SVG 2 §5.7.5).
///
/// * absent → `true`
/// * empty → `false`
/// * otherwise → `true` iff some preference matches some listed tag
fn eval_system_language(el: &Element) -> bool {
    match attr(el, "systemLanguage") {
        None => true,
        Some(v) => {
            let tags: Vec<&str> = v
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if tags.is_empty() {
                // Empty / whitespace-only / commas-only → "false".
                return false;
            }
            let prefs = current_languages();
            prefs
                .iter()
                .any(|pref| tags.iter().any(|tag| lang_matches(pref, tag)))
        }
    }
}

/// Evaluate every conditional processing attribute on `el`. Returns
/// `true` only when **all** present tests pass (SVG 2 §5.7.1 — "the
/// first of its children for which all of these attributes test true").
pub fn evaluate_conditional(el: &Element) -> bool {
    eval_required_features(el) && eval_required_extensions(el) && eval_system_language(el)
}

/// Pick the first direct child *element* of a `<switch>` for which
/// [`evaluate_conditional`] is `true` (SVG 2 §5.7.3). Text-node
/// children and never-rendered structural helpers (`<defs>`, `<style>`,
/// `<script>`) are skipped — they are not selectable switch branches.
/// Returns `None` when no branch matches.
pub fn select_switch_child(switch_el: &Element) -> Option<&Element> {
    for child in &switch_el.children {
        if let XmlNode::Element(c) = child {
            // Never-rendered helpers are not candidate switch branches
            // per §5.7.1 ("conditional processing does not affect the
            // processing of a style or script element"); they are also
            // not in the §5.7.3 content model. Skip so they never win
            // the switch ahead of a real renderable branch.
            let local = tag_local(&c.name).to_ascii_lowercase();
            if matches!(local.as_str(), "defs" | "style" | "script") {
                continue;
            }
            if evaluate_conditional(c) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_xml;

    fn first_el(src: &str) -> Element {
        match parse_xml(src).unwrap().into_iter().next().unwrap() {
            XmlNode::Element(e) => e,
            _ => panic!("expected element"),
        }
    }

    #[test]
    fn absent_attrs_pass() {
        let el = first_el("<g/>");
        assert!(evaluate_conditional(&el));
    }

    #[test]
    fn empty_required_extensions_fails() {
        let el = first_el(r#"<g requiredExtensions=""/>"#);
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn nonempty_required_extensions_fails() {
        let el = first_el(r#"<g requiredExtensions="http://example.org/Ext/1.0"/>"#);
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn whitespace_only_required_extensions_fails() {
        let el = first_el("<g requiredExtensions=\"   \"/>");
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn nonempty_required_features_passes() {
        let el = first_el(r#"<g requiredFeatures="http://www.w3.org/TR/SVG11/feature#Shape"/>"#);
        assert!(evaluate_conditional(&el));
    }

    #[test]
    fn empty_required_features_fails() {
        let el = first_el(r#"<g requiredFeatures=""/>"#);
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn lang_exact_and_prefix_match() {
        assert!(lang_matches("en", "en"));
        assert!(lang_matches("en", "en-US"));
        assert!(lang_matches("EN", "en-GB")); // case-insensitive
        assert!(!lang_matches("en", "eng")); // not a `-` boundary
        assert!(!lang_matches("en-US", "en")); // prefix direction
        assert!(!lang_matches("fr", "en"));
    }

    #[test]
    fn system_language_default_en_matches() {
        // Default preference list is ["en"].
        let el = first_el(r#"<g systemLanguage="fr, en-GB, de"/>"#);
        assert!(evaluate_conditional(&el));
        let el = first_el(r#"<g systemLanguage="fr, de"/>"#);
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn empty_system_language_fails() {
        let el = first_el(r#"<g systemLanguage=""/>"#);
        assert!(!evaluate_conditional(&el));
        let el = first_el(r#"<g systemLanguage=" , "/>"#);
        assert!(!evaluate_conditional(&el));
    }

    #[test]
    fn select_first_matching_branch() {
        let sw = first_el(
            r#"<switch>
                 <rect systemLanguage="fr"/>
                 <rect systemLanguage="de"/>
                 <rect/>
               </switch>"#,
        );
        // First two fail (fr/de don't match default "en"); the bare
        // catch-all wins.
        let chosen = select_switch_child(&sw).expect("a branch should match");
        assert_eq!(tag_local(&chosen.name), "rect");
        assert!(attr(chosen, "systemLanguage").is_none());
    }

    #[test]
    fn select_skips_never_rendered_helpers() {
        let sw = first_el(
            r#"<switch>
                 <defs/>
                 <style>rect{fill:red}</style>
                 <rect/>
               </switch>"#,
        );
        let chosen = select_switch_child(&sw).expect("a branch should match");
        assert_eq!(tag_local(&chosen.name), "rect");
    }

    #[test]
    fn select_none_when_no_branch_matches() {
        let sw = first_el(
            r#"<switch>
                 <rect systemLanguage="fr"/>
                 <rect requiredExtensions="http://example.org/X"/>
               </switch>"#,
        );
        assert!(select_switch_child(&sw).is_none());
    }
}
