//! Round 13 — `Stylesheet::resolve_imports` with caller-supplied
//! fetcher hook (CSS 2.1 §6.3). Round 11 captured `@import url(…)`
//! URLs; round 13 lets the caller resolve them into the rule list so
//! the cascade applies as if the rules were inline.

use oxideav_svg::css::Stylesheet;

#[test]
fn resolve_imports_inlines_fetched_rules() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("foo.css"); .x { fill: blue }"#);
    assert_eq!(s.rules.len(), 1, "before resolve, only inline rule");
    s.resolve_imports(|url| {
        if url == "foo.css" {
            Some(b".y { stroke: red }".to_vec())
        } else {
            None
        }
    });
    assert!(s.rules.len() >= 2, "after resolve, foo.css rules merged");
    let has_y = s.rules.iter().any(|r| {
        r.declarations
            .iter()
            .any(|(k, v)| k == "stroke" && v == "red")
    });
    assert!(has_y, "expected the imported `.y` rule's stroke=red");
}

#[test]
fn resolve_imports_skips_when_fetcher_returns_none() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("missing.css"); .a { fill: red }"#);
    let rules_before = s.rules.len();
    s.resolve_imports(|_| None);
    assert_eq!(
        s.rules.len(),
        rules_before,
        "missing import must not add rules"
    );
    // imports remain visible for caller introspection (cache invalidation, etc).
    assert_eq!(s.imports, vec!["missing.css".to_string()]);
}

#[test]
fn resolve_imports_silently_drops_invalid_utf8() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("bad.css");"#);
    s.resolve_imports(|_| Some(vec![0xff, 0xfe, 0xfd]));
    // Invalid UTF-8 → skipped; no rules added, no panic.
    assert_eq!(s.rules.len(), 0);
}

#[test]
fn resolve_imports_handles_cycle() {
    // a.css imports b.css; b.css imports a.css.
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("a.css");"#);
    s.resolve_imports(|url| match url {
        "a.css" => Some(b"@import url(\"b.css\"); .a { fill: red }".to_vec()),
        "b.css" => Some(b"@import url(\"a.css\"); .b { fill: blue }".to_vec()),
        _ => None,
    });
    // Both sheets should each contribute their non-circular rules.
    let has_a = s
        .rules
        .iter()
        .any(|r| r.declarations.iter().any(|(_, v)| v == "red"));
    let has_b = s
        .rules
        .iter()
        .any(|r| r.declarations.iter().any(|(_, v)| v == "blue"));
    assert!(has_a, "rule from a.css should be present");
    assert!(has_b, "rule from b.css should be present");
    // The cycle re-entry must NOT produce duplicate rule
    // appearances of either side (3 total: outer @import + 2
    // cycled-once entries; second cycle iteration is the cycle hit
    // and is skipped).
    let count_red = s
        .rules
        .iter()
        .filter(|r| r.declarations.iter().any(|(_, v)| v == "red"))
        .count();
    let count_blue = s
        .rules
        .iter()
        .filter(|r| r.declarations.iter().any(|(_, v)| v == "blue"))
        .count();
    assert_eq!(count_red, 1, "no duplicate `red` rule despite the cycle");
    assert_eq!(count_blue, 1, "no duplicate `blue` rule despite the cycle");
}

#[test]
fn resolve_imports_respects_depth_cap() {
    // Build an infinite chain a.css → a.css → a.css … (each fetch
    // returns "@import url('a.css');"). Cycle detection alone would
    // catch this; the depth cap is a separate belt-and-braces guard.
    // Here we make each level fetch a *different* URL so cycle
    // detection doesn't fire, and only depth cap stops us.
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("0");"#);
    s.resolve_imports(|url| {
        let n: usize = url.parse().ok()?;
        // Each level fetches the next URL — forms an unbounded chain.
        Some(format!("@import url(\"{}\");", n + 1).into_bytes())
    });
    // No rules from any of the levels (none had non-import content),
    // and no panic / runaway. Depth cap is 8.
    assert!(s.rules.is_empty());
}

#[test]
fn resolve_imports_recursive_chain_with_rules() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("level1.css"); .root { fill: black }"#);
    s.resolve_imports(|url| match url {
        "level1.css" => Some(b"@import url(\"level2.css\"); .l1 { fill: green }".to_vec()),
        "level2.css" => Some(b".l2 { fill: yellow }".to_vec()),
        _ => None,
    });
    // All three rules should be present (root + level1 + level2).
    let mut found_fills: Vec<String> = s
        .rules
        .iter()
        .flat_map(|r| {
            r.declarations
                .iter()
                .filter_map(|(k, v)| if k == "fill" { Some(v.clone()) } else { None })
        })
        .collect();
    found_fills.sort();
    assert!(found_fills.contains(&"black".to_string()));
    assert!(found_fills.contains(&"green".to_string()));
    assert!(found_fills.contains(&"yellow".to_string()));
}
