use super::*;
use codex_app_server_protocol::ImageReference;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use pretty_assertions::assert_eq;

fn turn(id: &str, user: &str, reply: &str) -> Turn {
    Turn {
        id: id.to_string(),
        items: vec![
            ThreadItem::UserMessage {
                id: format!("{id}-user"),
                client_id: None,
                content: vec![
                    UserInput::Text {
                        text: user.to_string(),
                        text_elements: Vec::new(),
                    },
                    UserInput::Image {
                        image: ImageReference::Inline {
                            url: "attachment-data-must-not-be-forwarded".to_string(),
                        },
                        detail: None,
                    },
                ],
            },
            ThreadItem::AgentMessage {
                id: format!("{id}-reply"),
                text: reply.to_string(),
                phase: Some(MessagePhase::FinalAnswer),
                memory_citation: None,
                delivery: None,
                questions: None,
            },
        ],
        items_view: TurnItemsView::Summary,
        status: TurnStatus::Completed,
        error: None,
        started_at: None,
        completed_at: None,
        duration_ms: None,
    }
}

#[test]
fn history_contains_only_two_recent_user_and_final_reply_pairs() {
    let turns = vec![
        turn("third", "third task", "third reply"),
        turn("second", "second task", "second reply"),
        turn("first", "first task", "first reply"),
    ];

    assert_eq!(
        recent_user_reply_pairs(&turns),
        Some("User: second task\nAssistant: second reply\n\nUser: third task\nAssistant: third reply".to_string())
    );
    assert!(
        !recent_user_reply_pairs(&turns)
            .expect("history pairs")
            .contains("attachment-data-must-not-be-forwarded")
    );
}

#[test]
fn unavailable_history_uses_task_alone() {
    assert_eq!(recent_user_reply_pairs(&[]), None);
}
