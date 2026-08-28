use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolOutput;
use codex_tools::ToolPayload;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::HistoryNotesAction;
use super::HistoryNotesToolOutput;

#[test]
fn marks_sensitive_history_notes_arguments_encrypted() {
    for (action, field) in [
        (HistoryNotesAction::HistorySearchContents, "query"),
        (HistoryNotesAction::NotesSearchContents, "query"),
        (HistoryNotesAction::NotesAppendToFile, "text"),
        (HistoryNotesAction::NotesWriteFile, "text"),
    ] {
        let parameters = action.parameters();
        assert_eq!(parameters["properties"][field]["encrypted"], true);
    }
}

#[test]
fn preserves_encrypted_history_output() {
    let result = HistoryNotesToolOutput {
        result: json!({"encrypted_output": "enc_payload"}),
    }
    .to_response_item(
        "call-1",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );

    let ResponseInputItem::FunctionCallOutput { output, .. } = result else {
        panic!("expected function-call output");
    };
    assert_eq!(
        output.content_items(),
        Some(
            [FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "enc_payload".to_string(),
            }]
            .as_slice()
        )
    );
}
