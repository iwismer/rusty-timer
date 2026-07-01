//! Integration test for the Race Director DBF import: a working directory of
//! committed fixture `.DBF` files → `import_participants_from_rd` → SQLite →
//! rebuilt in-memory chip lookup, resolving a known chip to bib/name/division
//! and leaving an unknown chip to the raw-chip fallback (no lookup entry).

use receiver::Db;
use receiver::control_api::{self, AppState};
use std::path::PathBuf;
use std::sync::Arc;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rd")
}

fn setup() -> Arc<AppState> {
    let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "test-receiver".to_owned());
    state
}

#[tokio::test]
async fn rd_dir_import_resolves_known_chip_with_division() {
    let state = setup();
    let dir = fixtures_dir().to_string_lossy().into_owned();

    let summary = control_api::import_participants_from_rd(&state, dir)
        .await
        .expect("RD import succeeds");
    // Three participants in RACE.DBF; bibs 1 and 2 are chipped, bib 3 is not.
    assert_eq!(summary.imported, 3);

    let lookup = state.chip_lookup.read().await;

    // Known chip → bib / "first last" / division display name.
    let entry = lookup
        .values()
        .find_map(|chips| chips.get("058003799177"))
        .expect("known chip resolves");
    assert_eq!(entry.bib, "1");
    assert_eq!(entry.name.as_deref(), Some("John Smith"));
    assert_eq!(entry.division.as_deref(), Some("5k"));

    // Latin1 + division 2 for the accented participant.
    let renee = lookup
        .values()
        .find_map(|chips| chips.get("058003799abc"))
        .expect("second chip resolves");
    assert_eq!(renee.bib, "2");
    assert_eq!(renee.name.as_deref(), Some("Renée Dupont"));
    assert_eq!(renee.division.as_deref(), Some("10k"));

    // Unknown chip → no lookup entry, so the resolver returns None and the read
    // falls back to displaying the raw chip id.
    assert!(
        lookup
            .values()
            .all(|chips| !chips.contains_key("ffffffffffff")),
        "an unknown chip must not resolve"
    );

    // Spare chip whose bib (900) has no participant row still resolves to the
    // bib so the UI/announcer can show an unknown-participant label.
    let spare = lookup
        .values()
        .find_map(|chips| chips.get("aaaaaaaaaaaa"))
        .expect("chip whose bib has no participant resolves to bib only");
    assert_eq!(spare.bib, "900");
    assert_eq!(spare.name, None);
    assert_eq!(spare.division, None);
}

#[tokio::test]
async fn rd_import_missing_directory_is_rejected_without_mutation() {
    let state = setup();
    // Seed a good import so we can prove a failed import leaves it intact.
    control_api::import_participants_from_rd(&state, fixtures_dir().to_string_lossy().into_owned())
        .await
        .expect("seed import");

    let bad_dir = fixtures_dir().join("does-not-exist");
    let err =
        control_api::import_participants_from_rd(&state, bad_dir.to_string_lossy().into_owned())
            .await
            .expect_err("missing dir rejected");
    assert!(matches!(err, receiver::ReceiverError::BadRequest(_)));

    // The prior import is untouched.
    let lookup = state.chip_lookup.read().await;
    assert!(
        lookup
            .values()
            .any(|chips| chips.contains_key("058003799177"))
    );
}
