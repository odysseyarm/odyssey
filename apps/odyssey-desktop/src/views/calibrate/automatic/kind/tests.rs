//! Tests for the display findings and the size plan, which share fixtures.

use odyssey_autocalib::display::{
    DisplayLookup, EdidInfo, LookupNote, LookupSource, PhysicalSize, Reliability, SizePrecision,
    SuspectReason, parse_edid,
};
use odyssey_autocalib::session::{KnownSize, SizeSource};

use super::finding::FindingKind;
use super::*;
use crate::views::calibrate::automatic::settings::Entered;

fn lookup(info: Option<EdidInfo>, note: Option<LookupNote>) -> DisplayLookup {
    DisplayLookup {
        edid: info,
        source: LookupSource::Unavailable,
        note,
    }
}

fn edid(physical_size: PhysicalSize, reliability: Reliability) -> EdidInfo {
    EdidInfo {
        manufacturer: "TST".into(),
        product_code: 1,
        monitor_name: Some("TEST TV".into()),
        native_resolution: Some((1920, 1080)),
        refresh_hz: Some(60.0),
        physical_size,
        reliability,
        checksums_valid: true,
    }
}

fn dims(width_mm: u32, height_mm: u32, precision: SizePrecision) -> PhysicalSize {
    PhysicalSize::Dimensions {
        width_mm,
        height_mm,
        precision,
    }
}

#[test]
fn a_reliable_edid_becomes_a_declared_panel_size() {
    let f = Finding::from_lookup(&lookup(
        Some(edid(
            dims(1210, 680, SizePrecision::Millimeters),
            Reliability::Reliable,
        )),
        None,
    ));
    assert!(f.is_found());
    assert_eq!(
        f.known_size(),
        Some(KnownSize::Dimensions {
            width_m: 1.21,
            height_m: 0.68
        })
    );
    assert_eq!(
        f.headline(),
        "We read the screen size from the TV itself: 55 inches (121 × 68 cm)"
    );
    assert!(f.advice().is_none());
}

#[test]
fn precision_shows_only_in_the_details() {
    let mm = Finding::from_lookup(&lookup(
        Some(edid(
            dims(1210, 680, SizePrecision::Millimeters),
            Reliability::Reliable,
        )),
        None,
    ));
    let cm = Finding::from_lookup(&lookup(
        Some(edid(
            dims(1210, 680, SizePrecision::Centimeters),
            Reliability::Reliable,
        )),
        None,
    ));
    assert!(!mm.headline().to_lowercase().contains("precision"));
    assert!(mm.detail_lines().iter().any(|l| l.contains("millimeters")));
    assert!(cm.detail_lines().iter().any(|l| l.contains("5 mm")));
}

#[test]
fn a_suspect_edid_is_not_an_anchor_and_says_why() {
    let reasons = [
        SuspectReason::AspectPlaceholder,
        SuspectReason::TooSmall,
        SuspectReason::TooLarge,
        SuspectReason::AspectMismatch {
            size_aspect: 1.6,
            native_aspect: 1.78,
        },
    ];
    for reason in reasons {
        let f = Finding::from_lookup(&lookup(
            Some(edid(
                dims(160, 90, SizePrecision::Centimeters),
                Reliability::Suspect(reason),
            )),
            None,
        ));
        assert!(matches!(f.kind, FindingKind::Doubtful { .. }), "{reason:?}");
        assert_eq!(f.known_size(), None);
        assert!(f.advice().is_some());
        assert!(
            f.detail_lines()
                .iter()
                .any(|l| l.starts_with("Not used because")),
            "{reason:?}"
        );
    }
}

#[test]
fn displays_without_a_size_are_not_guessed_to_be_projectors() {
    for physical in [PhysicalSize::Undefined, PhysicalSize::AspectOnly(1.78)] {
        let f = Finding::from_lookup(&lookup(
            Some(edid(physical, Reliability::NoDimensions)),
            None,
        ));
        assert_eq!(f.kind, FindingKind::NoSize);
        assert_eq!(f.known_size(), None);
        assert!(f.projector_aside().is_some());
        assert_eq!(default_kind(None), DisplayKind::Flat);
    }
}

#[test]
fn a_missing_edid_names_the_lookup_note() {
    for note in [
        LookupNote::NotFound,
        LookupNote::NoMonitorName,
        LookupNote::UnsupportedPlatform,
        LookupNote::Ambiguous { candidates: 2 },
        LookupNote::NoNativeResolution,
        LookupNote::ResolutionMismatch {
            native: (3840, 2160),
            screen: (1280, 1024),
        },
        LookupNote::Unreadable(std::io::ErrorKind::PermissionDenied),
    ] {
        let f = Finding::from_lookup(&lookup(None, Some(note.clone())));
        assert_eq!(f.kind, FindingKind::Missing(note));
        assert!(f.advice().is_some() && f.projector_aside().is_none());
    }
    assert!(matches!(
        Finding::from_lookup(&lookup(None, None)).kind,
        FindingKind::Missing(LookupNote::NotFound)
    ));
}

#[test]
fn a_parsed_projector_style_edid_reads_as_no_size() {
    // Both size bytes zero, as a projector reports.
    let mut bytes = vec![0u8; 128];
    bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    bytes[18] = 1;
    bytes[19] = 4;
    let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    bytes[127] = 0u8.wrapping_sub(sum);
    let info = parse_edid(&bytes).unwrap();
    assert_eq!(
        Finding::from_lookup(&lookup(Some(info), None)).kind,
        FindingKind::NoSize
    );
}

#[test]
fn the_remembered_kind_wins_and_flat_is_the_default() {
    assert_eq!(default_kind(None), DisplayKind::Flat);
    assert_eq!(
        default_kind(Some(DisplayKind::Projector)),
        DisplayKind::Projector
    );
    assert_eq!(default_kind(Some(DisplayKind::Flat)), DisplayKind::Flat);
}

fn found() -> Finding {
    Finding::from_lookup(&lookup(
        Some(edid(
            dims(1210, 680, SizePrecision::Millimeters),
            Reliability::Reliable,
        )),
        None,
    ))
}

#[test]
fn flat_screens_anchor_to_the_panel_and_require_their_edges() {
    let p = plan(
        DisplayKind::Flat,
        Some(&found()),
        None,
        16.0 / 9.0,
        false,
        None,
    );
    assert_eq!(
        p.known_size,
        Some(KnownSize::Dimensions {
            width_m: 1.21,
            height_m: 0.68
        })
    );
    assert_eq!(p.origin, Some(SizeOrigin::Panel));
    assert!(p.require_visible_edges);
}

#[test]
fn a_typed_size_beats_the_panel() {
    let p = plan(
        DisplayKind::Flat,
        Some(&found()),
        Some(Entered::Diagonal { inches: 40.0 }),
        16.0 / 9.0,
        false,
        None,
    );
    assert_eq!(p.known_size, Some(KnownSize::Diagonal { inches: 40.0 }));
    assert_eq!(p.origin, Some(SizeOrigin::Entered));
    assert!(p.require_visible_edges);
}

#[test]
fn preferring_the_camera_plans_a_flat_screen_like_one_whose_edid_said_nothing() {
    let p = plan(
        DisplayKind::Flat,
        Some(&found()),
        None,
        16.0 / 9.0,
        true,
        None,
    );
    assert_eq!((p.known_size, p.origin), (None, None));
    assert!(!p.require_visible_edges);
}

#[test]
fn a_typed_size_still_wins_when_the_camera_is_preferred() {
    let p = plan(
        DisplayKind::Flat,
        Some(&found()),
        Some(Entered::Diagonal { inches: 40.0 }),
        16.0 / 9.0,
        true,
        None,
    );
    assert_eq!(p.known_size, Some(KnownSize::Diagonal { inches: 40.0 }));
    assert_eq!(p.origin, Some(SizeOrigin::Entered));
    assert!(p.require_visible_edges);
}

#[test]
fn a_flat_screen_without_a_size_is_measured_and_may_crop_its_picture() {
    let none = Finding::from_lookup(&lookup(None, Some(LookupNote::NotFound)));
    let p = plan(
        DisplayKind::Flat,
        Some(&none),
        None,
        16.0 / 9.0,
        false,
        None,
    );
    assert_eq!((p.known_size, p.origin), (None, None));
    assert!(!p.require_visible_edges);
    assert_eq!(
        plan(DisplayKind::Flat, None, None, 16.0 / 9.0, false, None).known_size,
        None
    );
}

fn by_hand() -> KnownSize {
    KnownSize::Dimensions {
        width_m: 1.2,
        height_m: 0.675,
    }
}

#[test]
fn a_hand_measurement_beats_the_panel_and_the_camera_and_needs_no_edges() {
    for prefer_camera in [false, true] {
        let p = plan(
            DisplayKind::Flat,
            Some(&found()),
            None,
            16.0 / 9.0,
            prefer_camera,
            Some(by_hand()),
        );
        assert_eq!(p.known_size, Some(by_hand()));
        assert_eq!(p.origin, Some(SizeOrigin::Reference));
        assert!(!p.require_visible_edges);
    }
    let projected = plan(
        DisplayKind::Projector,
        None,
        None,
        16.0 / 9.0,
        false,
        Some(by_hand()),
    );
    assert_eq!(projected.origin, Some(SizeOrigin::Reference));
    assert!(!projected.require_visible_edges);
}

#[test]
fn a_typed_size_still_beats_a_hand_measurement() {
    let p = plan(
        DisplayKind::Flat,
        Some(&found()),
        Some(Entered::Diagonal { inches: 40.0 }),
        16.0 / 9.0,
        false,
        Some(by_hand()),
    );
    assert_eq!(p.origin, Some(SizeOrigin::Entered));
    assert!(p.require_visible_edges);
}

#[test]
fn projectors_ignore_the_display_and_never_require_edges() {
    let p = plan(
        DisplayKind::Projector,
        Some(&found()),
        None,
        16.0 / 9.0,
        false,
        None,
    );
    assert_eq!(
        (p.known_size, p.origin, p.require_visible_edges),
        (None, None, false)
    );
    let typed = plan(
        DisplayKind::Projector,
        Some(&found()),
        Some(Entered::Diagonal { inches: 100.0 }),
        16.0 / 9.0,
        false,
        None,
    );
    assert_eq!(
        typed.known_size,
        Some(KnownSize::Diagonal { inches: 100.0 })
    );
    assert!(!typed.require_visible_edges);
}

#[test]
fn a_typed_width_takes_its_height_from_the_pictures_shape() {
    let p = plan(
        DisplayKind::Projector,
        None,
        Some(Entered::Width { cm: 240.0 }),
        16.0 / 9.0,
        false,
        None,
    );
    let Some(KnownSize::Dimensions { width_m, height_m }) = p.known_size else {
        panic!("{p:?}")
    };
    assert!(
        (width_m - 2.4).abs() < 1e-9 && (height_m - 1.35).abs() < 1e-9,
        "{width_m} x {height_m}"
    );
}

#[test]
fn the_size_source_is_told_in_plain_words() {
    assert_eq!(
        source_label(SizeSource::KnownDimensions, Some(SizeOrigin::Panel)),
        "From the TV's own information"
    );
    assert_eq!(
        source_label(SizeSource::KnownDiagonal, Some(SizeOrigin::Entered)),
        "From the size you entered"
    );
    assert_eq!(
        source_label(SizeSource::KnownDimensions, Some(SizeOrigin::Entered)),
        "From the size you entered"
    );
    assert_eq!(
        source_label(SizeSource::Measured, None),
        "Measured by the camera"
    );
}

#[test]
fn the_demo_finding_is_marked_as_a_demo() {
    let f = Finding::demo(1.2, 0.675);
    assert!(f.demo && f.is_found());
    assert!(f.headline().starts_with("Demo:"));
}
