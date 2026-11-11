//! Accessory commands through the running hub, the calls Odyssey Desktop makes.

use clap::Subcommand;
use eros::Context as _;
use futures::StreamExt as _;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::accessory::{
    AccessoryInfo, AccessoryMap, AccessoryRadio, AccessoryRadioInfo, AccessoryType,
};

use crate::usb_discovery::format_bd_addr;

#[derive(Subcommand)]
pub(crate) enum AccessoryCommands {
    /// List radios, their connections and nearby accessories
    Radios,
    /// List configured accessories
    List,
    /// Print radios and accessories, then every change, until Ctrl-C
    Watch,
    /// Enroll a marker control board that has its pairing window open
    Enroll {
        /// Address as shown by `radios`, e.g. C0:48:4D:00:00:01
        address: String,
        /// `system`, `dongle:<id>` or `native:<id>`, as shown by `radios`
        #[arg(long, default_value = "system")]
        radio: String,
        #[arg(long, default_value = "Marker Control Board")]
        name: String,
    },
    /// Reset a marker control board to Auto mode
    ResetMode { address: String },
    /// Remove an accessory from the configuration
    Forget { address: String },
}

pub(crate) async fn handle_command(command: AccessoryCommands) -> eros::Result<()> {
    let client = Client::connect().await.context("connecting to the hub")?;
    match command {
        AccessoryCommands::Radios => print_radios(&client.get_accessory_radios().await?),
        AccessoryCommands::List => print_accessories(&client.get_accessory_map().await?),
        AccessoryCommands::Watch => return watch(&client).await,
        AccessoryCommands::Enroll {
            address,
            radio,
            name,
        } => {
            let mut info = AccessoryInfo::with_defaults(name, AccessoryType::MarkerControlBoard);
            info.radio = parse_radio(&radio)?;
            client
                .enroll_accessory(parse_address(&address)?, info)
                .await
                .context("enrolling")?;
            println!("Enrolled {address}");
        }
        AccessoryCommands::ResetMode { address } => {
            client
                .reset_marker_mode(parse_address(&address)?)
                .await
                .context("resetting marker mode")?;
            println!("Reset {address} to Auto");
        }
        AccessoryCommands::Forget { address } => {
            let address = parse_address(&address)?;
            let mut map: std::collections::HashMap<_, _> = client
                .get_accessory_map()
                .await?
                .into_iter()
                .map(|(address, (info, _))| (address, info))
                .collect();
            if map.remove(&address).is_none() {
                eros::bail!("{} is not configured", format_bd_addr(address));
            }
            client.update_accessory_info_map(map).await?;
            println!("Forgot {}", format_bd_addr(address));
        }
    }
    Ok(())
}

async fn watch(client: &Client) -> eros::Result<()> {
    let mut radios = client.subscribe_accessory_radios().await?;
    let mut accessories = client.subscribe_accessory_map().await?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            update = radios.next() => if let Some(update) = update {
                print_radios(&update?);
            } else {
                eros::bail!("hub closed the radio stream");
            },
            update = accessories.next() => if let Some(update) = update {
                print_accessories(&update?);
            } else {
                eros::bail!("hub closed the accessory stream");
            },
        }
    }
}

fn print_radios(radios: &[AccessoryRadioInfo]) {
    println!("{} radio(s):", radios.len());
    for radio in radios {
        println!(
            "  {} ({}){}{}",
            radio.name,
            radio_value(&radio.radio),
            if radio.available { "" } else { " unavailable" },
            radio
                .error
                .as_ref()
                .map_or_else(String::new, |error| format!(" error: {error}")),
        );
        for connection in &radio.connections {
            println!(
                "    {} {:?}{}",
                format_bd_addr(connection.address),
                connection.state,
                connection
                    .error
                    .as_ref()
                    .map_or_else(String::new, |error| format!(": {error}")),
            );
        }
        for nearby in &radio.nearby {
            let window = match nearby.pairing_window_expires_at_ms {
                None => "pairing window unknown".to_owned(),
                Some(0) => "pairing window closed".to_owned(),
                Some(expires) => format!("pairing window open until {expires} ms"),
            };
            println!(
                "    nearby {} {} ({}), {window}",
                format_bd_addr(nearby.address),
                nearby.name,
                nearby.ty.name(),
            );
        }
    }
}

fn print_accessories(map: &AccessoryMap) {
    println!(
        "{} accessor{}:",
        map.len(),
        if map.len() == 1 { "y" } else { "ies" }
    );
    for (address, (info, connected)) in map {
        println!(
            "  {} {} ({}) on {}{}",
            format_bd_addr(*address),
            info.name,
            info.ty.name(),
            radio_value(&info.radio),
            if *connected { ", connected" } else { "" },
        );
    }
}

fn radio_value(radio: &AccessoryRadio) -> String {
    match radio {
        AccessoryRadio::System => "system".to_owned(),
        AccessoryRadio::Native { id } => format!("native:{id}"),
        AccessoryRadio::Dongle { id } => format!("dongle:{id}"),
    }
}

fn parse_radio(value: &str) -> eros::Result<AccessoryRadio> {
    match value.split_once(':') {
        None if value == "system" => Ok(AccessoryRadio::System),
        Some(("dongle", id)) if !id.is_empty() => Ok(AccessoryRadio::Dongle { id: id.into() }),
        Some(("native", id)) if !id.is_empty() => Ok(AccessoryRadio::Native { id: id.into() }),
        _ => eros::bail!("radio must be system, dongle:<id> or native:<id>, not {value:?}"),
    }
}

fn parse_address(value: &str) -> eros::Result<[u8; 6]> {
    let mut address = [0; 6];
    let mut parts = value.split(':');
    for byte in &mut address {
        let part = parts.next().unwrap_or_default();
        *byte = u8::from_str_radix(part, 16)
            .ok()
            .filter(|_| part.len() == 2)
            .with_context(|| {
                format!("address must be six hex bytes like C0:48:4D:00:00:01, not {value:?}")
            })?;
    }
    if parts.next().is_some() {
        eros::bail!("address must be six hex bytes, not {value:?}");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip_and_reject_malformed_input() {
        let address = [0xC0, 0x48, 0x4D, 0x00, 0x00, 0x01];
        assert_eq!(parse_address(&format_bd_addr(address)).unwrap(), address);
        for bad in [
            "C0:48:4D:00:00",
            "C0:48:4D:00:00:01:02",
            "C0:48:4D:00:00:1",
            "C0:48:4D:00:00:G1",
        ] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn radios_round_trip() {
        for radio in [
            AccessoryRadio::System,
            AccessoryRadio::Dongle {
                id: "306B466645BC2184".into(),
            },
            AccessoryRadio::Native { id: "hci0".into() },
        ] {
            assert_eq!(parse_radio(&radio_value(&radio)).unwrap(), radio);
        }
        parse_radio("dongle:").unwrap_err();
        parse_radio("usb:1").unwrap_err();
    }
}
