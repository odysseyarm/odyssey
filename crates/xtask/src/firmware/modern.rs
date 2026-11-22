use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use eros::Context as _;
use release_signing::{
    Component, DFU_HEADER_LEN, DfuVerifier, RELEASE_KEYS, RootSet, TrustList, dfu_header,
};
use sha2::{Digest as _, Sha256};

use super::signing::Signer;
use super::{hex_bytes, signing, write_hex};

const SLOT_PREFIX: usize = DFU_HEADER_LEN + 380;

#[derive(clap::Subcommand)]
pub(crate) enum TrustListArgs {
    Create {
        #[arg(long)]
        generation: u16,
        #[arg(long = "key", required = true)]
        keys: Vec<String>,
        #[arg(long = "root", required = true)]
        roots: Vec<String>,
        #[arg(long)]
        threshold: u8,
        #[arg(long)]
        output: PathBuf,
    },
    Sign {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        signer: Option<String>,
    },
    Wrap {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        signer: Option<String>,
    },
}

#[derive(clap::Args)]
pub(crate) struct AssembleLite1Args {
    /// Raw stage1 Intel HEX artifact.
    #[arg(long)]
    stage1: PathBuf,
    /// Raw network-core bootloader binary, authenticated by stage1's immutable hash.
    #[arg(long)]
    netboot: PathBuf,
    /// Stage1 build's lowercase SHA-256 sidecar for the padded netboot region.
    #[arg(long)]
    stage1_netboot_hash: PathBuf,
    /// Root-authorized signed policy record, without a transport envelope.
    #[arg(long)]
    trust: PathBuf,
    /// Signed stage2, app-A, app-B and net-core DFU envelope inputs.
    #[arg(long)]
    stage2: PathBuf,
    #[arg(long)]
    app_a: PathBuf,
    #[arg(long)]
    app_b: PathBuf,
    #[arg(long)]
    net_core: PathBuf,
    /// Output merged Intel HEX.
    #[arg(long)]
    output: PathBuf,
    /// Output component/version/SHA256 provenance text.
    #[arg(long)]
    trace: PathBuf,
    /// Dev-unit image: accept the public development key as a recovery anchor.
    /// The trace is marked so the image cannot pass as a production artifact.
    #[arg(long)]
    development: bool,
}

pub(crate) fn trust_list(args: &TrustListArgs) -> eros::Result<()> {
    match args {
        TrustListArgs::Create {
            generation,
            keys,
            roots,
            threshold,
            output,
        } => {
            let keys = keys
                .iter()
                .map(|text| parse_public_key(text))
                .collect::<eros::Result<Vec<_>>>()?;
            let roots = roots
                .iter()
                .map(|text| parse_public_key(text))
                .collect::<eros::Result<Vec<_>>>()?;
            let root_set = RootSet::new(
                *threshold,
                roots
                    .try_into()
                    .map_err(|roots: Vec<release_signing::PublicKey>| {
                        eros::error!("expected four root slots, got {}", roots.len())
                    })?,
            )
            .context("invalid root set")?;
            let list = TrustList::new(*generation, &keys, root_set)
                .context("invalid trust-list inputs")?;
            let mut record = list.to_payload().to_vec();
            record.extend_from_slice(&[0, 0, 0, 0]);
            fs::write(output, record)
                .with_context(|| format!("could not write {}", output.display()))?;
        }
        TrustListArgs::Sign {
            input,
            output,
            public_key,
            signer,
        } => {
            let public_key = parse_public_key(public_key)?;
            let bytes =
                fs::read(input).with_context(|| format!("could not read {}", input.display()))?;
            let (list, mut entries) = if bytes.len() == release_signing::TRUST_LIST_LEN {
                (
                    TrustList::parse(&bytes).context("invalid v2 trust payload")?,
                    Vec::new(),
                )
            } else if bytes.len() == release_signing::RECORD_HEADER_LEN
                && bytes[release_signing::TRUST_LIST_LEN..] == [0, 0, 0, 0]
            {
                (
                    TrustList::parse(&bytes[..release_signing::TRUST_LIST_LEN])
                        .context("invalid v2 trust payload")?,
                    Vec::new(),
                )
            } else {
                let (list, entries) = TrustList::parse_record(&bytes)
                    .context("input must be unsigned or signed v2 trust record")?;
                (list, entries.to_vec())
            };
            let message = list.signed_message();
            let signature_entries = entries
                .as_chunks::<{ release_signing::SIGNATURE_ENTRY_LEN }>()
                .0;
            for entry in signature_entries {
                let key: release_signing::PublicKey =
                    entry[..32].try_into().expect("fixed signature entry size");
                let signature: [u8; release_signing::SIGNATURE_LEN] =
                    entry[32..].try_into().expect("fixed signature entry size");
                if !release_signing::verify_with_keys(&[key], &message, &signature) {
                    eros::bail!("input contains invalid signature entry");
                }
            }
            if signature_entries
                .iter()
                .any(|entry| entry[..32] == public_key)
            {
                eros::bail!("public key already signed this policy");
            }
            let signer = match signer {
                Some(spec) => Signer::parse(spec),
                None => Signer::required_from_env()?,
            };
            let signature = signer.sign(&message)?;
            if !release_signing::verify_with_keys(&[public_key], &message, &signature) {
                eros::bail!("signature does not verify against explicit public key");
            }
            entries.extend_from_slice(&public_key);
            entries.extend_from_slice(&signature);
            let count = entries.len() / release_signing::SIGNATURE_ENTRY_LEN;
            if count > release_signing::MAX_SIGNATURES {
                eros::bail!("too many policy signatures");
            }
            let mut record = list.to_payload().to_vec();
            record.extend_from_slice(&[count as u8, 0, 0, 0]);
            record.extend_from_slice(&entries);
            fs::write(output, record)
                .with_context(|| format!("could not write {}", output.display()))?;
        }
        TrustListArgs::Wrap {
            input,
            output,
            public_key,
            signer,
        } => {
            let public_key = parse_public_key(public_key)?;
            let record =
                fs::read(input).with_context(|| format!("could not read {}", input.display()))?;
            let (list, _) =
                TrustList::parse_record(&record).context("input must be signed v2 trust record")?;
            let digest: [u8; 32] = Sha256::digest(&record).into();
            let version = [u32::from(list.generation()), 0, 0];
            let length =
                u32::try_from(record.len()).context("trust record exceeds DFU size field")?;
            let message =
                release_signing::message(Component::Lite1TrustList, length, version, &digest);
            let signer = match signer {
                Some(spec) => Signer::parse(spec),
                None => Signer::required_from_env()?,
            };
            let signature = signer.sign(&message)?;
            if !release_signing::verify_with_keys(&[public_key], &message, &signature) {
                eros::bail!("envelope signature does not verify against explicit root public key");
            }
            let header = dfu_header(
                Component::Lite1TrustList,
                length,
                version,
                &digest,
                &signature,
            );
            let mut envelope = Vec::with_capacity(DFU_HEADER_LEN + record.len());
            envelope.extend_from_slice(&header);
            envelope.extend_from_slice(&record);
            fs::write(output, envelope)
                .with_context(|| format!("could not write {}", output.display()))?;
        }
    }
    Ok(())
}

fn parse_public_key(text: &str) -> eros::Result<release_signing::PublicKey> {
    let mut key = [0; 32];
    hex::decode_to_slice(text, &mut key)
        .with_context(|| format!("public key {text:?} must be 32-byte hex"))?;
    Ok(key)
}

pub(crate) fn assemble_lite1(args: &AssembleLite1Args) -> eros::Result<()> {
    let memory_path = crate::cmd::root().join("firmware/memory/memory-lite1.kdl");
    let memory_text = fs::read_to_string(&memory_path)
        .with_context(|| format!("could not read {}", memory_path.display()))?;
    let spec = memory_text
        .parse::<memory_spec::MemorySpec>()
        .map_err(|error| eros::error!("invalid lite1 memory layout: {error}"))?;
    let regions = spec.regions();
    let flash = regions
        .get("flash")
        .context("memory map lacks flash regions")?;
    let net_flash = regions
        .get("netcore_flash")
        .context("memory map lacks netcore_flash regions")?;
    let stage1_region = region(flash, "stage1_bootloader")?;
    let stage2_region = region(flash, "stage2_bootloader")?;
    let app_a_region = region(flash, "app_bank_a")?;
    let app_b_region = region(flash, "app_bank_b")?;
    let trust_a_region = region(flash, "trust_a")?;
    let netboot_region = region(net_flash, "netcore_bootloader")?;
    let netcore_region = region(net_flash, "netcore_primary")?;
    let mut bytes = BTreeMap::new();
    let mut trace = String::new();
    let stage1_hex = fs::read_to_string(&args.stage1)
        .with_context(|| format!("could not read Intel HEX {}", args.stage1.display()))?;
    insert_hex(&mut bytes, stage1_region, &stage1_hex)?;
    let anchors_region = region(stage1_region, "recovery_anchors")?;
    let anchors = recovery_anchors(&bytes, anchors_region, args.development)?;
    if args.development {
        trace.push_str("DEVELOPMENT IMAGE: trusts the public development key\n");
    }
    // The factory stage2 is confirmed by provisioning. Stage1 only commits a new
    // trust generation once this word reads BOOT_MAGIC, and stage2 never writes
    // it over an erased word, so without this a unit could never rotate keys.
    let stage1_state = address(region(flash, "stage1_state")?)?;
    for (offset, byte) in 0xD0D0_D0D0u32.to_le_bytes().into_iter().enumerate() {
        bytes.insert(stage1_state + offset as u32, byte);
    }
    trace_line(&mut trace, "lite1-stage1-hex", "0", stage1_hex.as_bytes());
    let netboot = fs::read(&args.netboot)
        .with_context(|| format!("could not read {}", args.netboot.display()))?;
    let netboot_padded = insert_padded(&mut bytes, netboot_region, &netboot)?;
    let expected_hash = fs::read_to_string(&args.stage1_netboot_hash)
        .with_context(|| format!("could not read {}", args.stage1_netboot_hash.display()))?;
    let padded_hash = hex::encode(Sha256::digest(&netboot_padded));
    if expected_hash.trim() != padded_hash {
        eros::bail!("stage1 hash sidecar does not match assembler's padded netboot image");
    }
    let digest_bytes = hex::decode(&padded_hash).context("padded netboot hash is not hex")?;
    let digest_start = stage1_region
        .end()
        .checked_sub(48)
        .context("stage1 region too small for embedded network bootloader hash")?;
    for (offset, byte) in digest_bytes.iter().enumerate() {
        let address =
            u32::try_from(digest_start + u64::try_from(offset).context("hash offset exceeds u64")?)
                .context("stage1 hash address exceeds u32")?;
        if bytes.get(&address) != Some(byte) {
            eros::bail!("stage1 HEX embedded hash does not match padded netboot image");
        }
    }
    trace_line(&mut trace, "lite1-netcore-bootloader-raw", "0", &netboot);
    trace_line(
        &mut trace,
        "lite1-netcore-bootloader-padded",
        "0",
        &netboot_padded,
    );
    let trust_list = read_trust(
        &args.trust,
        &anchors,
        &bytes,
        region(stage1_region, "recovery_threshold")?,
    )?;
    let record = fs::read(&args.trust)
        .with_context(|| format!("could not read {}", args.trust.display()))?;
    let mut transport = vec![0xff; DFU_HEADER_LEN];
    transport.extend_from_slice(&record);
    insert_raw(&mut bytes, trust_a_region, &transport)?;
    trace_line(
        &mut trace,
        "lite1-trust-list",
        &trust_list.generation().to_string(),
        &record,
    );
    for (path, slot, component) in [
        (
            &args.stage2,
            stage2_region,
            Component::Lite1SecureBootloader,
        ),
        (&args.app_a, app_a_region, Component::Lite1SecureApp),
        (&args.app_b, app_b_region, Component::Lite1SecureApp),
        (
            &args.net_core,
            netcore_region,
            Component::Lite1SecureNetCore,
        ),
    ] {
        let signed =
            fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        let capacity = if component == Component::Lite1SecureNetCore {
            netcore_region
                .length()
                .checked_sub(4096)
                .context("netcore primary lacks its state page")?
        } else {
            slot.length()
        };
        let (version, _) = insert_signed(
            &mut bytes,
            slot,
            &signed,
            component,
            trust_list.keys(),
            capacity,
        )?;
        trace_line(
            &mut trace,
            component_name(component),
            &version.map(|part| part.to_string()).join("."),
            &signed[DFU_HEADER_LEN..],
        );
    }
    fs::write(&args.output, write_hex(&bytes))
        .with_context(|| format!("could not write {}", args.output.display()))?;
    fs::write(&args.trace, trace)
        .with_context(|| format!("could not write {}", args.trace.display()))?;
    Ok(())
}
pub(super) fn assemble_release_lite1(
    entry: &serde_json::Value,
    build_dir: &Path,
    stage2: &[u8],
    net_core: &[u8],
) -> eros::Result<()> {
    let trust = std::env::var_os("ODYSSEY_LITE1_TRUST_DFU")
        .map(PathBuf::from)
        .context(
            "ODYSSEY_LITE1_TRUST_DFU must name reviewed root-authorized factory trust record",
        )?;
    let app_a = fs::read(build_dir.join("app-a.bin")).context("missing build/app-a.bin")?;
    let app_b = fs::read(build_dir.join("app-b.bin")).context("missing build/app-b.bin")?;
    let app_version = signing::app_version(
        entry["version"]
            .as_str()
            .context("Lite1 manifest entry has no application version")?,
    )?;
    let stage2_version = signing::app_version(
        entry["files"]["stage2-bootloader"]["version"]
            .as_str()
            .context("Lite1 manifest entry has no stage2 version")?,
    )?;
    let net_release = u32::try_from(
        entry["files"]["net-core"]["release"]
            .as_u64()
            .context("Lite1 manifest entry has no net-core release counter")?,
    )
    .context("net-core release counter exceeds u32")?;
    let stage2_file = build_dir.join("release-stage2.dfu");
    let app_a_file = build_dir.join("release-app-a.dfu");
    let app_b_file = build_dir.join("release-app-b.dfu");
    let net_core_file = build_dir.join("release-net-core.dfu");
    for (key, image, component, version, path) in [
        (
            "stage2-bootloader",
            stage2,
            Component::Lite1SecureBootloader,
            stage2_version,
            &stage2_file,
        ),
        (
            "app-a",
            &app_a,
            Component::Lite1SecureApp,
            app_version,
            &app_a_file,
        ),
        (
            "app-b",
            &app_b,
            Component::Lite1SecureApp,
            app_version,
            &app_b_file,
        ),
        (
            "net-core",
            net_core,
            Component::Lite1SecureNetCore,
            [net_release, 0, 0],
            &net_core_file,
        ),
    ] {
        write_manifest_signed(key, image, component, version, &entry["files"][key], path)?;
    }
    assemble_lite1(&AssembleLite1Args {
        development: false,
        stage1: build_dir.join("stage1-bootloader.hex"),
        stage1_netboot_hash: build_dir.join("stage1-netcore-bootloader.sha256"),
        netboot: build_dir.join("netcore-bootloader.bin"),
        trust,
        stage2: stage2_file,
        app_a: app_a_file,
        app_b: app_b_file,
        net_core: net_core_file,
        output: build_dir.join("merged.hex"),
        trace: build_dir.join("merged.trace"),
    })
}

fn write_manifest_signed(
    key: &str,
    image: &[u8],
    component: Component,
    version: [u32; 3],
    file: &serde_json::Value,
    path: &Path,
) -> eros::Result<()> {
    let digest = super::FileDigest::of(image);
    if file["sha256"].as_str() != Some(digest.sha256.as_str())
        || file["size"].as_u64() != Some(digest.size)
    {
        eros::bail!("{key} bytes differ from manifest SHA-256 or size");
    }
    let mut sha256 = [0; 32];
    hex::decode_to_slice(&digest.sha256, &mut sha256)
        .context("manifest image SHA-256 is malformed")?;
    let signature: [u8; 64] = hex::decode(
        file["signature"]
            .as_str()
            .with_context(|| format!("manifest file {key} has no signature"))?,
    )
    .with_context(|| format!("manifest file {key} signature is malformed"))?
    .try_into()
    .map_err(|_| eros::error!("{key} signature is not 64 bytes"))?;
    let header = dfu_header(
        component,
        u32::try_from(image.len()).context("image exceeds DFU size field")?,
        version,
        &sha256,
        &signature,
    );
    let mut verifier = DfuVerifier::new_with_keys(component, &header, &RELEASE_KEYS)
        .with_context(|| format!("manifest signature for {key} is not production-authorized"))?;
    verifier
        .update(image)
        .context("image exceeds signed length")?;
    verifier
        .finish()
        .context("manifest image digest or size mismatch")?;
    let mut signed = Vec::with_capacity(DFU_HEADER_LEN + image.len());
    signed.extend_from_slice(&header);
    signed.extend_from_slice(image);
    fs::write(path, signed).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

pub(super) fn region<'a>(
    regions: &'a memory_spec::Regions,
    name: &str,
) -> eros::Result<&'a memory_spec::Regions> {
    Ok(regions
        .get(name)
        .with_context(|| format!("lite1 memory map lacks region {name}"))?)
}

pub(super) fn address(region: &memory_spec::Regions) -> eros::Result<u32> {
    Ok(u32::try_from(region.origin()).context("memory region address exceeds u32")?)
}

fn insert_hex(
    bytes: &mut BTreeMap<u32, u8>,
    region: &memory_spec::Regions,
    hex: &str,
) -> eros::Result<()> {
    let start = region.origin();
    let end = region.end();
    for (address, byte) in hex_bytes(hex)? {
        if u64::from(address) < start || u64::from(address) >= end {
            eros::bail!("stage1 HEX contains byte outside stage1 region at {address:#010x}");
        }
        if bytes.insert(address, byte).is_some() {
            eros::bail!("overlapping firmware bytes at {address:#010x}");
        }
    }
    Ok(())
}
fn insert_raw(
    bytes: &mut BTreeMap<u32, u8>,
    region: &memory_spec::Regions,
    data: &[u8],
) -> eros::Result<()> {
    if data.len() as u64 > region.length() {
        eros::bail!("raw image exceeds configured memory region");
    }
    for (offset, byte) in data.iter().copied().enumerate() {
        insert_byte(bytes, address(region)?, offset, byte)?;
    }
    Ok(())
}

fn insert_padded(
    bytes: &mut BTreeMap<u32, u8>,
    region: &memory_spec::Regions,
    data: &[u8],
) -> eros::Result<Vec<u8>> {
    let length =
        usize::try_from(region.length()).context("network bootloader region exceeds usize")?;
    if data.len() > length || length != 60 * 1024 {
        eros::bail!("netcore bootloader must fit configured 60 KiB region");
    }
    let mut padded = vec![0xff; length];
    padded[..data.len()].copy_from_slice(data);
    insert_raw(bytes, region, &padded)?;
    Ok(padded)
}

fn trace_line(trace: &mut String, component: &str, version: &str, image: &[u8]) {
    use std::fmt::Write as _;
    writeln!(
        trace,
        "{component}\t{version}\t{}",
        hex::encode(Sha256::digest(image))
    )
    .expect("writing to String cannot fail");
}

fn read_trust(
    path: &Path,
    anchors: &[release_signing::PublicKey],
    stage1: &BTreeMap<u32, u8>,
    threshold_region: &memory_spec::Regions,
) -> eros::Result<TrustList> {
    let record = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    if threshold_region.length() != 4 {
        eros::bail!("immutable recovery threshold must occupy four bytes");
    }
    let threshold_address = address(threshold_region)?;
    let mut threshold_bytes = [0; 4];
    for (offset, byte) in threshold_bytes.iter_mut().enumerate() {
        *byte = *stage1
            .get(&(threshold_address + offset as u32))
            .context("stage1 HEX omits immutable recovery threshold")?;
    }
    let threshold = u32::from_le_bytes(threshold_bytes);
    if !(1..=4).contains(&threshold) {
        eros::bail!("invalid immutable recovery threshold");
    }
    validate_factory_trust(&record, anchors, threshold as u8)
}

fn validate_factory_trust(
    record: &[u8],
    anchors: &[release_signing::PublicKey],
    threshold: u8,
) -> eros::Result<TrustList> {
    let anchor_slots =
        <[release_signing::PublicKey; release_signing::KEY_SLOTS]>::try_from(anchors)
            .context("factory anchors must have four slots")?;
    let current = RootSet::new(threshold, anchor_slots).context("invalid immutable root set")?;
    let (list, entries) = TrustList::parse_record(record).context("invalid signed trust record")?;
    if list.generation() != 1
        || list.root_set() != &current
        || !list.authorized_by(&current, entries)
    {
        eros::bail!(
            "factory policy must be generation 1, preserve immutable roots and threshold, and carry threshold signatures"
        );
    }
    Ok(list)
}
fn recovery_anchors(
    bytes: &BTreeMap<u32, u8>,
    region: &memory_spec::Regions,
    development: bool,
) -> eros::Result<Vec<release_signing::PublicKey>> {
    if region.length() != 128 {
        eros::bail!("immutable recovery-anchor table must be 128 bytes");
    }
    let base = address(region)?;
    let mut anchors = Vec::with_capacity(4);
    for slot in 0..4 {
        let mut key = [0u8; 32];
        for (offset, byte) in key.iter_mut().enumerate() {
            *byte = *bytes
                .get(&(base + slot * 32 + offset as u32))
                .context("stage1 HEX omits immutable recovery-anchor bytes")?;
        }
        if key == release_signing::DEV_KEY && !development {
            eros::bail!("development key cannot be an immutable production recovery anchor");
        }
        if key.iter().any(|byte| *byte != 0) && anchors.contains(&key) {
            eros::bail!("immutable recovery-anchor table contains duplicate keys");
        }
        anchors.push(key);
    }
    if anchors.iter().all(|key| key.iter().all(|byte| *byte == 0)) {
        eros::bail!("stage1 immutable recovery-anchor table is empty");
    }
    Ok(anchors)
}

fn insert_signed(
    bytes: &mut BTreeMap<u32, u8>,
    region: &memory_spec::Regions,
    signed: &[u8],
    component: Component,
    keys: &[release_signing::PublicKey],
    capacity: u64,
) -> eros::Result<([u32; 3], [u8; 32])> {
    if signed.len() < DFU_HEADER_LEN {
        eros::bail!(
            "signed {} input is shorter than DFU header",
            component_name(component)
        );
    }
    let header = &signed[..DFU_HEADER_LEN];
    let payload = &signed[DFU_HEADER_LEN..];
    let mut verifier = DfuVerifier::new_with_keys(component, header, keys).with_context(|| {
        format!(
            "{} has invalid or wrong-component DFU header",
            component_name(component)
        )
    })?;
    verifier
        .update(payload)
        .context("signed image payload exceeds declared size")?;
    verifier
        .finish()
        .context("signed image payload hash or size mismatch")?;
    let occupied = SLOT_PREFIX
        .checked_add(payload.len())
        .context("signed slot length overflow")?;
    if occupied as u64 > capacity || occupied as u64 > region.length() {
        eros::bail!(
            "signed {} image exceeds configured slot",
            component_name(component)
        );
    }
    pack_signed(bytes, address(region)?, capacity, header, payload)?;
    let version = std::array::from_fn(|index| {
        let start = 23 + 4 * index;
        u32::from_le_bytes(
            header[start..start + 4]
                .try_into()
                .expect("version field width"),
        )
    });
    Ok((version, Sha256::digest(payload).into()))
}

fn pack_signed(
    bytes: &mut BTreeMap<u32, u8>,
    base: u32,
    capacity: u64,
    header: &[u8],
    payload: &[u8],
) -> eros::Result<()> {
    let occupied = SLOT_PREFIX
        .checked_add(payload.len())
        .context("signed slot length overflow")?;
    if occupied as u64 > capacity {
        eros::bail!("signed image exceeds configured slot");
    }
    for (offset, byte) in header.iter().copied().enumerate() {
        insert_byte(bytes, base, offset, byte)?;
    }
    for offset in DFU_HEADER_LEN..SLOT_PREFIX {
        insert_byte(bytes, base, offset, 0xff)?;
    }
    for (offset, byte) in payload.iter().copied().enumerate() {
        insert_byte(bytes, base, SLOT_PREFIX + offset, byte)?;
    }
    Ok(())
}
fn insert_byte(
    bytes: &mut BTreeMap<u32, u8>,
    base: u32,
    offset: usize,
    byte: u8,
) -> eros::Result<()> {
    let address = base
        .checked_add(u32::try_from(offset).context("slot offset exceeds u32")?)
        .context("slot address overflow")?;
    if bytes.insert(address, byte).is_some() {
        eros::bail!("overlapping firmware bytes at {address:#010x}");
    }
    Ok(())
}

fn component_name(component: Component) -> &'static str {
    match component {
        Component::Lite1SecureApp => "lite1-secure-app",
        Component::Lite1SecureNetCore => "lite1-secure-netcore",
        Component::Lite1SecureBootloader => "lite1-secure-stage2",
        _ => "unexpected-component",
    }
}

#[cfg(test)]
mod tests {
    use salty::Keypair;

    use super::*;

    const SEED: [u8; 32] = [0x37; 32];

    #[test]
    fn signed_slot_packs_header_padding_payload_and_rejects_tamper_and_component_swap() {
        let payload = b"firmware payload";
        let digest: [u8; 32] = Sha256::digest(payload).into();
        let message = release_signing::message(
            Component::Lite1SecureApp,
            payload.len() as u32,
            [1, 2, 3],
            &digest,
        );
        let signature = Keypair::from(&SEED).sign(&message).to_bytes();
        let header = dfu_header(
            Component::Lite1SecureApp,
            payload.len() as u32,
            [1, 2, 3],
            &digest,
            &signature,
        );
        let mut slot = BTreeMap::new();
        pack_signed(&mut slot, 0, 1024, &header, payload).unwrap();
        assert_eq!(slot[&(SLOT_PREFIX as u32)], payload[0]);
        assert!(
            slot.range(DFU_HEADER_LEN as u32..SLOT_PREFIX as u32)
                .all(|(_, byte)| *byte == 0xff)
        );
        let public = Keypair::from(&SEED).public.to_bytes();
        let mut verifier =
            DfuVerifier::new_with_keys(Component::Lite1SecureApp, &header, &[public]).unwrap();
        verifier.update(payload).unwrap();
        verifier.finish().unwrap();
        let mut tampered = payload.to_vec();
        tampered[0] ^= 1;
        let mut verifier =
            DfuVerifier::new_with_keys(Component::Lite1SecureApp, &header, &[public]).unwrap();
        verifier.update(&tampered).unwrap();
        assert!(verifier.finish().is_err());
        assert!(
            DfuVerifier::new_with_keys(Component::Lite1SecureNetCore, &header, &[public]).is_err()
        );
        assert_eq!(SLOT_PREFIX, 512);
    }

    #[test]
    fn v2_policy_accepts_threshold_and_rejects_bad_factory_policy_and_outer_key() {
        let key_a = Keypair::from(&[1; 32]);
        let key_b = Keypair::from(&[2; 32]);
        let mut slots = [[0; 32]; 4];
        slots[0] = key_a.public.to_bytes();
        slots[1] = key_b.public.to_bytes();
        let roots = RootSet::new(2, slots).unwrap();
        let list = TrustList::new(1, &[key_a.public.to_bytes()], roots).unwrap();
        let message = list.signed_message();
        let mut record = list.to_payload().to_vec();
        record.extend_from_slice(&[2, 0, 0, 0]);
        for key in [&key_a, &key_b] {
            record.extend_from_slice(&key.public.to_bytes());
            record.extend_from_slice(&key.sign(&message).to_bytes());
        }
        let (parsed, entries) = TrustList::parse_record(&record).unwrap();
        assert!(parsed.authorized_by(&roots, entries));
        assert_eq!(
            validate_factory_trust(&record, roots.roots(), roots.threshold()).unwrap(),
            list
        );
        let mut one_signature = list.to_payload().to_vec();
        one_signature.extend_from_slice(&[1, 0, 0, 0]);
        one_signature.extend_from_slice(&key_a.public.to_bytes());
        one_signature.extend_from_slice(&key_a.sign(&message).to_bytes());
        assert!(validate_factory_trust(&one_signature, roots.roots(), roots.threshold()).is_err());
        let mut malformed = record.clone();
        malformed[release_signing::TRUST_LIST_LEN] = 0;
        assert!(validate_factory_trust(&malformed, roots.roots(), roots.threshold()).is_err());
        let other_roots =
            RootSet::new(1, [key_a.public.to_bytes(), [0; 32], [0; 32], [0; 32]]).unwrap();
        assert!(
            validate_factory_trust(&record, other_roots.roots(), other_roots.threshold()).is_err()
        );
        let digest: [u8; 32] = Sha256::digest(&record).into();
        let outer_message = release_signing::message(
            Component::Lite1TrustList,
            record.len() as u32,
            [1, 0, 0],
            &digest,
        );
        let signature = key_a.sign(&outer_message).to_bytes();
        assert!(release_signing::verify_with_keys(
            &[key_a.public.to_bytes()],
            &outer_message,
            &signature,
        ));
        assert!(!release_signing::verify_with_keys(
            &[key_b.public.to_bytes()],
            &outer_message,
            &signature,
        ));
    }
}
