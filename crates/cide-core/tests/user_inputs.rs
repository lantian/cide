//! Regression for the installed Codex 0.160 transcript: its submitted prompts are completed
//! UserMessage items, not the old user_message events. The optional audit reads an existing
//! transcript with the production parser without starting a CLI or spending model quota.
use std::path::PathBuf;

use cide_core::user_inputs::TranscriptInputs;

#[test]
fn codex_completed_messages_preserve_submissions_without_model_input_duplicates() {
    let path = std::env::temp_dir().join(format!("cide-completed-{}.jsonl", uuid::Uuid::new_v4()));
    let fixture = include_str!("fixtures/codex-completed-user-messages.jsonl");
    std::fs::write(&path, fixture).unwrap();
    let mut inputs = TranscriptInputs::default();
    assert!(inputs.refresh(&path).unwrap());
    let page = inputs.page(None);
    assert_eq!(page.total, 3);
    assert_eq!(
        page.inputs
            .iter()
            .map(|input| input.text.as_str())
            .collect::<Vec<_>>(),
        ["hi", "How are you?", "How are you?"]
    );
    assert_ne!(page.inputs[1].id, page.inputs[2].id);
    assert!(!inputs.refresh(&path).unwrap());
    let head = cide_core::sessions::transcript_head(std::io::Cursor::new(fixture));
    assert_eq!(head.prompt.as_deref(), Some("hi"));
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "read CIDE_INPUT_TRANSCRIPT with no CLI processes or model calls"]
fn existing_transcript() {
    let path =
        PathBuf::from(std::env::var_os("CIDE_INPUT_TRANSCRIPT").expect("CIDE_INPUT_TRANSCRIPT"));
    let mut inputs = TranscriptInputs::default();
    inputs.refresh(&path).unwrap();
    let page = inputs.page(None);
    assert!(
        page.total > 0,
        "the real transcript must contain submitted messages"
    );
    if let Ok(expected) = std::env::var("CIDE_INPUT_EXPECTED") {
        let expected: Vec<String> = serde_json::from_str(&expected).unwrap();
        assert_eq!(
            page.inputs
                .iter()
                .map(|input| &input.text)
                .collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );
    }
    // Consumed by the optional browser audit, so it paints the parser's actual output instead
    // of a second hand-built history array that could mask a transcript-format regression.
    println!("USER_INPUT_PAGE={}", serde_json::to_string(&page).unwrap());
}
