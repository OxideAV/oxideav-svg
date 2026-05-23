//! SVG 2 §5.7 — conditional processing.
//!
//! Implements the `requiredExtensions` (§5.7.4) and `systemLanguage`
//! (§5.7.5) test attributes plus the evaluation rule the `<switch>`
//! element (§5.7.3) applies to its direct children: render the first
//! child for which *all* conditional processing attributes test true,
//! bypass the rest.
//!
//! Quoting SVG 2 §5.7.1 (Conditional processing overview):
//!
//! > Attributes `requiredExtensions` and `systemLanguage` act as tests
//! > and evaluate to either true or false. The `switch` renders the
//! > first of its children for which all of these attributes test true.
//! > If the given attribute is not specified, then a true value is
//! > assumed.
//!
//! Per §5.7.1, the legacy `requiredFeatures` attribute was **removed**
//! in SVG 2 ("poor specification and implementation … made it
//! unreliable"); it is no longer a conditional processing attribute, so
//! it is *not* evaluated here — only `requiredExtensions` and
//! `systemLanguage` are tests.

use crate::parser::{attr, Element};

/// SVG 2 §5.7.4 — evaluate the `requiredExtensions` attribute.
///
/// > The value is a list of URL references which identify the required
/// > extensions, with the individual values separated by white space.
/// > Determines whether all of the named extensions are supported by
/// > the user agent. If all of the given extensions are supported, then
/// > the attribute evaluates to true; otherwise, the current element
/// > and its children are skipped and thus will not be rendered.
/// >
/// > If the attribute is not present, then it implicitly evaluates to
/// > "true". If a null string or empty string value is given to
/// > attribute `requiredExtensions`, the attribute evaluates to
/// > "false".
///
/// oxideav implements **no** language extensions, so a non-empty list
/// of required extensions can never be fully satisfied → the attribute
/// evaluates to false. Absent → true; empty string → false (per the
/// spec's explicit null/empty rule).
pub fn eval_required_extensions(el: &Element) -> bool {
    match attr(el, "requiredExtensions") {
        // Not present → implicitly true.
        None => true,
        // Empty / whitespace-only string → false per §5.7.4.
        Some(v) if v.split_whitespace().next().is_none() => false,
        // A non-empty list names at least one extension. We support
        // none, so "all of the given extensions are supported" is
        // false.
        Some(_) => false,
    }
}

/// SVG 2 §5.7.5 — evaluate the `systemLanguage` attribute against the
/// user-preferred language list `user_langs`.
///
/// > The value is a set of comma-separated tokens, each of which must
/// > be a Language-Tag value, as defined in BCP 47. Evaluates to "true"
/// > if one of the language tags indicated by user preferences is a
/// > case-insensitive match of one of the language tags given in the
/// > value of this parameter, or if one of the language tags indicated
/// > by user preferences is a case-insensitive prefix of one of the
/// > language tags given in the value of this parameter such that the
/// > first tag character following the prefix is "-".
/// >
/// > Evaluates to "false" otherwise.
/// >
/// > If the attribute is not present, then it implicitly evaluates to
/// > "true". If a null string or empty string value is given to
/// > attribute `systemLanguage`, the attribute evaluates to "false".
///
/// `user_langs` carries the "language tags indicated by user
/// preferences"; the caller supplies it (oxideav owns no user-agent
/// locale registry). An empty `user_langs` means no language is
/// preferred, so a present-and-non-empty `systemLanguage` evaluates to
/// false (no preference can match) while an absent attribute still
/// evaluates to true.
pub fn eval_system_language(el: &Element, user_langs: &[String]) -> bool {
    match attr(el, "systemLanguage") {
        // Not present → implicitly true.
        None => true,
        Some(v) => {
            // Comma-separated tokens; trim surrounding white space.
            let attr_tags: Vec<&str> = v
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            // Empty / whitespace-only string → false per §5.7.5.
            if attr_tags.is_empty() {
                return false;
            }
            for user in user_langs {
                let user = user.trim();
                if user.is_empty() {
                    continue;
                }
                for tag in &attr_tags {
                    if lang_matches(user, tag) {
                        return true;
                    }
                }
            }
            false
        }
    }
}

/// §5.7.5 match test for a single (user-preference, attribute-tag) pair:
/// case-insensitive exact match, OR `user` is a case-insensitive prefix
/// of `tag` where the first tag character following the prefix is "-".
///
/// (The spec phrases the prefix direction as "one of the language tags
/// indicated by user preferences is a … prefix of one of the language
/// tags given in the value"; i.e. user `en` matches attribute `en-US`.)
fn lang_matches(user: &str, tag: &str) -> bool {
    if user.eq_ignore_ascii_case(tag) {
        return true;
    }
    if tag.len() > user.len()
        && tag[..user.len()].eq_ignore_ascii_case(user)
        && tag.as_bytes()[user.len()] == b'-'
    {
        return true;
    }
    false
}

/// Does `el` pass *all* its conditional processing tests (§5.7.1)?
///
/// > The `switch` renders the first of its children for which all of
/// > these attributes test true.
pub fn passes_conditional(el: &Element, user_langs: &[String]) -> bool {
    eval_required_extensions(el) && eval_system_language(el, user_langs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_xml;
    use crate::parser::Node as XmlNode;

    fn first_el(src: &str) -> Element {
        let nodes = parse_xml(src).unwrap();
        for n in nodes {
            if let XmlNode::Element(e) = n {
                return e;
            }
        }
        panic!("no element");
    }

    fn langs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // --- requiredExtensions (§5.7.4) -------------------------------

    #[test]
    fn required_extensions_absent_is_true() {
        let el = first_el(r#"<rect/>"#);
        assert!(eval_required_extensions(&el));
    }

    #[test]
    fn required_extensions_empty_string_is_false() {
        let el = first_el(r#"<rect requiredExtensions=""/>"#);
        assert!(!eval_required_extensions(&el));
    }

    #[test]
    fn required_extensions_whitespace_only_is_false() {
        let el = first_el(r#"<rect requiredExtensions="   "/>"#);
        assert!(!eval_required_extensions(&el));
    }

    #[test]
    fn required_extensions_named_is_false_we_support_none() {
        let el = first_el(r#"<rect requiredExtensions="http://example.org/Ext/1.0"/>"#);
        assert!(!eval_required_extensions(&el));
    }

    // --- systemLanguage (§5.7.5) -----------------------------------

    #[test]
    fn system_language_absent_is_true() {
        let el = first_el(r#"<text/>"#);
        assert!(eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_empty_string_is_false() {
        let el = first_el(r#"<text systemLanguage=""/>"#);
        assert!(!eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_exact_match_case_insensitive() {
        let el = first_el(r#"<text systemLanguage="EN"/>"#);
        assert!(eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_prefix_match() {
        // user "en" is a prefix of attribute "en-US" with a "-" next.
        let el = first_el(r#"<text systemLanguage="en-US"/>"#);
        assert!(eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_prefix_requires_dash_boundary() {
        // user "en" must NOT match attribute "english" (no "-" after).
        let el = first_el(r#"<text systemLanguage="english"/>"#);
        assert!(!eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_multi_token_any_match() {
        let el = first_el(r#"<text systemLanguage="mi, en"/>"#);
        assert!(eval_system_language(&el, &langs(&["en"])));
    }

    #[test]
    fn system_language_no_user_pref_is_false() {
        // No user-preferred language → a present, non-empty
        // systemLanguage can match nothing.
        let el = first_el(r#"<text systemLanguage="en"/>"#);
        assert!(!eval_system_language(&el, &langs(&[])));
    }

    #[test]
    fn system_language_no_match_is_false() {
        let el = first_el(r#"<text systemLanguage="fr, de"/>"#);
        assert!(!eval_system_language(&el, &langs(&["en"])));
    }

    // --- combined (§5.7.1) -----------------------------------------

    #[test]
    fn passes_conditional_both_true() {
        let el = first_el(r#"<g systemLanguage="en"/>"#);
        assert!(passes_conditional(&el, &langs(&["en"])));
    }

    #[test]
    fn passes_conditional_one_false_fails() {
        // systemLanguage true, requiredExtensions false → overall false.
        let el = first_el(r#"<g systemLanguage="en" requiredExtensions="urn:x"/>"#);
        assert!(!passes_conditional(&el, &langs(&["en"])));
    }
}
