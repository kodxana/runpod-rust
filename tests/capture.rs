use runpod::serverless::capture::{CaptureMode, MAX_CAPTURED_CHARS, RingBuffer, clip};
use std::borrow::Cow;

#[test]
fn ring_buffer_keeps_tail() {
    let mut buffer = RingBuffer::new(10);
    assert_eq!(buffer.write("0123456789ABCDEF"), 16);
    assert_eq!(buffer.as_str(), "6789ABCDEF");
}

#[test]
fn clip_keeps_head_and_tail() {
    let text = format!("{}{}", "A".repeat(100), "B".repeat(100));
    let clipped = clip(&text, 40);
    assert!(clipped.starts_with(&"A".repeat(20)));
    assert!(clipped.ends_with(&"B".repeat(20)));
    assert!(clipped.contains("160 characters truncated"));
    assert!(clipped.len() < text.len());
}

#[test]
fn clip_passthrough_when_small() {
    assert!(matches!(clip("short", 100), Cow::Borrowed("short")));
}

#[test]
fn auto_is_the_default() {
    assert_eq!(CaptureMode::default(), CaptureMode::Auto);
    assert!(!CaptureMode::Auto.handler_capture_enabled());
}

#[test]
fn unknown_mode_falls_back_to_auto() {
    assert_eq!(CaptureMode::from("yes-please"), CaptureMode::Auto);
}

#[test]
fn default_buffer_bounds_unicode_characters_across_writes() {
    let mut buffer = RingBuffer::default();
    buffer.write(&"\u{1f680}".repeat(MAX_CAPTURED_CHARS));
    assert_eq!(buffer.write("tail"), 4);
    assert_eq!(buffer.as_str().chars().count(), MAX_CAPTURED_CHARS);
    assert!(buffer.as_str().ends_with("tail"));
}

#[test]
fn python_capture_fixtures() {
    let cases: Vec<runpod::Value> =
        serde_json::from_str(include_str!("fixtures/capture.json")).unwrap();
    for case in cases {
        match case["operation"].as_str().unwrap() {
            "buffer" => {
                let limit = usize::try_from(case["limit"].as_u64().unwrap()).unwrap();
                let mut buffer = RingBuffer::new(limit);
                for (text, expected) in case["writes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(case["output"].as_array().unwrap())
                {
                    let written = buffer.write(text.as_str().unwrap());
                    assert_eq!(runpod::json!(written), expected["written"], "{case}");
                    assert_eq!(
                        buffer.as_str(),
                        expected["value"].as_str().unwrap(),
                        "{case}"
                    );
                }
            }
            "clip" => {
                let limit = usize::try_from(case["limit"].as_u64().unwrap()).unwrap();
                assert_eq!(
                    clip(case["input"].as_str().unwrap(), limit),
                    case["output"].as_str().unwrap(),
                    "{case}"
                );
            }
            "mode" => {
                let mode = case["input"]
                    .as_str()
                    .map(CaptureMode::from)
                    .unwrap_or_default();
                let expected = match case["output"].as_str().unwrap() {
                    "all" => CaptureMode::All,
                    "off" => CaptureMode::Off,
                    "auto" => CaptureMode::Auto,
                    other => panic!("unexpected capture mode {other}"),
                };
                assert_eq!(mode, expected, "{case}");
                assert_eq!(
                    mode.handler_capture_enabled(),
                    case["handler_enabled"].as_bool().unwrap()
                );
            }
            other => panic!("unexpected capture operation {other}"),
        }
    }
}
