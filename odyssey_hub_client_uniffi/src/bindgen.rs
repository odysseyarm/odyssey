use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let output_dir = Path::new("generated");

    // Get optional override for library path from CLI argument
    let lib_path = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        if cfg!(debug_assertions) {
            PathBuf::from("../target/debug/ohc_uniffi.dll")
        } else {
            PathBuf::from("../target/release/ohc_uniffi.dll")
        }
    });

    fs::create_dir_all(output_dir).unwrap();

    let status = Command::new("uniffi-bindgen-cs")
        .args([
            "--out-dir",
            output_dir.to_str().unwrap(),
            "--library",
            lib_path.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run uniffi-bindgen-cs");

    if !status.success() {
        panic!(
            "uniffi-bindgen-cs failed with exit code: {:?}",
            status.code()
        );
    }

    let target_file = output_dir.join("ohc_uniffi.cs");

    if target_file.exists() {
        let mut contents = String::new();
        File::open(&target_file)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        let patched = contents
            .replace("internal class UniffiException", "public class UniffiException")
            .replace("internal interface IEventCallback", "public interface IEventCallback")
            .replace("internal class EventCallback", "public class EventCallback")
            .replace("internal class UserCallback", "public class UserCallback")
            .replace("internal class CallbackException", "public class CallbackException")
            .replace("internal class ClientException", "public class ClientException")
            .replace("internal struct UserObj", "public struct UserObj")
            .replace("internal struct Client", "public struct Client")
            .replace("internal record", "public record")
            .replace("internal class Device", "public class Device")
            .replace("internal class AnyhowException", "public class AnyhowException")
            .replace("uniffi.ohc_uniffi", "Radiosity.OdysseyHubClient.uniffi")
            .replace("internal interface ITrackingHistory", "public interface ITrackingHistory")
            .replace("internal class TrackingHistory", "public class TrackingHistory")
            .replace("internal enum AccessoryType", "public enum AccessoryType");

        File::create(&target_file)
            .unwrap()
            .write_all(patched.as_bytes())
            .unwrap();
    } else {
        panic!("Generated C# file not found: {:?}", target_file);
    }
}
