//! What the display itself says about its size: EDID parsing and the
//! platform lookups that find a monitor's EDID.
//!
//! A flat panel's physical size is known to the factory, so it is a much
//! better scale anchor than the depth camera's measurement. A projector has no
//! meaningful size, and plenty of displays report a placeholder, which is why
//! the parser grades what it finds instead of passing numbers through.

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::solve::KnownSize;

const BLOCK_LEN: usize = 128;
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
/// Offset of the first 18-byte descriptor in the base block: the preferred
/// timing.
const FIRST_DESCRIPTOR: usize = 54;
const DESCRIPTOR_LEN: usize = 18;
const DESCRIPTOR_TAG_MONITOR_NAME: u8 = 0xFC;

/// Sizes outside this range on either axis are not a real panel.
const MIN_PLAUSIBLE_MM: u32 = 100;
const MAX_PLAUSIBLE_MM: u32 = 10_000;
/// The physical aspect may differ from the native resolution's by this much
/// before the size is considered unreliable. Panels with non-square pixels do
/// exist but stay well inside it.
const MAX_ASPECT_DEVIATION: f64 = 0.03;
/// Aspect ratios some firmwares write into the centimeter bytes instead of a
/// size, as `(width, height)`.
const PLACEHOLDER_CM: [(u32, u32); 3] = [(16, 9), (16, 10), (4, 3)];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdidError {
    #[error("EDID is {len} bytes, shorter than the 128-byte base block")]
    TooShort { len: usize },
    #[error("EDID does not start with the fixed 8-byte header")]
    BadHeader,
}

/// How finely the display reported its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizePrecision {
    /// From the preferred timing's 12-bit millimeter fields.
    Millimeters,
    /// From the base block's whole-centimeter bytes: up to 5 mm per axis off,
    /// about 0.4% of a 1.2 m panel.
    Centimeters,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhysicalSize {
    Dimensions {
        width_mm: u32,
        height_mm: u32,
        precision: SizePrecision,
    },
    /// Only the aspect ratio (width over height), which the base block encodes
    /// by zeroing exactly one of its two size bytes.
    AspectOnly(f64),
    /// Both size bytes are zero: what a projector reports.
    Undefined,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SuspectReason {
    TooSmall,
    TooLarge,
    /// The centimeter bytes hold an aspect ratio such as 16 by 9, not a size.
    AspectPlaceholder,
    /// The size's aspect ratio disagrees with the native resolution's.
    AspectMismatch {
        size_aspect: f64,
        native_aspect: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reliability {
    Reliable,
    /// A size was reported but should not be used as a scale anchor.
    Suspect(SuspectReason),
    /// There is no size to judge.
    NoDimensions,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdidInfo {
    /// Three-letter PnP vendor id, e.g. "SAM".
    pub manufacturer: String,
    pub product_code: u16,
    /// Name from the monitor-name descriptor, e.g. "LG TV".
    pub monitor_name: Option<String>,
    /// Active pixels of the preferred timing.
    pub native_resolution: Option<(u32, u32)>,
    pub refresh_hz: Option<f64>,
    pub physical_size: PhysicalSize,
    pub reliability: Reliability,
    /// Whether the base block and every extension block present sum to zero.
    /// A bad sum flags a corrupt or hand-edited EDID but the fields are still
    /// reported, since many working displays ship one.
    pub checksums_valid: bool,
}

impl EdidInfo {
    /// Width over height of the physical panel, when the EDID gives one.
    pub fn physical_aspect(&self) -> Option<f64> {
        match self.physical_size {
            PhysicalSize::Dimensions {
                width_mm,
                height_mm,
                ..
            } => Some(f64::from(width_mm) / f64::from(height_mm)),
            PhysicalSize::AspectOnly(a) => Some(a),
            PhysicalSize::Undefined => None,
        }
    }

    /// The panel size to use as a scale anchor: present only when the EDID
    /// reports dimensions and they pass every reliability check.
    pub fn known_size(&self) -> Option<KnownSize> {
        match (self.physical_size, self.reliability) {
            (
                PhysicalSize::Dimensions {
                    width_mm,
                    height_mm,
                    ..
                },
                Reliability::Reliable,
            ) => Some(KnownSize::Dimensions {
                width_m: f64::from(width_mm) / 1000.0,
                height_m: f64::from(height_mm) / 1000.0,
            }),
            _ => None,
        }
    }
}

fn decode_manufacturer(hi: u8, lo: u8) -> String {
    let v = u16::from_be_bytes([hi, lo]);
    [(v >> 10) & 0x1F, (v >> 5) & 0x1F, v & 0x1F]
        .into_iter()
        .map(|n| {
            if (1..=26).contains(&n) {
                char::from(b'A' - 1 + n as u8)
            } else {
                '?'
            }
        })
        .collect()
}

fn block_sums_to_zero(block: &[u8]) -> bool {
    block.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == 0
}

struct Timing {
    width: u32,
    height: u32,
    refresh_hz: Option<f64>,
    image_mm: (u32, u32),
}

/// Reads a detailed timing descriptor. A zero pixel clock marks a descriptor
/// that holds something else.
fn parse_timing(d: &[u8]) -> Option<Timing> {
    let clock_10khz = u32::from(u16::from_le_bytes([d[0], d[1]]));
    if clock_10khz == 0 {
        return None;
    }
    let h_active = u32::from(d[2]) | (u32::from(d[4] & 0xF0) << 4);
    let h_blank = u32::from(d[3]) | (u32::from(d[4] & 0x0F) << 8);
    let v_active = u32::from(d[5]) | (u32::from(d[7] & 0xF0) << 4);
    let v_blank = u32::from(d[6]) | (u32::from(d[7] & 0x0F) << 8);
    let image_w = u32::from(d[12]) | (u32::from(d[14] & 0xF0) << 4);
    let image_h = u32::from(d[13]) | (u32::from(d[14] & 0x0F) << 8);
    let total = f64::from((h_active + h_blank) * (v_active + v_blank));
    Some(Timing {
        width: h_active,
        height: v_active,
        refresh_hz: (total > 0.0).then(|| f64::from(clock_10khz) * 10_000.0 / total),
        image_mm: (image_w, image_h),
    })
}

fn parse_name(descriptor: &[u8]) -> Option<String> {
    if descriptor[..3] != [0, 0, 0] || descriptor[3] != DESCRIPTOR_TAG_MONITOR_NAME {
        return None;
    }
    let text: String = descriptor[5..]
        .iter()
        .take_while(|&&b| b != 0x0A)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                char::from(b)
            } else {
                '?'
            }
        })
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn judge(size: PhysicalSize, native: Option<(u32, u32)>) -> Reliability {
    let PhysicalSize::Dimensions {
        width_mm,
        height_mm,
        precision,
    } = size
    else {
        return Reliability::NoDimensions;
    };
    if precision == SizePrecision::Centimeters
        && PLACEHOLDER_CM.contains(&(width_mm / 10, height_mm / 10))
    {
        return Reliability::Suspect(SuspectReason::AspectPlaceholder);
    }
    if width_mm < MIN_PLAUSIBLE_MM || height_mm < MIN_PLAUSIBLE_MM {
        return Reliability::Suspect(SuspectReason::TooSmall);
    }
    if width_mm > MAX_PLAUSIBLE_MM || height_mm > MAX_PLAUSIBLE_MM {
        return Reliability::Suspect(SuspectReason::TooLarge);
    }
    if let Some((w, h)) = native.filter(|&(w, h)| w > 0 && h > 0) {
        let size_aspect = f64::from(width_mm) / f64::from(height_mm);
        let native_aspect = f64::from(w) / f64::from(h);
        if (size_aspect / native_aspect - 1.0).abs() > MAX_ASPECT_DEVIATION {
            return Reliability::Suspect(SuspectReason::AspectMismatch {
                size_aspect,
                native_aspect,
            });
        }
    }
    Reliability::Reliable
}

/// Parses an EDID blob, as read from sysfs or the registry.
pub fn parse_edid(bytes: &[u8]) -> Result<EdidInfo, EdidError> {
    if bytes.len() < BLOCK_LEN {
        return Err(EdidError::TooShort { len: bytes.len() });
    }
    if bytes[..8] != HEADER {
        return Err(EdidError::BadHeader);
    }

    let declared_blocks = 1 + usize::from(bytes[126]);
    let checksums_valid = bytes.len() >= declared_blocks * BLOCK_LEN
        && bytes
            .as_chunks::<BLOCK_LEN>()
            .0
            .iter()
            .take(declared_blocks)
            .all(|block| block_sums_to_zero(block));

    let descriptors: Vec<&[u8]> = (0..4)
        .map(|i| {
            &bytes
                [FIRST_DESCRIPTOR + i * DESCRIPTOR_LEN..FIRST_DESCRIPTOR + (i + 1) * DESCRIPTOR_LEN]
        })
        .collect();
    let preferred = parse_timing(descriptors[0]);
    let monitor_name = descriptors.iter().find_map(|d| parse_name(d));

    let physical_size = {
        let (cm_w, cm_h) = (u32::from(bytes[21]), u32::from(bytes[22]));
        match preferred.as_ref().map(|t| t.image_mm) {
            // The timing's millimeters are the finer figure; the base block
            // rounds to centimeters.
            Some((w, h)) if w > 0 && h > 0 => PhysicalSize::Dimensions {
                width_mm: w,
                height_mm: h,
                precision: SizePrecision::Millimeters,
            },
            _ => match (cm_w, cm_h) {
                (0, 0) => PhysicalSize::Undefined,
                (w, 0) => PhysicalSize::AspectOnly((f64::from(w) + 99.0) / 100.0),
                (0, h) => PhysicalSize::AspectOnly(100.0 / (f64::from(h) + 99.0)),
                (w, h) => PhysicalSize::Dimensions {
                    width_mm: w * 10,
                    height_mm: h * 10,
                    precision: SizePrecision::Centimeters,
                },
            },
        }
    };

    let native_resolution = preferred.as_ref().map(|t| (t.width, t.height));
    Ok(EdidInfo {
        manufacturer: decode_manufacturer(bytes[8], bytes[9]),
        product_code: u16::from_le_bytes([bytes[10], bytes[11]]),
        monitor_name,
        native_resolution,
        refresh_hz: preferred.and_then(|t| t.refresh_hz),
        reliability: judge(physical_size, native_resolution),
        physical_size,
        checksums_valid,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupSource {
    /// A `/sys/class/drm/<connector>/edid` file.
    Sysfs(PathBuf),
    /// The registry's per-monitor `EDID` value; holds the monitor's device id.
    WindowsRegistry(String),
    /// This platform has no lookup, or none was attempted.
    Unavailable,
}

/// Why a lookup found no EDID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupNote {
    UnsupportedPlatform,
    /// The caller has no monitor name to match on.
    NoMonitorName,
    /// No connected display carries that name.
    NotFound,
    /// More than one display matches, so any pick would be a guess.
    Ambiguous {
        candidates: usize,
    },
    /// The EDID has no native resolution to check the monitor's size against.
    NoNativeResolution,
    /// The EDID's resolution and aspect do not fit the monitor's pixel size,
    /// which means the name matched a different display than the one shown.
    ResolutionMismatch {
        native: (u32, u32),
        screen: (u32, u32),
    },
    Unreadable(io::ErrorKind),
    Invalid(EdidError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayLookup {
    pub edid: Option<EdidInfo>,
    pub source: LookupSource,
    pub note: Option<LookupNote>,
}

impl DisplayLookup {
    fn none(note: LookupNote) -> Self {
        Self {
            edid: None,
            source: LookupSource::Unavailable,
            note: Some(note),
        }
    }
}

/// Checks that an EDID can describe a monitor that the desktop reports with
/// `size_px`. Desktops run displays at modes other than native and at scaled
/// sizes, so the check accepts the native resolution, any listed mode and any
/// size with the native aspect ratio (either orientation).
fn check_consistency(
    edid: &EdidInfo,
    size_px: (u32, u32),
    modes: &[(u32, u32)],
) -> Result<(), LookupNote> {
    let Some(native) = edid.native_resolution.filter(|&(w, h)| w > 0 && h > 0) else {
        return Err(LookupNote::NoNativeResolution);
    };
    if size_px.0 == 0 || size_px.1 == 0 {
        return Err(LookupNote::ResolutionMismatch {
            native,
            screen: size_px,
        });
    }
    let swapped = |(w, h): (u32, u32)| (h, w);
    if modes.contains(&size_px) || modes.contains(&swapped(size_px)) {
        return Ok(());
    }
    let aspect = |(w, h): (u32, u32)| f64::from(w) / f64::from(h);
    let (screen_aspect, native_aspect) = (aspect(size_px), aspect(native));
    let close = |a: f64, b: f64| (a / b - 1.0).abs() <= MAX_ASPECT_DEVIATION;
    if close(screen_aspect, native_aspect) || close(1.0 / screen_aspect, native_aspect) {
        Ok(())
    } else {
        Err(LookupNote::ResolutionMismatch {
            native,
            screen: size_px,
        })
    }
}

/// Kernel connector names carry the connector type's variant letter
/// (`HDMI-A-1`) that X11 drivers drop (`HDMI-1`); compare without it.
fn normalize_connector(name: &str) -> String {
    name.trim()
        .to_ascii_uppercase()
        .replace("-A-", "-")
        .replace("-B-", "-")
}

/// `"1920x1080"`, `"1920x1080i"` and `"1920x1080_60.00"` all name 1920 by 1080.
fn parse_mode(line: &str) -> Option<(u32, u32)> {
    let (w, rest) = line.trim().split_once('x')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((w.parse().ok()?, digits.parse().ok()?))
}

struct SysfsDisplay {
    connector: String,
    path: PathBuf,
    edid: EdidInfo,
    modes: Vec<(u32, u32)>,
}

fn read_sysfs_displays(root: &Path) -> Result<Vec<SysfsDisplay>, io::Error> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        // Entries are `card<N>-<connector>`; bare `card<N>` and `renderD*` are not connectors.
        let Some((card, connector)) = file_name.split_once('-') else {
            continue;
        };
        if !card.starts_with("card") {
            continue;
        }
        let edid_path = entry.path().join("edid");
        let Ok(bytes) = fs::read(&edid_path) else {
            continue;
        };
        // A disconnected connector exposes an empty file.
        let Ok(edid) = parse_edid(&bytes) else {
            continue;
        };
        let modes = fs::read_to_string(entry.path().join("modes"))
            .map(|text| text.lines().filter_map(parse_mode).collect())
            .unwrap_or_default();
        found.push(SysfsDisplay {
            connector: connector.to_owned(),
            path: edid_path,
            edid,
            modes,
        });
    }
    Ok(found)
}

/// Finds the EDID of the display called `monitor_name` under a sysfs DRM
/// directory such as `/sys/class/drm`. The name is first matched against
/// connector names, then against the EDID's own monitor name. Anything but a
/// single consistent match yields no EDID.
pub fn lookup_in_sysfs(
    root: &Path,
    monitor_name: Option<&str>,
    size_px: (u32, u32),
) -> DisplayLookup {
    let Some(name) = monitor_name.map(str::trim).filter(|n| !n.is_empty()) else {
        return DisplayLookup::none(LookupNote::NoMonitorName);
    };
    let displays = match read_sysfs_displays(root) {
        Ok(d) => d,
        Err(e) => return DisplayLookup::none(LookupNote::Unreadable(e.kind())),
    };

    let wanted = normalize_connector(name);
    let mut matches: Vec<&SysfsDisplay> = displays
        .iter()
        .filter(|d| normalize_connector(&d.connector) == wanted)
        .collect();
    if matches.is_empty() {
        matches = displays
            .iter()
            .filter(|d| {
                d.edid
                    .monitor_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .collect();
    }
    match matches.as_slice() {
        [] => DisplayLookup::none(LookupNote::NotFound),
        [one] => match check_consistency(&one.edid, size_px, &one.modes) {
            Ok(()) => DisplayLookup {
                edid: Some(one.edid.clone()),
                source: LookupSource::Sysfs(one.path.clone()),
                note: None,
            },
            Err(note) => DisplayLookup::none(note),
        },
        many => DisplayLookup::none(LookupNote::Ambiguous {
            candidates: many.len(),
        }),
    }
}

/// Splits `MONITOR\DELA0A1\{4d36e96e-e325-11ce-bfc1-08002be10318}\0004` into
/// the hardware id (`DELA0A1`) that names the monitor's `Enum\DISPLAY` key and
/// the driver value (`{4d36e96e-...}\0004`) that picks the instance under it.
#[cfg(any(windows, test))]
fn parse_monitor_device_id(device_id: &str) -> Option<(String, String)> {
    let mut parts = device_id.splitn(3, '\\');
    let (class, hardware_id, driver) = (parts.next()?, parts.next()?, parts.next()?);
    (class.eq_ignore_ascii_case("MONITOR") && !hardware_id.is_empty() && !driver.is_empty())
        .then(|| (hardware_id.to_owned(), driver.to_owned()))
}

#[cfg(windows)]
mod windows_registry {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Graphics::Gdi::{
        DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_BINARY, REG_SZ, RegCloseKey, RegEnumKeyExW,
        RegOpenKeyExW, RegQueryValueExW,
    };

    use super::*;

    /// Registry values here are a few hundred bytes; anything larger is not an EDID.
    const MAX_VALUE_BYTES: u32 = 64 * 1024;
    /// Registry key names are at most 255 characters.
    const MAX_KEY_NAME: usize = 256;
    const MAX_ADAPTERS: u32 = 64;
    const MAX_MONITORS_PER_ADAPTER: u32 = 16;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }

    struct Key(HKEY);

    impl Key {
        fn open(parent: HKEY, path: &str) -> Option<Key> {
            let path = wide(path);
            let mut key: HKEY = null_mut();
            // SAFETY: `path` is NUL-terminated and outlives the call, `key` is a valid out pointer.
            let status = unsafe { RegOpenKeyExW(parent, path.as_ptr(), 0, KEY_READ, &mut key) };
            (status == ERROR_SUCCESS).then_some(Key(key))
        }

        fn value(&self, name: &str, expected_type: u32) -> Option<Vec<u8>> {
            let name = wide(name);
            let mut kind = 0u32;
            let mut size = 0u32;
            // SAFETY: a null data pointer asks only for the type and size.
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    name.as_ptr(),
                    null(),
                    &mut kind,
                    null_mut(),
                    &mut size,
                )
            };
            if status != ERROR_SUCCESS
                || kind != expected_type
                || size == 0
                || size > MAX_VALUE_BYTES
            {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            // SAFETY: `data` has `size` writable bytes, which `size` tells the call.
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    name.as_ptr(),
                    null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut size,
                )
            };
            (status == ERROR_SUCCESS).then(|| {
                data.truncate(size as usize);
                data
            })
        }

        fn string(&self, name: &str) -> Option<String> {
            let bytes = self.value(name, REG_SZ)?;
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            Some(from_wide(&units))
        }

        fn subkeys(&self) -> Vec<String> {
            let mut names = Vec::new();
            for index in 0..1024u32 {
                let mut buffer = [0u16; MAX_KEY_NAME];
                let mut len = MAX_KEY_NAME as u32;
                // SAFETY: `buffer` holds `len` units; the optional outputs are null.
                let status = unsafe {
                    RegEnumKeyExW(
                        self.0,
                        index,
                        buffer.as_mut_ptr(),
                        &mut len,
                        null(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                names.push(from_wide(&buffer[..len as usize]));
            }
            names
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful `RegOpenKeyExW` and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn display_device() -> DISPLAY_DEVICEW {
        // SAFETY: DISPLAY_DEVICEW is plain integers and arrays, for which all-zero is valid.
        let mut device: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        device
    }

    /// Device ids of the active monitors attached to the adapter called `adapter_name`.
    fn monitors_of(adapter_name: &[u16; 32]) -> Vec<String> {
        let mut ids = Vec::new();
        for index in 0..MAX_MONITORS_PER_ADAPTER {
            let mut monitor = display_device();
            // SAFETY: `adapter_name` is the NUL-terminated name Windows returned and `monitor` has `cb` set.
            let ok = unsafe { EnumDisplayDevicesW(adapter_name.as_ptr(), index, &mut monitor, 0) };
            if ok == 0 {
                break;
            }
            if monitor.StateFlags & DISPLAY_DEVICE_ACTIVE != 0 {
                ids.push(from_wide(&monitor.DeviceID));
            }
        }
        ids
    }

    fn adapter_named(name: &str) -> Option<DISPLAY_DEVICEW> {
        for index in 0..MAX_ADAPTERS {
            let mut adapter = display_device();
            // SAFETY: a null device name enumerates adapters; `adapter` has `cb` set.
            let ok = unsafe { EnumDisplayDevicesW(null(), index, &mut adapter, 0) };
            if ok == 0 {
                return None;
            }
            if from_wide(&adapter.DeviceName).eq_ignore_ascii_case(name) {
                return Some(adapter);
            }
        }
        None
    }

    /// Reads the `EDID` value of the monitor instance whose `Driver` value is `driver`.
    fn read_edid(hardware_id: &str, driver: &str) -> Option<Vec<u8>> {
        let display = Key::open(
            HKEY_LOCAL_MACHINE,
            &format!(r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{hardware_id}"),
        )?;
        for instance in display.subkeys() {
            let Some(key) = Key::open(display.0, &instance) else {
                continue;
            };
            if !key
                .string("Driver")
                .is_some_and(|d| d.eq_ignore_ascii_case(driver))
            {
                continue;
            }
            let parameters = Key::open(key.0, "Device Parameters")?;
            return parameters.value("EDID", REG_BINARY);
        }
        None
    }

    pub fn lookup(monitor_name: Option<&str>, size_px: (u32, u32)) -> DisplayLookup {
        let Some(name) = monitor_name.map(str::trim).filter(|n| !n.is_empty()) else {
            return DisplayLookup::none(LookupNote::NoMonitorName);
        };
        let Some(adapter) = adapter_named(name) else {
            return DisplayLookup::none(LookupNote::NotFound);
        };
        let ids = monitors_of(&adapter.DeviceName);
        let device_id = match ids.as_slice() {
            [] => return DisplayLookup::none(LookupNote::NotFound),
            [one] => one.clone(),
            // Mirrored displays share the adapter name; they may differ in size.
            many => {
                return DisplayLookup::none(LookupNote::Ambiguous {
                    candidates: many.len(),
                });
            }
        };
        let Some((hardware_id, driver)) = parse_monitor_device_id(&device_id) else {
            return DisplayLookup::none(LookupNote::NotFound);
        };
        let Some(bytes) = read_edid(&hardware_id, &driver) else {
            return DisplayLookup::none(LookupNote::NotFound);
        };
        let edid = match parse_edid(&bytes) {
            Ok(edid) => edid,
            Err(e) => return DisplayLookup::none(LookupNote::Invalid(e)),
        };
        match check_consistency(&edid, size_px, &[]) {
            Ok(()) => DisplayLookup {
                edid: Some(edid),
                source: LookupSource::WindowsRegistry(device_id),
                note: None,
            },
            Err(note) => DisplayLookup::none(note),
        }
    }
}

/// Looks up the EDID of the monitor the desktop calls `monitor_name` (the
/// connector name on Linux, `\\.\DISPLAYn` on Windows) whose current size is
/// `size_px`. Returns no EDID, with a note saying why, rather than a guess.
/// Never panics, reads only user-readable files and registry keys, and does no
/// network or device I/O.
pub fn lookup(monitor_name: Option<&str>, size_px: (u32, u32)) -> DisplayLookup {
    #[cfg(target_os = "linux")]
    {
        lookup_in_sysfs(Path::new("/sys/class/drm"), monitor_name, size_px)
    }
    #[cfg(windows)]
    {
        windows_registry::lookup(monitor_name, size_px)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (monitor_name, size_px);
        DisplayLookup::none(LookupNote::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spec {
        dtd_mm: (u16, u16),
        cm: (u8, u8),
        res: (u16, u16),
        name: Option<&'static str>,
        extension_blocks: u8,
    }

    impl Default for Spec {
        fn default() -> Self {
            Self {
                dtd_mm: (1210, 680),
                cm: (121, 68),
                res: (1920, 1080),
                name: Some("TEST TV"),
                extension_blocks: 0,
            }
        }
    }

    fn fix_checksum(block: &mut [u8]) {
        block[127] = 0;
        let sum = block.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        block[127] = 0u8.wrapping_sub(sum);
    }

    fn build(spec: &Spec) -> Vec<u8> {
        let mut e = vec![0u8; BLOCK_LEN * (1 + usize::from(spec.extension_blocks))];
        e[..8].copy_from_slice(&HEADER);
        // "SAM": S=19, A=1, M=13.
        e[8..10].copy_from_slice(&((19u16 << 10) | (1 << 5) | 13).to_be_bytes());
        e[10..12].copy_from_slice(&0x0F1Au16.to_le_bytes());
        e[21] = spec.cm.0;
        e[22] = spec.cm.1;
        e[126] = spec.extension_blocks;

        let (w, h) = (u32::from(spec.res.0), u32::from(spec.res.1));
        let (h_blank, v_blank) = (160u32, 62u32);
        let clock = (60.0 * f64::from((w + h_blank) * (h + v_blank)) / 10_000.0).round() as u16;
        let d = &mut e[FIRST_DESCRIPTOR..FIRST_DESCRIPTOR + DESCRIPTOR_LEN];
        d[0..2].copy_from_slice(&clock.to_le_bytes());
        d[2] = w as u8;
        d[3] = h_blank as u8;
        d[4] = (((w >> 8) as u8) << 4) | ((h_blank >> 8) as u8);
        d[5] = h as u8;
        d[6] = v_blank as u8;
        d[7] = (((h >> 8) as u8) << 4) | ((v_blank >> 8) as u8);
        d[12] = spec.dtd_mm.0 as u8;
        d[13] = spec.dtd_mm.1 as u8;
        d[14] = (((spec.dtd_mm.0 >> 8) as u8) << 4) | ((spec.dtd_mm.1 >> 8) as u8);

        if let Some(name) = spec.name {
            let at = FIRST_DESCRIPTOR + DESCRIPTOR_LEN;
            e[at + 3] = DESCRIPTOR_TAG_MONITOR_NAME;
            let mut text = name.as_bytes().to_vec();
            text.push(0x0A);
            text.resize(13, b' ');
            e[at + 5..at + 18].copy_from_slice(&text);
        }
        for block in e.as_chunks_mut::<BLOCK_LEN>().0 {
            fix_checksum(block);
        }
        e
    }

    #[test]
    fn millimeter_precision_comes_from_the_preferred_timing() {
        let info = parse_edid(&build(&Spec::default())).unwrap();
        assert_eq!(
            info.physical_size,
            PhysicalSize::Dimensions {
                width_mm: 1210,
                height_mm: 680,
                precision: SizePrecision::Millimeters
            }
        );
        assert_eq!(info.reliability, Reliability::Reliable);
        assert_eq!(info.manufacturer, "SAM");
        assert_eq!(info.product_code, 0x0F1A);
        assert_eq!(info.monitor_name.as_deref(), Some("TEST TV"));
        assert_eq!(info.native_resolution, Some((1920, 1080)));
        assert!(
            (info.refresh_hz.unwrap() - 60.0).abs() < 0.1,
            "{:?}",
            info.refresh_hz
        );
        assert!(info.checksums_valid);
        assert_eq!(
            info.known_size(),
            Some(KnownSize::Dimensions {
                width_m: 1.21,
                height_m: 0.68
            })
        );
    }

    #[test]
    fn twelve_bit_millimeter_fields_carry_sizes_above_255() {
        let info = parse_edid(&build(&Spec {
            dtd_mm: (3640, 2050),
            cm: (255, 205),
            res: (3840, 2160),
            ..Spec::default()
        }))
        .unwrap();
        assert!(matches!(
            info.physical_size,
            PhysicalSize::Dimensions {
                width_mm: 3640,
                height_mm: 2050,
                ..
            }
        ));
        assert_eq!(info.native_resolution, Some((3840, 2160)));
    }

    #[test]
    fn centimeter_bytes_are_the_fallback() {
        let info = parse_edid(&build(&Spec {
            dtd_mm: (0, 0),
            ..Spec::default()
        }))
        .unwrap();
        assert_eq!(
            info.physical_size,
            PhysicalSize::Dimensions {
                width_mm: 1210,
                height_mm: 680,
                precision: SizePrecision::Centimeters
            }
        );
        assert_eq!(info.reliability, Reliability::Reliable);
    }

    #[test]
    fn one_zero_size_byte_means_aspect_only() {
        let landscape = parse_edid(&build(&Spec {
            dtd_mm: (0, 0),
            cm: (77, 0),
            ..Spec::default()
        }))
        .unwrap();
        assert!(
            matches!(landscape.physical_size, PhysicalSize::AspectOnly(a) if (a - 1.76).abs() < 1e-9)
        );
        assert_eq!(landscape.reliability, Reliability::NoDimensions);
        assert_eq!(landscape.known_size(), None);

        let portrait = parse_edid(&build(&Spec {
            dtd_mm: (0, 0),
            cm: (0, 77),
            ..Spec::default()
        }))
        .unwrap();
        assert!(
            matches!(portrait.physical_size, PhysicalSize::AspectOnly(a) if (a - 100.0 / 176.0).abs() < 1e-9)
        );
    }

    #[test]
    fn both_zero_is_a_projector() {
        let info = parse_edid(&build(&Spec {
            dtd_mm: (0, 0),
            cm: (0, 0),
            ..Spec::default()
        }))
        .unwrap();
        assert_eq!(info.physical_size, PhysicalSize::Undefined);
        assert_eq!(info.known_size(), None);
        assert_eq!(info.physical_aspect(), None);
    }

    #[test]
    fn aspect_ratio_written_as_centimeters_is_a_placeholder() {
        for cm in [(16, 9), (16, 10), (4, 3)] {
            let res = if cm == (4, 3) {
                (1024, 768)
            } else if cm == (16, 10) {
                (1920, 1200)
            } else {
                (1920, 1080)
            };
            let info = parse_edid(&build(&Spec {
                dtd_mm: (0, 0),
                cm,
                res,
                ..Spec::default()
            }))
            .unwrap();
            assert_eq!(
                info.reliability,
                Reliability::Suspect(SuspectReason::AspectPlaceholder),
                "{cm:?}"
            );
            assert_eq!(info.known_size(), None);
        }
    }

    #[test]
    fn implausible_sizes_are_suspect() {
        let tiny = parse_edid(&build(&Spec {
            dtd_mm: (80, 45),
            cm: (8, 5),
            ..Spec::default()
        }))
        .unwrap();
        assert_eq!(
            tiny.reliability,
            Reliability::Suspect(SuspectReason::TooSmall)
        );
        assert_eq!(
            judge(
                PhysicalSize::Dimensions {
                    width_mm: 12_000,
                    height_mm: 6_750,
                    precision: SizePrecision::Millimeters
                },
                None
            ),
            Reliability::Suspect(SuspectReason::TooLarge)
        );
    }

    #[test]
    fn aspect_that_contradicts_the_native_resolution_is_suspect() {
        let info = parse_edid(&build(&Spec {
            dtd_mm: (700, 680),
            cm: (70, 68),
            ..Spec::default()
        }))
        .unwrap();
        assert!(
            matches!(
                info.reliability,
                Reliability::Suspect(SuspectReason::AspectMismatch { .. })
            ),
            "{:?}",
            info.reliability
        );
        // 2% off is inside the tolerance.
        let near = parse_edid(&build(&Spec {
            dtd_mm: (1234, 680),
            cm: (123, 68),
            ..Spec::default()
        }))
        .unwrap();
        assert_eq!(near.reliability, Reliability::Reliable);
    }

    #[test]
    fn bad_header_and_short_input_are_errors() {
        let mut bad = build(&Spec::default());
        bad[1] = 0;
        assert_eq!(parse_edid(&bad), Err(EdidError::BadHeader));
        assert_eq!(
            parse_edid(&bad[..100]),
            Err(EdidError::TooShort { len: 100 })
        );
    }

    #[test]
    fn bad_checksum_is_a_flag_not_a_failure() {
        let mut edid = build(&Spec::default());
        edid[127] = edid[127].wrapping_add(1);
        let info = parse_edid(&edid).unwrap();
        assert!(!info.checksums_valid);
        assert_eq!(info.reliability, Reliability::Reliable);
    }

    #[test]
    fn extension_blocks_are_checksummed_too() {
        let mut edid = build(&Spec {
            extension_blocks: 1,
            ..Spec::default()
        });
        assert!(parse_edid(&edid).unwrap().checksums_valid);
        edid[255] = edid[255].wrapping_add(1);
        assert!(!parse_edid(&edid).unwrap().checksums_valid);
        // A declared extension that is missing is a truncated EDID.
        let truncated = &build(&Spec {
            extension_blocks: 1,
            ..Spec::default()
        })[..BLOCK_LEN];
        assert!(!parse_edid(truncated).unwrap().checksums_valid);
    }

    #[test]
    fn monitor_name_without_descriptor_is_none() {
        let info = parse_edid(&build(&Spec {
            name: None,
            ..Spec::default()
        }))
        .unwrap();
        assert_eq!(info.monitor_name, None);
    }

    fn sysfs(entries: &[(&str, &Spec, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (connector, spec, modes) in entries {
            let path = dir.path().join(connector);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("edid"), build(spec)).unwrap();
            fs::write(path.join("modes"), modes).unwrap();
        }
        // A disconnected connector has an empty edid file, and card nodes have none.
        fs::create_dir_all(dir.path().join("card1-DP-2")).unwrap();
        fs::write(dir.path().join("card1-DP-2/edid"), []).unwrap();
        fs::create_dir_all(dir.path().join("card1")).unwrap();
        dir
    }

    #[test]
    fn sysfs_lookup_matches_connector_names() {
        let spec = Spec::default();
        let dir = sysfs(&[
            ("card1-DP-1", &spec, "1920x1080\n1280x720\n"),
            (
                "card1-HDMI-A-1",
                &Spec {
                    name: Some("OTHER"),
                    ..Spec::default()
                },
                "1920x1080\n",
            ),
        ]);
        let found = lookup_in_sysfs(dir.path(), Some("DP-1"), (1920, 1080));
        assert_eq!(
            found.edid.as_ref().and_then(|e| e.monitor_name.as_deref()),
            Some("TEST TV")
        );
        assert!(matches!(&found.source, LookupSource::Sysfs(p) if p.ends_with("card1-DP-1/edid")));
        assert_eq!(found.note, None);

        // X11 drivers drop the kernel's variant letter.
        let hdmi = lookup_in_sysfs(dir.path(), Some("HDMI-1"), (1920, 1080));
        assert_eq!(hdmi.edid.unwrap().monitor_name.as_deref(), Some("OTHER"));

        // Falls back to the EDID's own name.
        let by_name = lookup_in_sysfs(dir.path(), Some("test tv"), (1920, 1080));
        assert!(by_name.edid.is_some());
    }

    #[test]
    fn sysfs_lookup_accepts_other_listed_modes_and_scaled_sizes() {
        let dir = sysfs(&[("card1-DP-1", &Spec::default(), "1920x1080\n1280x1024\n")]);
        assert!(
            lookup_in_sysfs(dir.path(), Some("DP-1"), (1280, 1024))
                .edid
                .is_some(),
            "listed mode"
        );
        assert!(
            lookup_in_sysfs(dir.path(), Some("DP-1"), (1536, 864))
                .edid
                .is_some(),
            "scaled, same aspect"
        );
    }

    #[test]
    fn sysfs_lookup_refuses_to_guess() {
        let dir = sysfs(&[("card1-DP-1", &Spec::default(), "1920x1080\n")]);
        let wrong_size = lookup_in_sysfs(dir.path(), Some("DP-1"), (1024, 768));
        assert_eq!(wrong_size.edid, None);
        assert_eq!(
            wrong_size.note,
            Some(LookupNote::ResolutionMismatch {
                native: (1920, 1080),
                screen: (1024, 768)
            })
        );
        assert_eq!(
            lookup_in_sysfs(dir.path(), Some("DP-9"), (1920, 1080)).note,
            Some(LookupNote::NotFound)
        );
        assert_eq!(
            lookup_in_sysfs(dir.path(), None, (1920, 1080)).note,
            Some(LookupNote::NoMonitorName)
        );
        assert_eq!(
            lookup_in_sysfs(dir.path(), Some("DP-2"), (1920, 1080)).note,
            Some(LookupNote::NotFound),
            "disconnected connector"
        );

        let twins = sysfs(&[
            ("card1-DP-1", &Spec::default(), "1920x1080\n"),
            ("card1-DP-3", &Spec::default(), "1920x1080\n"),
        ]);
        assert_eq!(
            lookup_in_sysfs(twins.path(), Some("TEST TV"), (1920, 1080)).note,
            Some(LookupNote::Ambiguous { candidates: 2 })
        );

        let missing = lookup_in_sysfs(
            Path::new("/nonexistent/odyssey"),
            Some("DP-1"),
            (1920, 1080),
        );
        assert_eq!(
            missing.note,
            Some(LookupNote::Unreadable(io::ErrorKind::NotFound))
        );
    }

    #[test]
    fn mode_lines_parse_with_suffixes() {
        assert_eq!(parse_mode("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_mode("1920x1080i\n"), Some((1920, 1080)));
        assert_eq!(parse_mode("junk"), None);
    }

    #[test]
    fn windows_device_ids_split_into_hardware_id_and_driver() {
        let id = r"MONITOR\DELA0A1\{4d36e96e-e325-11ce-bfc1-08002be10318}\0004";
        assert_eq!(
            parse_monitor_device_id(id),
            Some((
                "DELA0A1".to_owned(),
                r"{4d36e96e-e325-11ce-bfc1-08002be10318}\0004".to_owned()
            ))
        );
        assert_eq!(parse_monitor_device_id(r"PCI\VEN_10DE"), None);
        assert_eq!(parse_monitor_device_id(r"MONITOR\\x"), None);
    }

    #[test]
    fn real_machine_lookup_never_panics() {
        // Whatever this machine has, the lookup answers without panicking or hanging.
        let _ = lookup(Some("DP-1"), (1920, 1080));
        let _ = lookup(None, (0, 0));
    }
}
