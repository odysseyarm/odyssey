use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div};
use odyssey_hub_client::client::DongleInfo;
use odyssey_hub_server::firmware::{self, FirmwareEntry, FirmwareManifest};

use super::update_check::{desktop_version, format_version};
use crate::components::ui::{ButtonKind, button, card, card_title};
use crate::theme::Palette;

const RELEASE_URL: &str = "https://github.com/odysseyarm/odyssey/releases/tag/dongle-v";

/// The dongle entry worth announcing for a dongle running `installed`. A dongle
/// that reported no version (`[0, 0, 0]`) gets none, as an unread device does.
fn offered(manifest: &FirmwareManifest, installed: [u16; 3]) -> Option<&FirmwareEntry> {
    if installed == [0; 3] {
        return None;
    }
    let entry = firmware::select_firmware(manifest, "dongle", desktop_version(), 0).offered?;
    firmware::version_newer(installed, entry.version_parts()?).then_some(entry)
}

/// A card announcing a newer dongle firmware, or `None` when it is current.
///
/// Dongles keep their makers' bootloaders (Fanstel's or Laird's, both Nordic
/// serial DFU), which only accept a package signed with that maker's key, so the
/// card links to the release rather than installing.
pub(crate) fn dongle_update_card(
    dongle: &DongleInfo,
    manifest: Option<&FirmwareManifest>,
    p: Palette,
) -> Option<AnyElement> {
    let entry = offered(manifest?, dongle.firmware_version)?;
    let version = entry.version.clone();
    let mut card = card(p)
        .gap_1()
        .child(card_title(p, "Dongle update available"))
        .child(div().text_sm().child(format!(
            "Dongle {} runs v{}; v{version} is available.",
            dongle.id,
            format_version(dongle.firmware_version)
        )))
        .child(div().text_sm().text_color(p.fg_muted).child(
            "Flash it with Nordic's nrfutil, using the package that matches your dongle's maker: \
             -dfu-fanstel.zip or -dfu-laird.zip. The other one is rejected.",
        ));
    // Offline mode forbids offering a link to the web.
    card = if odyssey_hub_server::policy::offline() {
        card.child(
            div()
                .text_xs()
                .text_color(p.fg_muted)
                .child("Ask your Odyssey provider for it."),
        )
    } else {
        let url = format!("{RELEASE_URL}{version}");
        card.child(
            button(
                format!("dongle-release-{}", dongle.id),
                "Open release",
                ButtonKind::Secondary,
                p,
            )
            .on_click(move |_, _, cx| cx.open_url(&url)),
        )
    };
    Some(card.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(entries: &[(&str, &str)]) -> FirmwareManifest {
        let firmwares: Vec<_> = entries
            .iter()
            .map(|(version, min_desktop)| {
                serde_json::json!({
                    "version": version,
                    "requires_bootloader": 0,
                    "requires_netcore": 0,
                    "min_desktop": min_desktop,
                    "files": {},
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "devices": { "dongle": { "firmwares": firmwares } },
        }))
        .unwrap()
    }

    #[test]
    fn newer_release_is_offered_and_current_or_unread_is_not() {
        let manifest = manifest(&[("0.2.0", "0.0.0")]);
        for (installed, want) in [
            ([0, 1, 11], Some("0.2.0")),
            ([0, 2, 0], None),
            ([0, 3, 0], None),
            ([0, 0, 0], None),
        ] {
            let got = offered(&manifest, installed).map(|entry| entry.version.as_str());
            assert_eq!(got, want, "{installed:?}");
        }
    }

    #[test]
    fn release_needing_a_newer_desktop_is_not_offered() {
        let manifest = manifest(&[("0.2.0", "99.0.0")]);
        assert!(offered(&manifest, [0, 1, 0]).is_none());
    }

    #[test]
    fn an_older_installable_entry_is_offered_when_the_newest_needs_a_newer_desktop() {
        let manifest = manifest(&[("0.3.0", "99.0.0"), ("0.2.0", "0.0.0")]);
        let got = offered(&manifest, [0, 1, 0]).map(|entry| entry.version.as_str());
        assert_eq!(got, Some("0.2.0"));
    }
}
