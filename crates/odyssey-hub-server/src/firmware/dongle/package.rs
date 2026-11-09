use std::io::{Cursor, Read as _};

use serde::Deserialize;

use super::DongleUpdateError;
use super::nordic::Package;

const MAX_APPLICATION: u64 = super::layout::APPLICATION_SIZE as u64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    manifest: ApplicationManifest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationManifest {
    application: Application,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Application {
    bin_file: String,
    dat_file: String,
}

pub(super) fn parse(bytes: &[u8]) -> Result<Package, DongleUpdateError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let manifest = member(&mut archive, "manifest.json", 16 * 1024)?;
    let manifest: Manifest = serde_json::from_slice(&manifest)?;
    let application = manifest.manifest.application;
    if application.bin_file == application.dat_file
        || application.bin_file == "manifest.json"
        || application.dat_file == "manifest.json"
    {
        return Err(DongleUpdateError::InvalidPackage(
            "overlapping package members",
        ));
    }
    let init = member(&mut archive, &application.dat_file, 4096)?;
    let image = member(&mut archive, &application.bin_file, MAX_APPLICATION)?;
    Ok(Package { init, image })
}

fn member(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    maximum: u64,
) -> Result<Vec<u8>, DongleUpdateError> {
    let count = archive.file_names().filter(|entry| *entry == name).count();
    if count != 1 {
        return Err(DongleUpdateError::InvalidPackage(
            "missing or duplicated package member",
        ));
    }
    let file = archive.by_name(name)?;
    let size = file.size();
    if size == 0 || size > maximum {
        return Err(DongleUpdateError::InvalidPackage(
            "empty or oversized package member",
        ));
    }
    let mut data = Vec::with_capacity(size as usize);
    file.take(size + 1).read_to_end(&mut data)?;
    if data.len() as u64 != size {
        return Err(DongleUpdateError::InvalidPackage(
            "package member length mismatch",
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn archive(manifest: &[u8], image: &[u8]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in [
            ("manifest.json", manifest),
            ("app.dat", b"signed init"),
            ("app.bin", image),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    const MANIFEST: &[u8] =
        br#"{"manifest":{"application":{"bin_file":"app.bin","dat_file":"app.dat"}}}"#;

    #[test]
    fn only_nonempty_application_packages_are_flashable() {
        let parsed = parse(&archive(MANIFEST, b"application image")).unwrap();
        assert_eq!(parsed.image, b"application image");
        assert_eq!(parsed.init, b"signed init");
        assert!(matches!(
            parse(&archive(MANIFEST, b"")),
            Err(DongleUpdateError::InvalidPackage(_))
        ));
        assert!(matches!(
            parse(&archive(
                br#"{"manifest":{"bootloader":{"bin_file":"app.bin","dat_file":"app.dat"}}}"#,
                b"bootloader"
            )),
            Err(DongleUpdateError::Json(_))
        ));
        assert!(matches!(
            parse(&archive(MANIFEST, &vec![0; MAX_APPLICATION as usize + 1])),
            Err(DongleUpdateError::InvalidPackage(_))
        ));
    }
}
