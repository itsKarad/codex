use codex_app_server_protocol::SkillSuggestion;
use codex_app_server_protocol::SkillSuggestionNotification;
use codex_app_server_protocol::SkillSuggestionStatus;

use super::*;

#[tokio::test]
async fn skill_suggestion_notification_status_is_shown_in_the_transcript() {
    let (mut chat, mut events, _) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = chat
        .thread_id()
        .map(|thread_id| thread_id.to_string())
        .unwrap_or_else(|| "thread".to_string());
    let notification = |status, suggestions| {
        ServerNotification::SkillSuggestion(SkillSuggestionNotification {
            thread_id: thread_id.clone(),
            turn_id: "turn".to_string(),
            status,
            suggestions,
        })
    };

    chat.handle_server_notification(
        notification(
            SkillSuggestionStatus::Suggested,
            ["PDF", "Documents", "Charts", "extra"]
                .into_iter()
                .map(|name| SkillSuggestion {
                    name: name.to_string(),
                    fit_probability: 0.9,
                })
                .collect(),
        ),
        /*replay_kind*/ None,
    );
    chat.handle_server_notification(
        notification(SkillSuggestionStatus::NoMatch, Vec::new()),
        /*replay_kind*/ None,
    );
    chat.handle_server_notification(
        notification(SkillSuggestionStatus::Unavailable, Vec::new()),
        /*replay_kind*/ None,
    );

    let rendered = drain_insert_history(&mut events)
        .into_iter()
        .map(|lines| lines_to_single_string(&lines))
        .collect::<String>();
    insta::assert_snapshot!(rendered, @r"
    • Jev suggests skills: PDF (90.0%), Documents (90.0%), Charts (90.0%)

    • Jev found no high-confidence skill match.

    • Jev skill suggestions are unavailable.
    ");
}
