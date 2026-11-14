use std::env;
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Usage: cs_bindgen [library path] [output dir]
    let lib_path = env::args().nth(1).map_or_else(
        || {
            if cfg!(debug_assertions) {
                PathBuf::from("target/debug/ohc_uniffi.dll")
            } else {
                PathBuf::from("target/release/ohc_uniffi.dll")
            }
        },
        PathBuf::from,
    );

    // Output dir must be the one the csproj compiles (OdysseyHubClient/generated)
    let output_dir = env::args()
        .nth(2)
        .map_or_else(|| PathBuf::from("generated"), PathBuf::from);
    let output_dir: &Path = &output_dir;

    fs::create_dir_all(output_dir)?;

    let status = Command::new("uniffi-bindgen-cs")
        .args([
            "--no-format",
            "--out-dir",
            output_dir.to_str().ok_or("output directory path is not UTF-8")?,
            "--library",
            lib_path.to_str().ok_or("library path is not UTF-8")?,
        ])
        .status()?;

    assert!(
        status.success(),
        "uniffi-bindgen-cs failed with exit code: {:?}",
        status.code()
    );

    let target_file = output_dir.join("ohc_uniffi.cs");

    if target_file.exists() {
        let mut contents = String::new();
        File::open(&target_file)?.read_to_string(&mut contents)?;

        let mut patched = contents
            .replace(
                "internal class UniffiException",
                "public class UniffiException",
            )
            .replace(
                "internal interface IEventCallback",
                "public interface IEventCallback",
            )
            .replace("internal class EventCallback", "public class EventCallback")
            .replace("internal class UserCallback", "public class UserCallback")
            .replace(
                "internal class CallbackException",
                "public class CallbackException",
            )
            .replace(
                "internal class ClientException",
                "public class ClientException",
            )
            .replace("internal struct UserObj", "public struct UserObj")
            .replace("internal struct Client", "public struct Client")
            .replace("internal record", "public record")
            .replace("internal class Device", "public class Device")
            .replace(
                "internal interface IRpcFailure",
                "public interface IRpcFailure",
            )
            .replace("internal class RpcFailure", "public class RpcFailure")
            .replace(
                "internal interface IConnectionFailure",
                "public interface IConnectionFailure",
            )
            .replace(
                "internal class ConnectionFailure",
                "public class ConnectionFailure",
            )
            .replace("uniffi.ohc_uniffi", "OdysseyArm.HubClient.uniffi")
            // Fully-qualified cross-crate refs must escape the renamed namespace:
            // inside OdysseyArm.HubClient.uniffi, bare `uniffi.` binds to the
            // namespace's own `uniffi` segment instead of the global namespace.
            .replace(
                "uniffi.odyssey_hub_common.",
                "global::uniffi.odyssey_hub_common.",
            )
            .replace(
                "internal interface ITrackingHistory",
                "public interface ITrackingHistory",
            )
            .replace(
                "internal class TrackingHistory",
                "public class TrackingHistory",
            )
            .replace("internal enum AccessoryType", "public enum AccessoryType");

        // Import the shared common bindings, and their BigEndianStream, ahead of the namespace.
        if !patched.contains("using uniffi.odyssey_hub_common;") {
            patched = patched.replace(
                "namespace OdysseyArm.HubClient.uniffi;",
                "using uniffi.odyssey_hub_common;\nusing BigEndianStream = uniffi.odyssey_hub_common.BigEndianStream;\nnamespace OdysseyArm.HubClient.uniffi;",
            );
        }

        // Rename the local BigEndianStream so the alias above resolves to the common one.
        // Two distinct BigEndianStream types would not match across the two binding files.
        patched = patched.replace(
            "static class BigEndianStreamExtensions",
            "static class LocalBigEndianStreamExtensions",
        );
        patched = patched.replace("class BigEndianStream", "class LocalBigEndianStream");
        patched = patched.replace(
            "public BigEndianStream(Stream stream)",
            "public LocalBigEndianStream(Stream stream)",
        );

        File::create(&target_file)?.write_all(patched.as_bytes())?;
    } else {
        return Err(format!("Generated C# file not found: {}", target_file.display()).into());
    }

    // uniffi-bindgen-cs emits Transport and EventsTransport as internal, but the SDK exposes them.
    let common_file = output_dir.join("odyssey_hub_common.cs");
    if common_file.exists() {
        let mut contents = String::new();
        File::open(&common_file)?.read_to_string(&mut contents)?;
        let patched = contents
            .replace("internal enum Transport", "public enum Transport")
            .replace(
                "internal enum EventsTransport",
                "public enum EventsTransport",
            );
        File::create(&common_file)?.write_all(patched.as_bytes())?;
    }
    Ok(())
}
