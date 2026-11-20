//! `text::set_font_provider`: the provider sees family / weight / style,
//! is asked once per distinct font of a document (whatever the sizes),
//! and missing styles are synthesised.
//!
//! Own integration-test binary: the font hook is process-global.

#![cfg(feature = "text")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::text::{FontQuery, set_font_provider};
use oxideav_svg::{Group, Node, SvgDocument, parse};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

static CALLS: AtomicUsize = AtomicUsize::new(0);
static QUERIES: Mutex<Vec<FontQuery>> = Mutex::new(Vec::new());

fn install() {
    // One parsed face chain for the whole process: the provider hands
    // out clones of the `Arc`.
    static CHAIN: OnceLock<Arc<FaceChain>> = OnceLock::new();
    let _ = set_font_provider(|q| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        QUERIES.lock().unwrap().push(q.clone());
        if q.families().any(|f| f == "Known") {
            #[allow(clippy::arc_with_non_send_sync)]
            Some(
                CHAIN
                    .get_or_init(|| {
                        Arc::new(FaceChain::new(
                            Face::from_ttf_bytes(FONT.to_vec()).expect("fixture font"),
                        ))
                    })
                    .clone(),
            )
        } else {
            None
        }
    });
}

fn doc(body: &str) -> SvgDocument {
    install();
    let src = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400" font-family="Known">{body}</svg>"##
    );
    parse(src.as_bytes()).expect("parse")
}

fn text_group(d: &SvgDocument) -> &Group {
    match d.root.children.last().expect("text node") {
        Node::Group(g) => g,
        other => panic!("expected the text group, got {other:?}"),
    }
}

// The provider is process-global and the counters shared, so the whole
// scenario runs as one test.
#[test]
fn provider_contract() {
    // 1. One query per distinct (family, weight, style) — not per size,
    //    not per run.
    let before = CALLS.load(Ordering::SeqCst);
    let d = doc(
        r#"<text x="0" y="20" font-size="10">a<tspan font-size="20">b</tspan><tspan font-size="30">c</tspan></text>
<text x="0" y="60" font-size="40">d</text><text x="0" y="90" font-size="12">e</text>"#,
    );
    assert_eq!(d.root.children.len(), 3);
    assert_eq!(CALLS.load(Ordering::SeqCst) - before, 1);

    // 2. Weight and style reach the provider.
    QUERIES.lock().unwrap().clear();
    let d = doc(
        r#"<text x="0" y="20" font-size="16" font-weight="bold" font-style="italic">AB</text>"#,
    );
    {
        let q = QUERIES.lock().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(
            (q[0].family(), q[0].weight(), q[0].italic()),
            ("Known", 700, true)
        );
        assert_eq!(q[0].size_px(), 16.0);
    }
    // The fixture is upright and regular: the slant is a shear of the
    // glyph space (x grows with height above the baseline)…
    let g = text_group(&d);
    let Node::Group(first) = &g.children[0] else {
        panic!("placement group")
    };
    assert!(
        first.transform.c < -0.2 && first.transform.c > -0.22,
        "{:?}",
        first.transform
    );
    assert_eq!(
        (first.transform.a, first.transform.b, first.transform.d),
        (1.0, 0.0, 1.0)
    );
    // …and the weight a thin outline in the fill colour.
    let Node::Path(p) = &first.children[0] else {
        panic!("glyph path")
    };
    let stroke = p.stroke.as_ref().expect("emboldening outline");
    assert!((stroke.width - 16.0 * 0.03).abs() < 1e-4);

    // Regular text gets neither.
    let d = doc(r#"<text x="0" y="20" font-size="16">AB</text>"#);
    let Node::Group(first) = &text_group(&d).children[0] else {
        panic!("placement group")
    };
    assert_eq!(first.transform.c, 0.0);
    let Node::Path(p) = &first.children[0] else {
        panic!("glyph path")
    };
    assert!(p.stroke.is_none());

    // 3. Family fallback: the whole list, then each family, in order.
    QUERIES.lock().unwrap().clear();
    let d = doc(
        r#"<text x="0" y="20" font-size="16" font-family="'Missing One', Known, serif">AB</text>"#,
    );
    assert_eq!(text_group(&d).children.len(), 2);
    {
        let q = QUERIES.lock().unwrap();
        let asked: Vec<&str> = q.iter().map(|q| q.family()).collect();
        // The raw list already names `Known`, so the first query hits.
        assert_eq!(asked, ["'Missing One', Known, serif"]);
    }
    // Nothing resolvable: every candidate is tried, then `sans-serif`,
    // and the text renders nothing.
    QUERIES.lock().unwrap().clear();
    let d = doc(r#"<text x="0" y="20" font-size="16" font-family="Missing, Absent">AB</text>"#);
    assert!(text_group(&d).children.is_empty());
    {
        let q = QUERIES.lock().unwrap();
        let asked: Vec<&str> = q.iter().map(|q| q.family()).collect();
        assert_eq!(
            asked,
            ["Missing, Absent", "Missing", "Absent", "sans-serif"]
        );
    }
}
