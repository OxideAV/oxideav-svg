//! Round 215 — SVG 1.1 §14.3.5 `clip-rule` property integration tests.
//!
//! `clip-rule: nonzero | evenodd | inherit` selects which fill-rule
//! algorithm computes membership of the clipping path's interior. Per
//! §14.3.5:
//!
//! * Initial value `nonzero`.
//! * Inherited (`Inherited: yes`).
//! * Applies to graphics elements **within a `<clipPath>` element**
//!   only — `clip-rule="evenodd"` on the *referencing* element (the
//!   shape with `clip-path="url(#…)"`) is ignored.
//! * The rule on the `<clipPath>` element cascades to its shape
//!   children (since the property is inherited); a per-child
//!   `clip-rule=` overrides the inherited value.
//!
//! Round 215 ships parse (typed [`oxideav_core::FillRule`] on
//! [`oxideav_svg::defs::ClipPathDef::clip_rule`]) + scope-restricted
//! cascade + round-trip preservation via
//! [`oxideav_svg::preserved::PreservedExtras::clip_rules`]. The actual
//! clipping evaluation honours the rule when a rasterizer that consults
//! `ClipPathDef::clip_rule` is downstream; the round-1 scene-graph
//! representation (`Group::clip`) carries the path geometry only — the
//! typed def is the source of truth for the rule.

use oxideav_core::FillRule;
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Round 215: a `<clipPath>` with no `clip-rule=` attribute (anywhere)
/// resolves to the §14.3.5 initial value `nonzero` and does NOT record
/// a side-channel binding.
#[test]
fn baseline_no_clip_rule_attr_no_binding() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.clip_rules.is_empty(),
        "round 215: a clipPath with no clip-rule= must not record a binding"
    );
}

/// Round 215: `clip-rule="evenodd"` on the inner shape records a
/// binding with the source `<clipPath>` id and the canonical keyword.
#[test]
fn evenodd_on_child_shape_records_binding() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="myclip">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#myclip)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.clip_rules.len(),
        1,
        "round 215: clip-rule=evenodd on child shape must record exactly one binding"
    );
    assert_eq!(extras.clip_rules[0].clip_path_id, "myclip");
    assert_eq!(extras.clip_rules[0].clip_rule, "evenodd");
}

/// Round 215: `clip-rule="evenodd"` on the `<clipPath>` element itself
/// cascades to the child shape per §14.3.5 (inherited property), so
/// the binding records `evenodd` for the merged path.
#[test]
fn evenodd_on_clip_path_element_cascades_to_child() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="myclip" clip-rule="evenodd">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#myclip)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 1);
    assert_eq!(extras.clip_rules[0].clip_rule, "evenodd");
}

/// Round 215: per-child `clip-rule=` overrides the inherited rule from
/// the `<clipPath>` element (the §14.3.5 example).
#[test]
fn per_child_clip_rule_overrides_inherited() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="myclip" clip-rule="nonzero">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#myclip)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 1);
    assert_eq!(
        extras.clip_rules[0].clip_rule, "evenodd",
        "round 215: per-child clip-rule must override the inherited rule"
    );
}

/// Round 215: an explicit `clip-rule="nonzero"` (the initial value)
/// still records a binding — the round-trip preserves the author's
/// intent even when the value equals the spec default.
#[test]
fn explicit_nonzero_records_binding_for_round_trip() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="myclip">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="nonzero"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#myclip)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 1);
    assert_eq!(extras.clip_rules[0].clip_rule, "nonzero");
}

/// Round 215: case-insensitive matching of the keyword (`EVENODD`
/// works the same as `evenodd`). The canonicalised form on the
/// binding is always lowercase.
#[test]
fn case_insensitive_keyword_match_canonicalises_lowercase() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="EVENODD"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 1);
    assert_eq!(extras.clip_rules[0].clip_rule, "evenodd");
}

/// Round 215: unknown / malformed `clip-rule=` payloads fall back to
/// the §14.3.5 initial value `nonzero` and do NOT poison the side-
/// channel with a binding.
#[test]
fn unknown_keyword_falls_back_to_initial_without_binding() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="bogus-rule"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.clip_rules.is_empty(),
        "round 215: an unknown clip-rule= keyword must fall through to the initial value silently"
    );
}

/// Round 215: per §14.3.5, `clip-rule` on the *referencing* element
/// (the shape with `clip-path="url(#…)"`) is ignored — the source
/// example "whereas the following fragment of code will not cause an
/// evenodd clipping rule to be applied". The binding therefore does
/// NOT record a rule from the referencing shape.
#[test]
fn clip_rule_on_referencing_element_is_ignored() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="myclip">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red"
            clip-path="url(#myclip)" clip-rule="evenodd"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.clip_rules.is_empty(),
        "round 215: clip-rule= on the referencing element must be ignored per §14.3.5"
    );
}

/// Round 215: `<clipPath>` without an `id` attribute can't be
/// referenced, so the binding skips it even when `clip-rule=evenodd`
/// is present.
#[test]
fn clip_path_without_id_records_no_binding() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath>
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.clip_rules.is_empty(),
        "round 215: an id-less clipPath cannot be referenced so the binding must skip it"
    );
}

/// Round 215: `parse_svg` (no extras) still loads a document with
/// `clip-rule=` cleanly. The round-2 scene graph populates a `Group`
/// with the clipping path; the rule lives on the typed
/// `DefsTables::clip_paths` view (which is internal to the parser) and
/// the rule keyword goes nowhere — `parse_svg` is the no-round-trip
/// fast path.
#[test]
fn parse_svg_without_extras_still_loads_document() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let frame = parse_svg(src).expect("parse_svg must load a document carrying clip-rule cleanly");
    assert_eq!(frame.root.children.len(), 1);
}

/// Round 215: round-trip — `parse_svg_with_extras → write_svg_with_extras`
/// re-emits `clip-rule="evenodd"` on the inner `<path>` of the
/// `<clipPath>` def, matching the §14.3.5 worked example structure
/// (rule on the clipping-shape, not the referencing element).
#[test]
fn round_trip_re_emits_evenodd_on_inner_path() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        out.contains("clip-rule=\"evenodd\""),
        "round 215: round-trip must re-emit clip-rule=evenodd on the inner path; got:\n{}",
        out
    );
    // The attribute must land inside the <clipPath> def, not on the
    // referencing <rect>.
    let cp_pos = out.find("<clipPath").expect("clipPath emitted");
    let cp_end_pos = out.find("</clipPath>").expect("clipPath closed");
    let rule_pos = out
        .find("clip-rule=\"evenodd\"")
        .expect("rule emitted somewhere");
    assert!(
        rule_pos > cp_pos && rule_pos < cp_end_pos,
        "round 215: the clip-rule keyword must land inside the <clipPath> def"
    );
}

/// Round 215: round-trip — an explicit `clip-rule="nonzero"` (the
/// initial value) round-trips with the source intent preserved.
#[test]
fn round_trip_preserves_explicit_nonzero() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="nonzero"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        out.contains("clip-rule=\"nonzero\""),
        "round 215: explicit clip-rule=nonzero must round-trip (author intent preserved); got:\n{}",
        out
    );
}

/// Round 215: a `<clipPath>` with no `clip-rule=` does NOT add the
/// attribute on round-trip (the §14.3.5 initial value `nonzero` is
/// silent — avoids bloating outputs with redundant defaults).
#[test]
fn round_trip_omits_attribute_when_default_and_unset() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        !out.contains("clip-rule="),
        "round 215: a clipPath without an explicit clip-rule= must round-trip without the attribute; got:\n{}",
        out
    );
}

/// Round 215: a double round-trip converges — the canonical form is
/// idempotent.
#[test]
fn double_round_trip_converges() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z" clip-rule="evenodd"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 215: a double round-trip must converge — the canonical form is idempotent"
    );
}

/// Round 215: a `<clipPath>` containing multiple shape children
/// records the resolved rule from the first contributing child (the
/// merged path can only honour one rule). Subsequent children's
/// per-shape rules are tolerated but not separately emitted — see
/// `parse_clip_path_def`'s docstring.
#[test]
fn first_child_rule_wins_for_merged_path() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c1">
          <path d="M 0 0 L 50 0 L 50 50 L 0 50 Z" clip-rule="evenodd"/>
          <path d="M 50 50 L 100 50 L 100 100 L 50 100 Z" clip-rule="nonzero"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="100" height="100" fill="red" clip-path="url(#c1)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 1);
    assert_eq!(
        extras.clip_rules[0].clip_rule, "evenodd",
        "round 215: first child's rule wins for the merged path"
    );
}

/// Round 215: the typed [`oxideav_svg::defs::ClipPathDef::clip_rule`]
/// field exposes the resolved rule for downstream consumers that read
/// the parser's defs table (e.g. a rasterizer that wants the
/// non-default rule without round-tripping through XML). The typed
/// def is internal to the parser, but we verify the resolution via
/// `parse_clip_path_def` directly.
#[test]
fn typed_def_exposes_resolved_rule_evenodd() {
    use oxideav_svg::defs::ClipPathDef;
    use oxideav_svg::element::{parse_clip_path_def, ParseContext};
    use oxideav_svg::parser::{parse_xml, Node as XmlNode};

    let src = r##"<clipPath id="c1" clip-rule="evenodd">
      <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
    </clipPath>"##;
    let nodes = parse_xml(src).unwrap();
    let el = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) => Some(e),
            _ => None,
        })
        .unwrap();
    let mut ctx = ParseContext::new();
    let parsed = parse_clip_path_def(el, &mut ctx).unwrap();
    let (id, def): (String, ClipPathDef) = parsed.unwrap();
    assert_eq!(id, "c1");
    assert_eq!(def.clip_rule, FillRule::EvenOdd);
}

/// Round 215: the typed clip-rule defaults to `nonzero` when the
/// source has no explicit value.
#[test]
fn typed_def_default_rule_is_nonzero() {
    use oxideav_svg::defs::ClipPathDef;
    use oxideav_svg::element::{parse_clip_path_def, ParseContext};
    use oxideav_svg::parser::{parse_xml, Node as XmlNode};

    let src = r##"<clipPath id="c1">
      <path d="M 0 0 L 100 0 L 100 100 L 0 100 Z"/>
    </clipPath>"##;
    let nodes = parse_xml(src).unwrap();
    let el = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) => Some(e),
            _ => None,
        })
        .unwrap();
    let mut ctx = ParseContext::new();
    let (_id, def): (String, ClipPathDef) = parse_clip_path_def(el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.clip_rule, FillRule::NonZero);
}

/// Round 215: two distinct `<clipPath>` defs with different rules
/// each record their own binding.
#[test]
fn two_clip_paths_record_distinct_bindings() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
      <defs>
        <clipPath id="c_even">
          <path d="M 0 0 L 50 0 L 50 50 L 0 50 Z" clip-rule="evenodd"/>
        </clipPath>
        <clipPath id="c_non">
          <path d="M 50 50 L 100 50 L 100 100 L 50 100 Z" clip-rule="nonzero"/>
        </clipPath>
      </defs>
      <rect x="0" y="0" width="50" height="50" fill="red" clip-path="url(#c_even)"/>
      <rect x="50" y="50" width="50" height="50" fill="blue" clip-path="url(#c_non)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.clip_rules.len(), 2);
    let by_id: std::collections::HashMap<&str, &str> = extras
        .clip_rules
        .iter()
        .map(|b| (b.clip_path_id.as_str(), b.clip_rule.as_str()))
        .collect();
    assert_eq!(by_id.get("c_even"), Some(&"evenodd"));
    assert_eq!(by_id.get("c_non"), Some(&"nonzero"));
}
