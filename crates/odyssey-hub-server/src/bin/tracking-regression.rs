use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::process::ExitCode;

use odyssey_hub_server::tracking_regression::{self as regression, Capture, Error, Report, Trace};
use odyssey_sim::dataset::SceneDescription;
use odyssey_sim::scene_asset::SceneAsset;
use serde::Serialize;

const USAGE: &str = "tracking-regression dump FILE.cbor | replay TRACE.cbor [REPORT.json] | capture TRACE.cbor CAPTURE.cbor | evaluate TRACE.cbor CAPTURE.cbor [REPORT.json] | evaluate-report TRACE.cbor REPORT.json [OUTPUT.json] | suite [REPORT.json] | surface-suite [REPORT.json] | generate DIRECTORY | scenes DIRECTORY | validate-scene SCENE.json | validate-trace TRACE.cbor | validate-asset ASSET.glb | relabel-scene INPUT.json OUTPUT.json [--asset ASSET.glb] | import-recording RECORDING TEMPLATE.json OUTPUT.cbor | measure-marker-noise TRACE.cbor MODEL.json | apply-marker-noise TRACE.cbor MODEL.json SEED OUTPUT.cbor";

#[derive(Serialize)]
struct SuiteReport {
    schema_version: u32,
    passed: bool,
    reports: Vec<Report>,
}

fn load<T: serde::de::DeserializeOwned + Serialize>(path: &str) -> Result<T, Error> {
    regression::read_json(File::open(path)?)
}

fn load_cbor<T: serde::de::DeserializeOwned + Serialize>(path: &str) -> Result<T, Error> {
    regression::read_cbor(File::open(path)?)
}

fn save_cbor(value: &impl Serialize, path: &Path) -> Result<(), Error> {
    let mut writer = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    writer.write_all(&regression::to_cbor(value)?)?;
    writer.flush()?;
    Ok(())
}

fn save(value: &impl Serialize, path: Option<&Path>) -> Result<(), Error> {
    let Some(path) = path else {
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, value)?;
        stdout.write_all(b"\n")?;
        return Ok(());
    };
    let mut writer = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

async fn run(args: &[String]) -> Result<bool, Error> {
    match args {
        [command, path] if command == "dump" => {
            let value: serde_json::Value = load_cbor(path)?;
            save(&value, None)?;
            Ok(true)
        }
        [command, recording, template, output] if command == "import-recording" => {
            let template: regression::recording::RecordingTemplate = load(template)?;
            let trace = regression::recording::import_recording(Path::new(recording), template)?;
            save_cbor(&trace, Path::new(output))?;
            Ok(true)
        }
        [command, trace, model] if command == "measure-marker-noise" => {
            let trace: Trace = load_cbor(trace)?;
            let measured = regression::marker_noise::measure(&trace)?;
            save(&measured.model, Some(Path::new(model)))?;
            save(&measured, None)?;
            Ok(true)
        }
        [command, trace, model, seed, output] if command == "apply-marker-noise" => {
            let trace: Trace = load_cbor(trace)?;
            let model: regression::marker_noise::MarkerNoise = load(model)?;
            let seed = seed
                .parse()
                .map_err(|_| Error::Invalid("SEED must be an unsigned 64-bit integer".into()))?;
            let noisy = regression::marker_noise::apply(&trace, model, seed)?;
            save_cbor(&noisy, Path::new(output))?;
            Ok(true)
        }
        [command, trace] | [command, trace, _] if command == "replay" => {
            let trace: Trace = load_cbor(trace)?;
            let report = regression::replay(&trace).await?;
            save(&report, args.get(2).map(Path::new))?;
            Ok(report.passed)
        }
        [command, trace, output] if command == "capture" => {
            let trace: Trace = load_cbor(trace)?;
            let report = regression::replay(&trace).await?;
            save_cbor(&report.capture, Path::new(output))?;
            Ok(report.passed)
        }
        [command, trace, capture] | [command, trace, capture, _] if command == "evaluate" => {
            let trace: Trace = load_cbor(trace)?;
            let capture: Capture = load_cbor(capture)?;
            let report = regression::evaluate(&trace, &capture)?;
            save(&report, args.get(3).map(Path::new))?;
            Ok(report.passed)
        }
        [command, trace, report] | [command, trace, report, _] if command == "evaluate-report" => {
            let trace: Trace = load_cbor(trace)?;
            let report: Report = load(report)?;
            let evaluated = regression::evaluate(&trace, &report.capture)?;
            save(&evaluated, args.get(3).map(Path::new))?;
            Ok(evaluated.passed)
        }
        [command] | [command, _] if command == "suite" || command == "surface-suite" => {
            let traces = if command == "suite" {
                regression::scenarios::builtin_scenarios()
            } else {
                regression::scenarios::surface_cases()
            };
            let mut reports = Vec::new();
            for trace in traces {
                reports.push(regression::replay(&trace).await?);
            }
            if reports.is_empty() {
                return Err(Error::Invalid("empty built-in suite".into()));
            }
            let suite = SuiteReport {
                schema_version: regression::SCHEMA_VERSION,
                passed: reports.iter().all(|r| r.passed),
                reports,
            };
            save(&suite, args.get(1).map(Path::new))?;
            Ok(suite.passed)
        }
        [command, input, output] | [command, input, output, _, _] if command == "relabel-scene" => {
            let mut scene: SceneDescription = load(input)?;
            if let Some(path) = args.get(4) {
                if args[3] != "--asset" {
                    return Err(Error::Invalid(USAGE.into()));
                }
                let asset = SceneAsset::load(Path::new(path))
                    .map_err(|error| Error::Invalid(error.to_string()))?;
                scene
                    .relabel_with_asset(&asset)
                    .map_err(|error| Error::Invalid(error.to_string()))?;
            } else {
                scene
                    .relabel()
                    .map_err(|error| Error::Invalid(error.to_string()))?;
            }
            scene
                .provenance
                .push_str("; synthetic labels recomputed from scene geometry");
            scene
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            save(&scene, Some(Path::new(output)))?;
            Ok(true)
        }
        [command, path] if command == "validate-asset" => {
            SceneAsset::load(Path::new(path)).map_err(|error| Error::Invalid(error.to_string()))?;
            save(
                &serde_json::json!({ "valid": true, "format": "glTF 2.0 GLB" }),
                None,
            )?;
            Ok(true)
        }
        [command, path] if command == "validate-trace" => {
            let trace: Trace = load_cbor(path)?;
            trace.validate()?;
            save(
                &serde_json::json!({ "valid": true, "schema_version": trace.schema_version }),
                None,
            )?;
            Ok(true)
        }
        [command, path] if command == "validate-scene" => {
            let scene: SceneDescription = load(path)?;
            scene
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            save(
                &serde_json::json!({ "valid": true, "schema_version": scene.schema_version }),
                None,
            )?;
            Ok(true)
        }
        [command, directory] if command == "scenes" => {
            std::fs::create_dir_all(directory)?;
            let mut paths = Vec::new();
            for (name, scene) in regression::scenarios::scene_cases() {
                scene
                    .validate()
                    .map_err(|error| Error::Invalid(error.to_string()))?;
                if !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(Error::Invalid("unsafe built-in scene name".into()));
                }
                let path = Path::new(directory).join(format!("{name}.json"));
                save(&scene, Some(&path))?;
                paths.push(path);
            }
            save(
                &serde_json::json!({ "schema_version": odyssey_sim::dataset::SCENE_SCHEMA_VERSION, "scenes": paths }),
                None,
            )?;
            Ok(true)
        }
        [command, directory] if command == "generate" => {
            std::fs::create_dir_all(directory)?;
            let mut paths = Vec::new();
            let traces = regression::scenarios::builtin_scenarios()
                .into_iter()
                .chain(regression::scenarios::surface_cases());
            for trace in traces {
                trace.validate()?;
                if !trace
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(Error::Invalid("unsafe built-in trace name".into()));
                }
                let path = Path::new(directory).join(format!("{}.cbor", trace.name));
                save_cbor(&trace, &path)?;
                paths.push(path);
            }
            save(
                &serde_json::json!({ "schema_version": regression::SCHEMA_VERSION, "traces": paths }),
                None,
            )?;
            Ok(true)
        }
        _ => Err(Error::Invalid(USAGE.into())),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}",
                serde_json::json!({ "error": error.to_string() })
            );
            ExitCode::FAILURE
        }
    }
}
