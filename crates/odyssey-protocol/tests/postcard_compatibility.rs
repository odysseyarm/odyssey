use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use odyssey_protocol::Packet;
use odyssey_protocol::accessory::{Command, Event, Reply};
use odyssey_protocol::control::device::DeviceMsg;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use odyssey_protocol::mux::MuxMsg;
use postcard_schema_ng::Schema as _;
use serde_json::Value;

fn roots() -> BTreeMap<&'static str, Value> {
    [
        ("Packet", Packet::SCHEMA),
        ("MuxMsg", MuxMsg::SCHEMA),
        ("DeviceMsg", DeviceMsg::SCHEMA),
        ("UsbMuxCtrlMsg", UsbMuxCtrlMsg::SCHEMA),
        ("AccessoryCommand", Command::SCHEMA),
        ("AccessoryReply", Reply::SCHEMA),
        ("AccessoryEvent", Event::SCHEMA),
    ]
    .map(|(name, schema)| {
        (
            name,
            serde_json::to_value(schema).expect("serialize wire schema"),
        )
    })
    .into()
}

fn compatible(old: &Value, new: &Value, path: &str) -> Result<(), String> {
    match (old, new) {
        (Value::Object(old), Value::Object(new)) if old.keys().eq(new.keys()) => {
            for (key, value) in old {
                let next = &new[key];
                let path = format!("{path}.{key}");
                if key == "Enum" {
                    let (Some(old), Some(new)) = (value.as_object(), next.as_object()) else {
                        return Err(format!("{path}: expected enum schema"));
                    };
                    if !old.keys().eq(new.keys()) {
                        return Err(format!("{path}: enum schema fields changed"));
                    }
                    for (key, value) in old {
                        if key == "variants" {
                            let (Some(old), Some(new)) = (value.as_array(), new[key].as_array())
                            else {
                                return Err(format!("{path}: expected variant list"));
                            };
                            if new.len() < old.len() {
                                return Err(format!("{path}: variants removed"));
                            }
                            for (index, (old, new)) in old.iter().zip(new).enumerate() {
                                compatible(old, new, &format!("{path}.variants[{index}]"))?;
                            }
                        } else {
                            compatible(value, &new[key], &format!("{path}.{key}"))?;
                        }
                    }
                } else {
                    compatible(value, next, &path)?;
                }
            }
            Ok(())
        }
        (Value::Array(old), Value::Array(new)) if old.len() == new.len() => {
            for (index, (old, new)) in old.iter().zip(new).enumerate() {
                compatible(old, new, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ if old == new => Ok(()),
        _ => Err(format!("{path}: wire schema changed")),
    }
}

#[test]
fn transport_roots_preserve_append_only_schema_compatibility() {
    let current = roots();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/postcard-compatibility.json");
    if std::env::var_os("UPDATE_POSTCARD_COMPATIBILITY").is_some() {
        assert!(
            std::env::var_os("CI").is_none(),
            "cannot rewrite baseline in CI"
        );
        fs::write(
            &path,
            serde_json::to_string_pretty(&current).expect("serialize schemas") + "\n",
        )
        .expect("write compatibility baseline");
        return;
    }
    let baseline: BTreeMap<String, Value> =
        serde_json::from_slice(&fs::read(path).expect("read compatibility baseline"))
            .expect("parse compatibility baseline");
    assert!(
        baseline
            .keys()
            .map(String::as_str)
            .eq(current.keys().copied()),
        "transport root set changed"
    );
    for (name, old) in baseline {
        compatible(&old, &current[name.as_str()], &name).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn variants_are_append_only_including_nested_enums() {
    let original = roots()["Packet"].clone();
    let mut appended = original.clone();
    let variants = appended
        .pointer_mut("/Struct/data/Struct/0/ty/Enum/variants")
        .expect("PacketData variants")
        .as_array_mut()
        .expect("variant array");
    let mut extra = variants[0].clone();
    extra["name"] = "Future".into();
    variants.push(extra);
    compatible(&original, &appended, "Packet").expect("appended variants preserve old payloads");
    let mut reordered = original.clone();
    reordered
        .pointer_mut("/Struct/data/Struct/0/ty/Enum/variants")
        .expect("variants")
        .as_array_mut()
        .expect("array")
        .swap(0, 1);
    assert!(compatible(&original, &reordered, "Packet").is_err());
    let mut removed = original.clone();
    removed
        .pointer_mut("/Struct/data/Struct/0/ty/Enum/variants")
        .expect("variants")
        .as_array_mut()
        .expect("array")
        .pop();
    assert!(compatible(&original, &removed, "Packet").is_err());
}

#[test]
fn existing_payload_fields_and_bounds_are_frozen() {
    let original = roots()["Packet"].clone();
    for change in 0..4 {
        let mut changed = original.clone();
        let fields = changed
            .pointer_mut("/Struct/data/Struct")
            .expect("Packet fields")
            .as_array_mut()
            .expect("fields");
        match change {
            0 => fields.push(fields[1].clone()),
            1 => fields.swap(0, 1),
            2 => fields[1]["ty"] = "U16".into(),
            _ => {
                fields.pop();
            }
        }
        assert!(
            compatible(&original, &changed, "Packet").is_err(),
            "mutation {change}"
        );
    }
    let old = serde_json::json!({"ByteArray": {"max_len": 98}});
    let new = serde_json::json!({"ByteArray": {"max_len": 99}});
    assert!(compatible(&old, &new, "VendorData.data").is_err());
}
