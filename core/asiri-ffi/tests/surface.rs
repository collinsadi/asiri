//! The Swift-facing surface behaves like the engine underneath: opening, profiles, settings,
//! codes and errors. (End-to-end behaviour is covered by tools/asiri-sim.)

use std::sync::{Arc, Mutex};

use asiri_ffi::{
    AsiriCore, AsiriError, CodeKind, ConnectionState, CoreEvent, CoreListener, CoreOptions,
};

#[derive(Default)]
struct Events(Mutex<Vec<CoreEvent>>);

impl CoreListener for Events {
    fn on_event(&self, event: CoreEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn open(dir: &std::path::Path, key: u8) -> Result<Arc<AsiriCore>, AsiriError> {
    AsiriCore::open(
        CoreOptions {
            data_dir: dir.to_string_lossy().into_owned(),
            release_public_key: None,
            app_build: 1,
        },
        vec![key; 32],
        Arc::new(Events::default()),
    )
}

#[test]
fn opens_and_persists_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(dir.path(), 7).unwrap();
    let profile = core.profile().unwrap();
    assert!(!profile.registered);
    assert_eq!(profile.user_id.len(), 16);
    assert_eq!(core.connection_state(), ConnectionState::NotRegistered);
    core.set_profile("Ada".into(), None).unwrap();

    let mut settings = core.settings().unwrap();
    assert!(settings.read_receipts, "read receipts default to on");
    settings.read_receipts = false;
    settings.default_timer_seconds = 86_400;
    core.set_settings(settings.clone()).unwrap();
    let user = profile.user_id;
    drop(core);

    let core = open(dir.path(), 7).unwrap();
    assert_eq!(core.profile().unwrap().user_id, user);
    assert_eq!(core.profile().unwrap().display_name, "Ada");
    assert_eq!(core.settings().unwrap(), settings);
}

#[test]
fn a_wrong_key_or_size_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(open(dir.path(), 7).unwrap());
    assert!(matches!(
        open(dir.path(), 8),
        Err(AsiriError::Storage { .. })
    ));
    let short = AsiriCore::open(
        CoreOptions {
            data_dir: dir.path().to_string_lossy().into_owned(),
            release_public_key: None,
            app_build: 1,
        },
        vec![7; 16],
        Arc::new(Events::default()),
    );
    assert!(matches!(short, Err(AsiriError::Invalid { .. })));
}

#[test]
fn codes_are_classified_and_errors_are_plain() {
    let dir = tempfile::tempdir().unwrap();
    let core = open(dir.path(), 1).unwrap();
    core.set_profile("Ben".into(), None).unwrap();
    let offer = core.device_offer_code().unwrap();
    assert!(offer.starts_with("ASIRI1:"));
    let c = core.classify_code(offer).unwrap();
    assert_eq!(c.kind, CodeKind::DeviceOffer);
    assert_eq!(c.name, "Ben");
    assert!(matches!(
        core.classify_code("hello".into()),
        Err(AsiriError::Pairing { .. })
    ));
    // Not registered yet: no contact card, and sending needs a chat.
    assert!(core.contact_code().is_err());
    assert!(matches!(
        core.message(vec![1; 16]),
        Err(AsiriError::NotFound)
    ));
    assert!(matches!(
        core.message(vec![1; 3]),
        Err(AsiriError::Invalid { .. })
    ));
    assert!(core.chats().unwrap().is_empty());
    assert_eq!(asiri_ffi::describe_timer(604_800), "1 week");
}

#[test]
fn blurhash_round_trips() {
    let pixels: Vec<u8> = (0..32 * 32)
        .flat_map(|i| [(i % 256) as u8, 80, 160, 255])
        .collect();
    let hash = asiri_ffi::blurhash_encode(pixels, 32, 32).unwrap();
    let decoded = asiri_ffi::blurhash_decode(hash, 16, 16).unwrap();
    assert_eq!(decoded.len(), 16 * 16 * 4);
    assert!(asiri_ffi::blurhash_decode("x".into(), 16, 16).is_err());
}
