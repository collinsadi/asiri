//! Property tests: every frame survives an encode/decode round trip, and decoding arbitrary bytes
//! never panics (architecture §18, "Protobuf round-trips").

use asiri_proto::Message;
use asiri_proto::app::v1 as app;
use asiri_proto::relay::v1 as relay;
use proptest::prelude::*;

fn id() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 16)
}

fn send_frame() -> impl Strategy<Value = relay::ClientFrame> {
    (
        any::<u64>(),
        id(),
        0i32..6,
        id(),
        any::<u64>(),
        prop::collection::vec(id(), 0..8),
        prop::collection::vec(any::<u8>(), 0..2048),
        any::<u32>(),
    )
        .prop_map(
            |(req_id, envelope_id, kind, route, epoch, to, payload, ttl)| relay::ClientFrame {
                req_id,
                body: Some(relay::client_frame::Body::Send(relay::Send {
                    envelope_id,
                    kind,
                    group_route: route,
                    epoch,
                    to_inboxes: to.clone(),
                    payload: payload.clone(),
                    ttl_seconds: ttl,
                    welcome: (kind == relay::PayloadKind::Commit as i32).then(|| {
                        relay::AttachedWelcome {
                            envelope_id: vec![7; 16],
                            to_inboxes: to,
                            payload,
                        }
                    }),
                })),
            },
        )
}

fn text_message() -> impl Strategy<Value = app::AppMessage> {
    (id(), any::<u64>(), any::<i64>(), any::<u32>(), ".{0,200}").prop_map(
        |(message_id, sender_seq, sent_at_ms, expires_in_s, body)| app::AppMessage {
            message_id,
            sender_seq,
            sent_at_ms,
            expires_in_s,
            reply_to: vec![],
            body: Some(app::app_message::Body::Text(app::Text {
                body,
                mentions: vec![],
                link_preview: None,
            })),
        },
    )
}

proptest! {
    #[test]
    fn client_frames_round_trip(frame in send_frame()) {
        let bytes = frame.encode_to_vec();
        let back = relay::ClientFrame::decode(bytes.as_slice()).unwrap();
        prop_assert_eq!(back, frame);
    }

    #[test]
    fn app_messages_round_trip(msg in text_message()) {
        let bytes = msg.encode_to_vec();
        let back = app::AppMessage::decode(bytes.as_slice()).unwrap();
        prop_assert_eq!(back, msg);
    }

    #[test]
    fn decoding_garbage_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = relay::ClientFrame::decode(bytes.as_slice());
        let _ = relay::ServerFrame::decode(bytes.as_slice());
        let _ = app::AppMessage::decode(bytes.as_slice());
        let _ = asiri_proto::pairing::v1::PairingPayload::decode(bytes.as_slice());
    }
}

#[test]
fn unknown_fields_are_ignored() {
    // A future version adds field 99 to AppMessage; v1 must still decode the known fields.
    let mut bytes = app::AppMessage {
        message_id: vec![7; 16],
        sender_seq: 3,
        ..Default::default()
    }
    .encode_to_vec();
    bytes.extend_from_slice(&[0x9a, 0x06, 0x03, b'n', b'e', b'w']); // field 99, length-delimited
    let msg = app::AppMessage::decode(bytes.as_slice()).unwrap();
    assert_eq!(msg.sender_seq, 3);
    assert!(
        msg.body.is_none(),
        "unknown body shows the 'Update Asiri' placeholder"
    );
}
