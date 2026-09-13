use std::collections::BTreeSet;

use collab_core::{CollabEngine, LoroCollabEngine};

const CORPUS: &str = include_str!("../../../testing/fixtures/flow-wire-v08/corpus.json");

#[test]
fn locked_update_corpus_rejects_damage_and_converges_duplicates_and_reordering() {
    let manifest: serde_json::Value = serde_json::from_str(CORPUS).expect("locked corpus manifest parses");
    assert_eq!(manifest["schema"], "openpr.flow.wire-corpus.v0.8");
    let ids = manifest["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .map(|case| case["id"].as_str().expect("case id is a string"))
        .collect::<BTreeSet<_>>();
    for required in [
        "corrupt_update_bytes",
        "truncated_update_bytes",
        "duplicate_update_bytes",
        "out_of_order_update_bytes",
    ] {
        assert!(ids.contains(required), "locked corpus is missing {required}");
    }

    let mut source = LoroCollabEngine::new_empty(101);
    let empty_frontier = source.frontier();
    source.set_title("first").expect("first source change commits");
    let first_frontier = source.frontier();
    let first_delta = source.export_from(&empty_frontier).expect("first delta exports");
    source.set_title("second").expect("second source change commits");
    let second_delta = source.export_from(&first_frontier).expect("second delta exports");
    let source_semantic = source.semantic_snapshot().expect("source semantic snapshot");

    let mut stable = LoroCollabEngine::new_empty(202);
    stable.set_title("stable").expect("stable state commits");
    let stable_frontier = stable.frontier();
    assert!(stable.import_update(b"not-a-loro-update").is_err());
    assert_eq!(stable.frontier(), stable_frontier, "corrupt input must be atomic");
    assert!(first_delta.len() > 8, "fixture delta is large enough to truncate meaningfully");
    assert!(stable.import_update(&first_delta[..first_delta.len() / 2]).is_err());
    assert_eq!(stable.frontier(), stable_frontier, "truncated input must be atomic");

    let mut duplicate = LoroCollabEngine::new_empty(303);
    assert!(duplicate.import_update(&first_delta).expect("first import decodes").changed);
    assert!(
        !duplicate.import_update(&first_delta).expect("duplicate import decodes").changed,
        "byte-for-byte duplicate replay must be a no-op"
    );

    let mut reordered = LoroCollabEngine::new_empty(404);
    reordered
        .import_update(&second_delta)
        .expect("causally later delta may arrive first without process failure");
    reordered
        .import_update(&first_delta)
        .expect("missing predecessor later closes the causal gap");
    assert_eq!(
        reordered.semantic_snapshot().expect("reordered semantic snapshot"),
        source_semantic,
        "out-of-order delivery must converge to the exact source semantic state"
    );
}
