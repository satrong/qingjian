//! 上下文协议的旧消息兼容与双向文本传输。

use super::{ClientMessage, InputSettings, SessionId, read_message, write_message};

#[test]
fn old_surrounding_message_defaults_to_empty_after() {
    let message: ClientMessage =
        serde_json::from_str(r#"{"Surrounding":{"session":1,"text":"今天"}}"#).unwrap();
    assert_eq!(
        message,
        ClientMessage::Surrounding {
            session: SessionId(1),
            text: "今天".into(),
            after: String::new(),
        }
    );
}

#[test]
fn surrounding_round_trips_both_sides() {
    let message = ClientMessage::Surrounding {
        session: SessionId(1),
        text: "今天𠮷".into(),
        after: "😀明天".into(),
    };
    let mut bytes = Vec::new();
    write_message(&mut bytes, &message).unwrap();
    assert_eq!(
        read_message::<_, ClientMessage>(&mut bytes.as_slice()).unwrap(),
        Some(message)
    );
}

#[test]
fn old_input_settings_keep_default_context_window() {
    let mut value = serde_json::to_value(InputSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("context_before");
    value.as_object_mut().unwrap().remove("context_after");
    let settings: InputSettings = serde_json::from_value(value).unwrap();
    assert_eq!((settings.context_before, settings.context_after), (64, 32));
}
