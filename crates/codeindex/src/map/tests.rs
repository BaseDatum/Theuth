//! The contract's proof (147 §15 C): every §4.5 type round-trips through the
//! UI fixtures byte-for-value, every fixture diagram passes the surface
//! crate's `Graph` validator, and the events deserialise as `CodeindexEvent`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::*;
use crate::event::CodeindexEvent;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../desktop/ui/src/components/codemap/__fixtures__/",
            $name
        ))
    };
}

const OVERVIEW: &str = fixture!("map-overview.json");
const OVERVIEW_STRUCTURE: &str = fixture!("map-overview-structure.json");
const ELEMENT_CONTENT_STORES: &str = fixture!("map-element-content-stores.json");
const ELEMENT_CODEINDEX: &str = fixture!("map-element-codeindex.json");
const PROGRESS: &str = fixture!("map-progress.json");

/// Parse as `T`, serialise again, and demand the same JSON value: a field the
/// type drops or renames shows up as a difference.
fn round_trip<T: Serialize + DeserializeOwned>(src: &str) -> T {
    let raw: Value = serde_json::from_str(src).unwrap();
    let typed: T = serde_json::from_value(raw.clone()).unwrap();
    let again = serde_json::to_value(&typed).unwrap();
    assert_eq!(again, raw, "serde round trip changed the fixture");
    typed
}

/// The fixture `Graph` as a json-render fence body the surface validator accepts.
fn validate_graph(g: &GraphProps, what: &str) {
    let body = serde_json::json!({
        "root": "g",
        "elements": { "g": { "type": "Graph", "props": g } }
    });
    if let Err(errs) = surface::parse(&body.to_string()) {
        panic!("{what}: {errs:?}");
    }
    assert!(
        g.nodes.len() <= DIAGRAM_NODES,
        "{what}: {} nodes",
        g.nodes.len()
    );
}

#[test]
fn overview_fixtures_round_trip() {
    let ready: MapIndex = round_trip(OVERVIEW);
    assert_eq!(ready.meta.state, MapState::Ready);
    assert_eq!(ready.meta.schema, store::MAP_SCHEMA_VERSION);
    let oa = &ready.perspectives[0];
    assert_eq!(oa.id, "overall-architecture");
    assert_eq!(oa.origin, MapOrigin::Generated);
    // The 11-node partition sits at depth 1; the perspective's root row first.
    assert_eq!(oa.tree[0].depth, 0);
    assert_eq!(oa.tree[0].id, oa.id);
    assert_eq!(oa.tree.iter().filter(|r| r.depth == 1).count(), 11);
    assert_eq!(oa.elements as usize, oa.tree.len());
    assert_eq!(
        oa.stale as usize,
        oa.tree.iter().filter(|r| r.stale).count()
    );
    assert!(oa.tree.iter().all(|r| r.depth <= MAX_DEPTH));
    // Every row's id is `<perspective>/<node>` and its parent is a row.
    for r in &oa.tree[1..] {
        assert_eq!(r.id, element_id(&oa.id, &r.node), "{}", r.id);
        let parent = r.parent.as_deref().unwrap();
        assert!(oa.tree.iter().any(|p| p.id == parent), "{parent}");
    }
    // Clusters are keyed by their members.
    let structure: MapIndex = round_trip(OVERVIEW_STRUCTURE);
    assert_eq!(structure.meta.state, MapState::Structure);
    assert!(structure.meta.job.is_none());
    let p = &structure.perspectives[0];
    assert_eq!(p.origin, MapOrigin::Structure);
    assert_eq!(
        p.tree.iter().filter(|r| r.depth == 1).count(),
        DIAGRAM_NODES + 1
    );
    assert!(
        p.tree
            .iter()
            .any(|r| r.kind == "bundle" && r.node.starts_with("bundle:"))
    );
}

#[test]
fn element_fixtures_round_trip_and_their_diagrams_validate() {
    let cs: MapElement = round_trip(ELEMENT_CONTENT_STORES);
    let ci: MapElement = round_trip(ELEMENT_CODEINDEX);
    assert_eq!(ci.parent.as_deref(), Some(cs.id.as_str()));
    assert_eq!(ci.crumbs.len(), 3);
    assert_eq!(ci.crumbs[1].0, cs.id);
    // The cluster id follows the shared rule.
    assert_eq!(cs.node, cluster_id(&cs.members));
    for e in [&cs, &ci] {
        let g = e.diagram.as_ref().unwrap();
        validate_graph(g, &e.id);
        // Diagram nodes are exactly the children.
        let mut ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut kids: Vec<&str> = e.children.iter().map(|c| c.node.as_str()).collect();
        ids.sort_unstable();
        kids.sort_unstable();
        assert_eq!(ids, kids, "{}", e.id);
        // `backed` is computed from the evidence, and dashed iff inferred.
        for edge in &e.edges {
            let backed = edge
                .evidence
                .iter()
                .any(|ev| !matches!(ev, MapEvidence::Inferred { .. }));
            assert_eq!(edge.backed, backed, "{} {}→{}", e.id, edge.from, edge.to);
            assert!(edge.label.split_whitespace().count() <= validate::EDGE_LABEL_WORDS);
            assert!(
                !edge.label.chars().any(|c| c.is_ascii_digit()),
                "{}",
                edge.label
            );
            let drawn = g
                .edges
                .iter()
                .find(|x| x.from == edge.from && x.to == edge.to)
                .unwrap();
            let dashed = drawn.style == Some(surface::GraphEdgeStyle::Dashed);
            assert_eq!(dashed, !edge.backed);
        }
        assert!(e.mermaid.as_deref().unwrap().starts_with("flowchart "));
        for c in e.children.iter().filter_map(|c| c.thumbnail.as_ref()) {
            validate_graph(c, "thumbnail");
        }
    }
    // One stale node in the module diagram, tagged for the hatch.
    assert_eq!(ci.children.iter().filter(|c| c.stale).count(), 1);
    assert_eq!(
        ci.diagram
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .filter(|n| n.tag.as_deref() == Some("stale"))
            .count(),
        1
    );
}

#[test]
fn progress_fixture_is_a_codeindex_event_sequence() {
    let raw: Vec<Value> = serde_json::from_str(PROGRESS).unwrap();
    let events: Vec<CodeindexEvent> = serde_json::from_str(PROGRESS).unwrap();
    assert_eq!(events.len(), raw.len());
    let mut last_seq = 0;
    let mut stages = Vec::new();
    for (ev, v) in events.iter().zip(&raw) {
        assert_eq!(&serde_json::to_value(ev).unwrap(), v);
        match ev {
            CodeindexEvent::CodeMapProgress { seq, stage, .. } => {
                stages.push(stage.as_str());
                assert!(*seq > last_seq);
                last_seq = *seq;
            }
            CodeindexEvent::CodeMapChanged { seq, elements, .. } => {
                assert!(!elements.is_empty());
                assert!(*seq > last_seq);
                last_seq = *seq;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(stages.first(), Some(&"select"));
    assert_eq!(stages.last(), Some(&"done"));
}

#[test]
fn ids_follow_the_grammar() {
    let members = ["unit:crates/b".to_owned(), "unit:crates/a".to_owned()];
    let id = cluster_id(&members);
    assert_eq!(id, cluster_id(&[members[1].clone(), members[0].clone()]));
    assert_eq!(id.len(), "cluster:".len() + 8);
    assert_ne!(id, cluster_id(&members[..1]));
    assert_eq!(
        element_id("overall-architecture", &id),
        format!("overall-architecture/{id}")
    );
    assert_eq!(MapOrigin::Imported.as_str(), "imported");
    assert_eq!(
        serde_json::to_string(&MapOrigin::Imported).unwrap(),
        "\"imported\""
    );
}

#[tokio::test]
async fn stubs_report_their_owner() {
    let dir = std::env::temp_dir().join(format!("codemap-stubs-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let ix = crate::Codeindex::new(crate::CodeindexConfig::default(), &dir).unwrap();
    // W1 built: an unindexed root is refused, never indexed (§10.2).
    let e = skeleton::overview(&ix, Overview { root: dir.clone() })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not indexed"), "{e}");
    let e = generator::cancel(&ix, Cancel { root: dir.clone() })
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "codemap: generator::cancel not built (147 W2)"
    );
    let e = export::export(&ix, Export { root: dir.clone() })
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "codemap: export::export not built (147 W4)");
    let _ = std::fs::remove_dir_all(&dir);
}
