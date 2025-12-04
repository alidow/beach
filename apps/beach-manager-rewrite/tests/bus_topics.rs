use beach_manager_rewrite::bus::publisher::{TOPIC_ACK, TOPIC_ACTION, TOPIC_HEALTH, TOPIC_STATE};
use beach_manager_rewrite::bus::subscriber::MANAGER_TOPICS;

#[test]
fn manager_topics_match_publisher_constants() {
    let declared: std::collections::HashSet<&'static str> =
        MANAGER_TOPICS.iter().copied().collect();
    let publishers: std::collections::HashSet<&'static str> =
        [TOPIC_ACTION, TOPIC_ACK, TOPIC_STATE, TOPIC_HEALTH]
            .into_iter()
            .collect();
    assert_eq!(
        declared, publishers,
        "bus subscriber topics should match publisher constants"
    );
}
