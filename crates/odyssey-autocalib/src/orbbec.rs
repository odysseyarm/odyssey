//! Orbbec depth cameras through the OrbbecSDK v2 C API, loaded at run time.
//!
//! The SDK is not linked. [`runtime_status`] looks for its shared library when
//! asked and [`OrbbecBackend`] fails with [`CameraError::RuntimeUnavailable`]
//! while none is usable, so an application built with this module starts and
//! runs without the SDK installed. [`runtime`] documents where the library is
//! looked for and which versions are accepted.
//!
//! The SDK is only alive while a camera is open: [`OrbbecBackend::list`] builds
//! a context, reads the device list and tears the context down again, so an idle
//! application owns no SDK threads. Every SDK object lives in a small RAII
//! wrapper that releases it on drop and holds a reference to the loaded
//! library, which keeps the library mapped for as long as any object exists.
//! The `unsafe` sits in those wrappers, in the [`ob!`] call macro and in the
//! IMU callback.

mod ffi;
pub mod runtime;

use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{GrayImage, RgbImage};
use nalgebra::{Matrix3, Vector3};
use parking_lot::Mutex;

use self::runtime::Sdk;
pub use self::runtime::{
    LIBRARY_NAME, RuntimeStatus, SDK_DIR_ENV, SUPPORTED_VERSIONS, SdkVersion, runtime_status,
};
use crate::camera::{
    CameraBackend, CameraError, CameraGeometry, CameraInfo, CameraStream, CaptureSettings,
    DepthImage, Extrinsics, Frames, ImuSample, Intrinsics,
};

/// Frame rate every video stream is requested at.
const STREAM_FPS: u32 = 30;
/// The IMU rate closest to this one is used; gravity estimation needs no more.
const IMU_TARGET_HZ: f64 = 100.0;
/// Upper bound on buffered IMU samples; the oldest drop first when nobody drains.
const IMU_BUFFER_LIMIT: usize = 8192;
const COLOR_SHARPNESS: i32 = 100;
/// `OB_PROP_COLOR_POWER_LINE_FREQUENCY_INT` value for 60 Hz mains.
const POWER_LINE_60_HZ: i32 = 2;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// Copies an SDK-owned C string; null becomes empty.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string valid for the call.
unsafe fn c_str(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Converts the SDK's out-parameter error into a `CameraError`, freeing it.
fn check(sdk: &Sdk, err: *mut ffi::ob_error) -> Result<(), CameraError> {
    if err.is_null() {
        return Ok(());
    }
    // SAFETY: a non-null `ob_error` from the SDK stays valid until
    // `ob_delete_error`, which runs exactly once here. The message and function
    // strings belong to the error object and are copied before it is deleted.
    let (message, function) = unsafe {
        let message = c_str(sdk.ob_error_get_message(err));
        let function = c_str(sdk.ob_error_get_function(err));
        sdk.ob_delete_error(err);
        (message, function)
    };
    Err(CameraError::Sdk(if function.is_empty() {
        message
    } else {
        format!("{function}: {message}")
    }))
}

/// Runs one SDK call with a fresh error out-parameter and turns a populated
/// error into `Err`.
fn ob<T>(sdk: &Sdk, call: impl FnOnce(*mut *mut ffi::ob_error) -> T) -> Result<T, CameraError> {
    let mut err: *mut ffi::ob_error = ptr::null_mut();
    let value = call(&mut err);
    check(sdk, err)?;
    Ok(value)
}

/// `ob!(sdk, ob_fn(a, b))` calls `sdk.ob_fn(a, b, &mut error)` and yields a
/// `Result`. `sdk` is a loaded [`Sdk`], usually the one inside a handle wrapper.
macro_rules! ob {
    ($sdk:expr_2021, $f:ident($($a:expr_2021),* $(,)?)) => {{
        let sdk: &Sdk = &$sdk;
        // SAFETY: every call site passes handles borrowed from the RAII
        // wrappers below, which keep them alive for the duration of the call,
        // and pointers to values that outlive it.
        ob(sdk, |e| unsafe { sdk.$f($($a,)* e) })
    }};
}

/// Logs a failed best-effort step and carries on.
fn best_effort<T>(what: &str, result: Result<T, CameraError>) -> Option<T> {
    result.map_err(|e| tracing::warn!("{what}: {e}")).ok()
}

/// One-time setup right after the library loads, before any context exists.
///
/// Keeps the SDK from writing a `Log` directory next to the application, and
/// points it at its `extensions/` when the loader knows where the library came
/// from. Errors only mean the SDK keeps its defaults.
fn initialize(sdk: &Sdk, extensions: Option<&Path>) {
    let no_directory = CString::default();
    best_effort(
        "ob_set_logger_to_file",
        ob!(
            sdk,
            ob_set_logger_to_file(
                ffi::OBLogSeverity_OB_LOG_SEVERITY_OFF,
                no_directory.as_ptr()
            )
        ),
    );
    let Some(dir) = extensions else { return };
    match dir.to_str().map(CString::new) {
        Some(Ok(dir_c)) => {
            best_effort(
                "ob_set_extensions_directory",
                ob!(sdk, ob_set_extensions_directory(dir_c.as_ptr())),
            );
        }
        _ => tracing::warn!(
            "extensions directory {} cannot be passed to the SDK; it keeps its own lookup",
            dir.display()
        ),
    }
}

// ---------------------------------------------------------------------------
// RAII handles
// ---------------------------------------------------------------------------

/// Declares an owning wrapper around an SDK handle that deletes it on drop.
/// The wrapper holds the [`Sdk`] it came from so the library outlives it.
macro_rules! handle {
    ($name:ident, $raw:ty, $delete:ident) => {
        struct $name {
            sdk: Sdk,
            ptr: NonNull<$raw>,
        }

        impl $name {
            /// Takes ownership of a handle the SDK returned.
            fn new(sdk: &Sdk, p: *mut $raw, what: &str) -> Result<Self, CameraError> {
                NonNull::new(p)
                    .map(|ptr| Self {
                        sdk: Arc::clone(sdk),
                        ptr,
                    })
                    .ok_or_else(|| CameraError::Sdk(format!("{what}: the SDK returned no object")))
            }

            fn as_ptr(&self) -> *mut $raw {
                self.ptr.as_ptr()
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: `self` owns the single reference the SDK handed out
                // and drop runs once, so the handle is valid and freed once.
                let result = ob!(self.sdk, $delete(self.ptr.as_ptr() as _));
                best_effort(stringify!($delete), result);
            }
        }
    };
}

handle!(Context, ffi::ob_context, ob_delete_context);
handle!(DeviceList, ffi::ob_device_list, ob_delete_device_list);
handle!(Device, ffi::ob_device, ob_delete_device);
handle!(Pipeline, ffi::ob_pipeline, ob_delete_pipeline);
handle!(Config, ffi::ob_config, ob_delete_config);
handle!(
    ProfileList,
    ffi::ob_stream_profile_list,
    ob_delete_stream_profile_list
);
handle!(Profile, ffi::ob_stream_profile, ob_delete_stream_profile);
handle!(SensorList, ffi::ob_sensor_list, ob_delete_sensor_list);
handle!(Sensor, ffi::ob_sensor, ob_delete_sensor);
handle!(Filter, ffi::ob_filter, ob_delete_filter);
handle!(Frame, ffi::ob_frame, ob_delete_frame);

impl Context {
    fn create(sdk: &Sdk) -> Result<Self, CameraError> {
        Self::new(sdk, ob!(sdk, ob_create_context())?, "ob_create_context")
    }
}

impl DeviceList {
    fn query(ctx: &Context) -> Result<Self, CameraError> {
        Self::new(
            &ctx.sdk,
            ob!(ctx.sdk, ob_query_device_list(ctx.as_ptr()))?,
            "ob_query_device_list",
        )
    }

    fn len(&self) -> Result<u32, CameraError> {
        ob!(self.sdk, ob_device_list_get_count(self.as_ptr()))
    }

    fn uid(&self, index: u32) -> Result<String, CameraError> {
        let p = ob!(
            self.sdk,
            ob_device_list_get_device_uid(self.as_ptr(), index)
        )?;
        // SAFETY: the string lives in the list, which `self` keeps alive; `c_str` copies it.
        Ok(unsafe { c_str(p) })
    }

    fn name(&self, index: u32) -> Result<String, CameraError> {
        let p = ob!(
            self.sdk,
            ob_device_list_get_device_name(self.as_ptr(), index)
        )?;
        // SAFETY: the string lives in the list, which `self` keeps alive; `c_str` copies it.
        Ok(unsafe { c_str(p) })
    }

    fn serial(&self, index: u32) -> Result<String, CameraError> {
        let p = ob!(
            self.sdk,
            ob_device_list_get_device_serial_number(self.as_ptr(), index)
        )?;
        // SAFETY: the string lives in the list, which `self` keeps alive; `c_str` copies it.
        Ok(unsafe { c_str(p) })
    }
}

// ---------------------------------------------------------------------------
// Device properties
// ---------------------------------------------------------------------------

impl Device {
    fn supports(&self, id: ffi::ob_property_id) -> bool {
        ob!(
            self.sdk,
            ob_device_is_property_supported(
                self.as_ptr(),
                id,
                ffi::OBPermissionType_OB_PERMISSION_READ_WRITE
            )
        )
        .unwrap_or(false)
    }

    fn set_bool(&self, id: ffi::ob_property_id, value: bool) -> Result<(), CameraError> {
        ob!(
            self.sdk,
            ob_device_set_bool_property(self.as_ptr(), id, value)
        )
    }

    fn get_bool(&self, id: ffi::ob_property_id) -> Result<bool, CameraError> {
        ob!(self.sdk, ob_device_get_bool_property(self.as_ptr(), id))
    }

    fn get_int(&self, id: ffi::ob_property_id) -> Result<i32, CameraError> {
        ob!(self.sdk, ob_device_get_int_property(self.as_ptr(), id))
    }

    fn set_int(&self, id: ffi::ob_property_id, value: i32) -> Result<(), CameraError> {
        ob!(
            self.sdk,
            ob_device_set_int_property(self.as_ptr(), id, value)
        )
    }

    /// Sets an integer property after fitting `value` into the range and step
    /// the camera reports, since out-of-range writes fail. Returns the value
    /// written.
    fn set_int_clamped(&self, id: ffi::ob_property_id, value: i32) -> Result<i32, CameraError> {
        let range = ob!(
            self.sdk,
            ob_device_get_int_property_range(self.as_ptr(), id)
        )?;
        let mut fitted = value;
        if range.min <= range.max {
            fitted = fitted.clamp(range.min, range.max);
            if range.step > 1 {
                fitted = range.min + (fitted - range.min) / range.step * range.step;
            }
        }
        self.set_int(id, fitted)?;
        Ok(fitted)
    }

    fn sensor_list(&self) -> Result<SensorList, CameraError> {
        SensorList::new(
            &self.sdk,
            ob!(self.sdk, ob_device_get_sensor_list(self.as_ptr()))?,
            "ob_device_get_sensor_list",
        )
    }

    /// Applies the capture settings the calibration depends on. Settings persist
    /// in the camera across sessions, so every one is written explicitly.
    fn apply_capture_settings(&self, settings: &CaptureSettings) -> Result<(), CameraError> {
        if self.supports(ffi::OBPropertyID_OB_PROP_COLOR_SHARPNESS_INT) {
            let result = self.set_int_clamped(
                ffi::OBPropertyID_OB_PROP_COLOR_SHARPNESS_INT,
                COLOR_SHARPNESS,
            );
            best_effort("color sharpness", result);
        }
        if self.supports(ffi::OBPropertyID_OB_PROP_COLOR_POWER_LINE_FREQUENCY_INT) {
            let result = self.set_int(
                ffi::OBPropertyID_OB_PROP_COLOR_POWER_LINE_FREQUENCY_INT,
                POWER_LINE_60_HZ,
            );
            best_effort("color power-line frequency", result);
        }
        if self.supports(ffi::OBPropertyID_OB_PROP_COLOR_AUTO_EXPOSURE_BOOL) {
            self.set_bool(ffi::OBPropertyID_OB_PROP_COLOR_AUTO_EXPOSURE_BOOL, false)?;
        } else {
            tracing::warn!("camera has no color auto-exposure switch; color exposure may drift");
        }
        let to_i32 = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
        if self.supports(ffi::OBPropertyID_OB_PROP_COLOR_GAIN_INT) {
            self.set_int_clamped(
                ffi::OBPropertyID_OB_PROP_COLOR_GAIN_INT,
                to_i32(settings.color_gain),
            )?;
        }
        if self.supports(ffi::OBPropertyID_OB_PROP_COLOR_EXPOSURE_INT) {
            self.set_int_clamped(
                ffi::OBPropertyID_OB_PROP_COLOR_EXPOSURE_INT,
                to_i32(settings.color_exposure_us),
            )?;
        }
        self.restore_ir_auto_exposure()
    }

    fn restore_ir_auto_exposure(&self) -> Result<(), CameraError> {
        if self.supports(ffi::OBPropertyID_OB_PROP_IR_AUTO_EXPOSURE_BOOL) {
            self.set_bool(ffi::OBPropertyID_OB_PROP_IR_AUTO_EXPOSURE_BOOL, true)?;
        }
        Ok(())
    }
}

/// How a camera switches its depth projector, with the state to restore.
#[derive(Clone, Copy)]
enum Projector {
    /// `OB_PROP_LASER_CONTROL_INT`: 0 off, 1 on, 2 auto.
    Control { original: i32 },
    /// `OB_PROP_LASER_BOOL`.
    Switch { original: bool },
}

impl Projector {
    fn detect(device: &Device) -> Option<Self> {
        if device.supports(ffi::OBPropertyID_OB_PROP_LASER_CONTROL_INT) {
            let original = device
                .get_int(ffi::OBPropertyID_OB_PROP_LASER_CONTROL_INT)
                .ok()?;
            return Some(Self::Control { original });
        }
        if device.supports(ffi::OBPropertyID_OB_PROP_LASER_BOOL) {
            let original = device.get_bool(ffi::OBPropertyID_OB_PROP_LASER_BOOL).ok()?;
            return Some(Self::Switch { original });
        }
        None
    }

    /// Turns the projector off, or back to the state the camera started in.
    /// A camera that started with the projector off turns on when asked.
    fn set(self, device: &Device, enabled: bool) -> Result<(), CameraError> {
        match self {
            Self::Control { original } => {
                let value = match (enabled, original) {
                    (false, _) => 0,
                    (true, 0) => 1,
                    (true, original) => original,
                };
                device.set_int(ffi::OBPropertyID_OB_PROP_LASER_CONTROL_INT, value)
            }
            Self::Switch { .. } => device.set_bool(ffi::OBPropertyID_OB_PROP_LASER_BOOL, enabled),
        }
    }

    fn restore(self, device: &Device) -> Result<(), CameraError> {
        match self {
            Self::Control { original } => {
                device.set_int(ffi::OBPropertyID_OB_PROP_LASER_CONTROL_INT, original)
            }
            Self::Switch { original } => {
                device.set_bool(ffi::OBPropertyID_OB_PROP_LASER_BOOL, original)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stream profiles and calibration
// ---------------------------------------------------------------------------

impl Profile {
    fn format(&self) -> Result<ffi::ob_format, CameraError> {
        ob!(self.sdk, ob_stream_profile_get_format(self.as_ptr()))
    }

    fn fps(&self) -> Result<u32, CameraError> {
        ob!(self.sdk, ob_video_stream_profile_get_fps(self.as_ptr()))
    }

    fn size(&self) -> Result<(u32, u32), CameraError> {
        Ok((
            ob!(self.sdk, ob_video_stream_profile_get_width(self.as_ptr()))?,
            ob!(self.sdk, ob_video_stream_profile_get_height(self.as_ptr()))?,
        ))
    }

    /// Takes points from this stream's camera frame to `target`'s.
    fn extrinsics_to(&self, target: &Profile) -> Result<Extrinsics, CameraError> {
        let e = ob!(
            self.sdk,
            ob_stream_profile_get_extrinsic_to(self.as_ptr(), target.as_ptr())
        )?;
        let r = e.rot;
        let rotation = Matrix3::from_row_slice(&r.map(f64::from));
        // The SDK reports translations in millimeters.
        let translation = Vector3::new(
            f64::from(e.trans[0]),
            f64::from(e.trans[1]),
            f64::from(e.trans[2]),
        ) / 1000.0;
        Ok(Extrinsics {
            rotation,
            translation,
        })
    }

    fn intrinsics(&self) -> Result<Intrinsics, CameraError> {
        let i = ob!(
            self.sdk,
            ob_video_stream_profile_get_intrinsic(self.as_ptr())
        )?;
        let d = ob!(
            self.sdk,
            ob_video_stream_profile_get_distortion(self.as_ptr())
        )?;
        let (width, height) = self.size()?;
        let all = [d.k1, d.k2, d.k3, d.k4, d.k5, d.k6].map(f64::from);
        let k = match d.model {
            ffi::OBCameraDistortionModel_OB_DISTORTION_NONE => [0.0; 6],
            ffi::OBCameraDistortionModel_OB_DISTORTION_BROWN_CONRADY
            | ffi::OBCameraDistortionModel_OB_DISTORTION_MODIFIED_BROWN_CONRADY => {
                [all[0], all[1], all[2], 0.0, 0.0, 0.0]
            }
            ffi::OBCameraDistortionModel_OB_DISTORTION_BROWN_CONRADY_K6 => all,
            _ => {
                return Err(CameraError::Unsupported(
                    "Brown-Conrady lens distortion model",
                ));
            }
        };
        let p = if d.model == ffi::OBCameraDistortionModel_OB_DISTORTION_NONE {
            [0.0; 2]
        } else {
            [f64::from(d.p1), f64::from(d.p2)]
        };
        Ok(Intrinsics {
            width,
            height,
            fx: f64::from(i.fx),
            fy: f64::from(i.fy),
            cx: f64::from(i.cx),
            cy: f64::from(i.cy),
            k,
            p,
        })
    }
}

/// A profile chosen for one stream.
struct Choice {
    profile: Profile,
    format: ffi::ob_format,
}

impl ProfileList {
    fn for_sensor_type(
        pipeline: &Pipeline,
        sensor: ffi::ob_sensor_type,
    ) -> Result<Self, CameraError> {
        Self::new(
            &pipeline.sdk,
            ob!(
                pipeline.sdk,
                ob_pipeline_get_stream_profile_list(pipeline.as_ptr(), sensor)
            )?,
            "ob_pipeline_get_stream_profile_list",
        )
    }

    fn for_sensor(sensor: &Sensor) -> Result<Self, CameraError> {
        Self::new(
            &sensor.sdk,
            ob!(
                sensor.sdk,
                ob_sensor_get_stream_profile_list(sensor.as_ptr())
            )?,
            "ob_sensor_get_stream_profile_list",
        )
    }

    fn len(&self) -> Result<u32, CameraError> {
        ob!(self.sdk, ob_stream_profile_list_get_count(self.as_ptr()))
    }

    fn get(&self, index: u32) -> Result<Profile, CameraError> {
        Profile::new(
            &self.sdk,
            ob!(
                self.sdk,
                ob_stream_profile_list_get_profile(self.as_ptr(), index as i32)
            )?,
            "ob_stream_profile_list_get_profile",
        )
    }

    /// The largest-area profile at [`STREAM_FPS`] in the first of `formats`
    /// that has any.
    fn best(&self, formats: &[ffi::ob_format]) -> Result<Option<Choice>, CameraError> {
        let count = self.len()?;
        for &wanted in formats {
            let mut best: Option<(u64, Profile)> = None;
            for i in 0..count {
                let profile = self.get(i)?;
                if profile.format()? != wanted || profile.fps()? != STREAM_FPS {
                    continue;
                }
                let (w, h) = profile.size()?;
                let area = u64::from(w) * u64::from(h);
                if best.as_ref().is_none_or(|(a, _)| area > *a) {
                    best = Some((area, profile));
                }
            }
            if let Some((_, profile)) = best {
                return Ok(Some(Choice {
                    profile,
                    format: wanted,
                }));
            }
        }
        Ok(None)
    }
}

/// Color formats the SDK can turn into RGB, with the filter conversion to use.
/// Native RGB comes first so no conversion runs when the camera offers it.
const COLOR_FORMATS: [(ffi::ob_format, Option<ffi::ob_convert_format>); 6] = [
    (ffi::OBFormat_OB_FORMAT_RGB, None),
    (
        ffi::OBFormat_OB_FORMAT_MJPG,
        Some(ffi::OBConvertFormat_FORMAT_MJPG_TO_RGB),
    ),
    (
        ffi::OBFormat_OB_FORMAT_YUYV,
        Some(ffi::OBConvertFormat_FORMAT_YUYV_TO_RGB),
    ),
    (
        ffi::OBFormat_OB_FORMAT_UYVY,
        Some(ffi::OBConvertFormat_FORMAT_UYVY_TO_RGB),
    ),
    (
        ffi::OBFormat_OB_FORMAT_NV12,
        Some(ffi::OBConvertFormat_FORMAT_NV12_TO_RGB),
    ),
    (
        ffi::OBFormat_OB_FORMAT_I420,
        Some(ffi::OBConvertFormat_FORMAT_I420_TO_RGB),
    ),
];

/// Which infrared imager feeds marker detection.
#[derive(Clone, Copy)]
struct IrSource {
    sensor: ffi::ob_sensor_type,
    frame: ffi::ob_frame_type,
}

const IR_SOURCES: [IrSource; 2] = [
    IrSource {
        sensor: ffi::OBSensorType_OB_SENSOR_IR_LEFT,
        frame: ffi::OBFrameType_OB_FRAME_IR_LEFT,
    },
    IrSource {
        sensor: ffi::OBSensorType_OB_SENSOR_IR,
        frame: ffi::OBFrameType_OB_FRAME_IR,
    },
];

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

impl Frame {
    /// Wraps a frame the SDK handed out, or `None` for null.
    fn adopt(sdk: &Sdk, raw: *mut ffi::ob_frame) -> Option<Self> {
        NonNull::new(raw).map(|ptr| Self {
            sdk: Arc::clone(sdk),
            ptr,
        })
    }

    fn data(&self) -> Result<&[u8], CameraError> {
        let len = ob!(self.sdk, ob_frame_get_data_size(self.as_ptr()))? as usize;
        let data = ob!(self.sdk, ob_frame_get_data(self.as_ptr()))?;
        if data.is_null() || len == 0 {
            return Err(CameraError::Sdk("frame carries no data".into()));
        }
        // SAFETY: the SDK documents `data` as `len` readable bytes owned by the
        // frame; the slice borrows `self`, which keeps the frame alive.
        Ok(unsafe { slice::from_raw_parts(data, len) })
    }

    fn format(&self) -> Result<ffi::ob_format, CameraError> {
        ob!(self.sdk, ob_frame_get_format(self.as_ptr()))
    }

    /// Row-major pixel bytes of an uncompressed video frame with row padding
    /// removed. `bytes_per_pixel` is the pixel format's width.
    fn pixels(&self, bytes_per_pixel: usize) -> Result<(u32, u32, Vec<u8>), CameraError> {
        let w = ob!(self.sdk, ob_video_frame_get_width(self.as_ptr()))?;
        let h = ob!(self.sdk, ob_video_frame_get_height(self.as_ptr()))?;
        let (wu, hu) = (w as usize, h as usize);
        let data = self.data()?;
        let row = wu * bytes_per_pixel;
        if hu == 0 || data.len() < row * hu {
            return Err(CameraError::Sdk(format!(
                "frame of {w}x{h} holds {} bytes, expected at least {}",
                data.len(),
                row * hu
            )));
        }
        // The SDK exposes no stride, so derive it from the payload size.
        let stride = data.len() / hu;
        let mut out = Vec::with_capacity(row * hu);
        for y in 0..hu {
            out.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        Ok((w, h, out))
    }

    /// The sub-frame of a frameset, or `None` if the set lacks that stream.
    fn sub_frame(&self, kind: ffi::ob_frame_type) -> Option<Frame> {
        let raw = ob!(self.sdk, ob_frameset_get_frame(self.as_ptr(), kind)).ok()?;
        Frame::adopt(&self.sdk, raw)
    }
}

fn gray_from_frame(frame: &Frame) -> Result<GrayImage, CameraError> {
    if frame.format()? != ffi::OBFormat_OB_FORMAT_Y8 {
        return Err(CameraError::Unsupported("8-bit infrared frames"));
    }
    let (w, h, buf) = frame.pixels(1)?;
    GrayImage::from_raw(w, h, buf)
        .ok_or_else(|| CameraError::Sdk("infrared frame size mismatch".into()))
}

fn depth_from_frame(frame: &Frame) -> Result<(DepthImage, f32), CameraError> {
    if frame.format()? != ffi::OBFormat_OB_FORMAT_Y16 {
        return Err(CameraError::Unsupported("16-bit depth frames"));
    }
    let (w, h, bytes) = frame.pixels(2)?;
    let raw: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let image = DepthImage::from_raw(w, h, raw)
        .ok_or_else(|| CameraError::Sdk("depth frame size mismatch".into()))?;
    // The SDK's value scale is millimeters per raw unit.
    let scale_mm = ob!(frame.sdk, ob_depth_frame_get_value_scale(frame.as_ptr()))?;
    Ok((image, scale_mm / 1000.0))
}

// ---------------------------------------------------------------------------
// IMU
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ImuState {
    /// Latest accelerometer reading in the color frame, held until the next.
    accel: Option<Vector3<f64>>,
    samples: VecDeque<ImuSample>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImuKind {
    Accel,
    Gyro,
}

/// Per-sensor state the SDK callback reads through its `user_data` pointer.
struct ImuCallbackCtx {
    kind: ImuKind,
    /// Rotates this sensor's vectors into the color camera's frame.
    to_color: Matrix3<f64>,
    shared: Arc<Mutex<ImuState>>,
    /// The callback calls the SDK; this keeps the library loaded until the
    /// context is freed.
    sdk: Sdk,
}

fn imu_hz(rate: ffi::ob_accel_sample_rate) -> f64 {
    match rate {
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_1_5625_HZ => 1.5625,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_3_125_HZ => 3.125,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_6_25_HZ => 6.25,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_12_5_HZ => 12.5,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_25_HZ => 25.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_50_HZ => 50.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_100_HZ => 100.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_200_HZ => 200.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_400_HZ => 400.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_500_HZ => 500.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_800_HZ => 800.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_1_KHZ => 1000.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_2_KHZ => 2000.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_4_KHZ => 4000.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_8_KHZ => 8000.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_16_KHZ => 16000.0,
        ffi::OBIMUSampleRate_OB_SAMPLE_RATE_32_KHZ => 32000.0,
        _ => f64::INFINITY,
    }
}

/// SDK entry point for IMU frames. It owns the frame it receives.
unsafe extern "C" fn imu_callback(frame: *mut ffi::ob_frame, user_data: *mut c_void) {
    // SAFETY: `user_data` is the `ImuCallbackCtx` leaked in `ImuSensor::start`.
    // It is freed only after `ob_sensor_stop` returns, which ends callbacks.
    let ctx = unsafe { &*(user_data as *const ImuCallbackCtx) };
    let Some(frame) = Frame::adopt(&ctx.sdk, frame) else {
        return;
    };
    // A panic must not unwind into the SDK's C++ threads.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Ok(timestamp_us) = ob!(ctx.sdk, ob_frame_get_timestamp_us(frame.as_ptr())) else {
            return;
        };
        let read = match ctx.kind {
            ImuKind::Accel => {
                ob!(ctx.sdk, ob_accel_frame_get_value(frame.as_ptr())).map(|v| [v.x, v.y, v.z])
            }
            ImuKind::Gyro => {
                ob!(ctx.sdk, ob_gyro_frame_get_value(frame.as_ptr())).map(|v| [v.x, v.y, v.z])
            }
        };
        let Ok([x, y, z]) = read else { return };
        let v = ctx.to_color * Vector3::new(f64::from(x), f64::from(y), f64::from(z));
        let mut state = ctx.shared.lock();
        match ctx.kind {
            ImuKind::Accel => state.accel = Some(v),
            // Gyro drives sample timing; it holds the latest accelerometer reading.
            ImuKind::Gyro => {
                if let Some(accel) = state.accel {
                    if state.samples.len() >= IMU_BUFFER_LIMIT {
                        state.samples.pop_front();
                    }
                    state.samples.push_back(ImuSample {
                        timestamp: Duration::from_micros(timestamp_us),
                        gyro: v,
                        accel,
                    });
                }
            }
        }
    }));
}

/// A running IMU sensor. Dropping it stops the sensor and frees the callback
/// context, then releases the sensor handle.
struct ImuSensor {
    ctx: *mut ImuCallbackCtx,
    sensor: Sensor,
}

impl ImuSensor {
    fn start(
        sensors: &SensorList,
        kind: ImuKind,
        color: &Profile,
        shared: &Arc<Mutex<ImuState>>,
    ) -> Result<Option<Self>, CameraError> {
        let sdk = &sensors.sdk;
        let wanted = match kind {
            ImuKind::Accel => ffi::OBSensorType_OB_SENSOR_ACCEL,
            ImuKind::Gyro => ffi::OBSensorType_OB_SENSOR_GYRO,
        };
        let count = ob!(sdk, ob_sensor_list_get_count(sensors.as_ptr()))?;
        let mut present = false;
        for i in 0..count {
            present |= ob!(sdk, ob_sensor_list_get_sensor_type(sensors.as_ptr(), i))? == wanted;
        }
        if !present {
            return Ok(None);
        }
        let sensor = Sensor::new(
            sdk,
            ob!(
                sdk,
                ob_sensor_list_get_sensor_by_type(sensors.as_ptr(), wanted)
            )?,
            "IMU sensor",
        )?;

        let profiles = ProfileList::for_sensor(&sensor)?;
        let mut best: Option<(f64, Profile)> = None;
        for i in 0..profiles.len()? {
            let profile = profiles.get(i)?;
            let rate = match kind {
                ImuKind::Accel => ob!(
                    sdk,
                    ob_accel_stream_profile_get_sample_rate(profile.as_ptr())
                )?,
                ImuKind::Gyro => ob!(
                    sdk,
                    ob_gyro_stream_profile_get_sample_rate(profile.as_ptr())
                )?,
            };
            let distance = (imu_hz(rate) - IMU_TARGET_HZ).abs();
            if best.as_ref().is_none_or(|(d, _)| distance < *d) {
                best = Some((distance, profile));
            }
        }
        let Some((_, profile)) = best else {
            return Ok(None);
        };

        let to_color = profile.extrinsics_to(color)?.rotation;
        let ctx = Box::into_raw(Box::new(ImuCallbackCtx {
            kind,
            to_color,
            shared: Arc::clone(shared),
            sdk: Arc::clone(sdk),
        }));
        // SAFETY: `ctx` stays allocated until `ImuSensor::drop`, which stops the
        // sensor first; the callback is the only other user of it.
        let started = ob!(
            sdk,
            ob_sensor_start(
                sensor.as_ptr(),
                profile.as_ptr(),
                Some(imu_callback),
                ctx as *mut c_void
            )
        );
        if let Err(e) = started {
            // SAFETY: the sensor never started, so no callback holds `ctx`.
            drop(unsafe { Box::from_raw(ctx) });
            return Err(e);
        }
        Ok(Some(Self { ctx, sensor }))
    }
}

impl Drop for ImuSensor {
    fn drop(&mut self) {
        best_effort(
            "ob_sensor_stop",
            ob!(self.sensor.sdk, ob_sensor_stop(self.sensor.as_ptr())),
        );
        // SAFETY: `ctx` came from `Box::into_raw` and the sensor is stopped, so
        // no callback can still be reading it.
        drop(unsafe { Box::from_raw(self.ctx) });
    }
}

// ---------------------------------------------------------------------------
// Backend and stream
// ---------------------------------------------------------------------------

/// Lists and opens Orbbec depth cameras.
///
/// Every call first makes sure the SDK library is loaded, and fails with
/// [`CameraError::RuntimeUnavailable`] when it cannot be; check
/// [`runtime_status`] to tell the reasons apart.
#[derive(Debug, Default, Clone)]
pub struct OrbbecBackend {
    extra_dir: Option<PathBuf>,
}

impl OrbbecBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend that also looks for the SDK library in `extra_dir`, after the
    /// environment variable and the system loader path (see [`runtime`]).
    pub fn with_extra_dir(extra_dir: Option<PathBuf>) -> Self {
        Self { extra_dir }
    }

    fn sdk(&self) -> Result<Sdk, CameraError> {
        runtime::load(self.extra_dir.as_deref())
            .map(|loaded| loaded.sdk)
            .map_err(|status| CameraError::RuntimeUnavailable(status.to_string()))
    }
}

impl CameraBackend for OrbbecBackend {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError> {
        // The context owns SDK threads; it lives only for this enumeration.
        let context = Context::create(&self.sdk()?)?;
        let devices = DeviceList::query(&context)?;
        (0..devices.len()?)
            .map(|i| {
                Ok(CameraInfo {
                    id: devices.uid(i)?,
                    name: devices.name(i)?,
                    serial: devices.serial(i)?,
                })
            })
            .collect()
    }

    fn open(
        &self,
        camera: &CameraInfo,
        settings: &CaptureSettings,
    ) -> Result<Box<dyn CameraStream>, CameraError> {
        Ok(Box::new(OrbbecStream::open(self.sdk()?, camera, settings)?))
    }
}

/// An open, streaming camera.
///
/// Fields drop in declaration order after [`Drop::drop`]: sensors stop before
/// the pipeline, the pipeline before its device, and the context goes last.
/// Each field holds the loaded library, so it outlives every one of them.
pub struct OrbbecStream {
    imu: Vec<ImuSensor>,
    imu_state: Arc<Mutex<ImuState>>,
    converter: Option<Filter>,
    ir_frame: ffi::ob_frame_type,
    projector: Option<Projector>,
    pipeline: Pipeline,
    pipeline_started: bool,
    device: Device,
    geometry: CameraGeometry,
    _devices: DeviceList,
    _context: Context,
}

// SAFETY: the raw SDK handles are plain pointers to objects the SDK documents as
// usable from any thread, and `OrbbecStream` is not `Sync`, so a single thread
// uses them at a time. The IMU callback context is shared only through a mutex.
unsafe impl Send for OrbbecStream {}

impl OrbbecStream {
    fn open(
        sdk: Sdk,
        camera: &CameraInfo,
        settings: &CaptureSettings,
    ) -> Result<Self, CameraError> {
        let context = Context::create(&sdk)?;
        let devices = DeviceList::query(&context)?;
        let uid = CString::new(camera.id.as_str()).map_err(|_| CameraError::NoDevice)?;
        let raw = ob!(
            sdk,
            ob_device_list_get_device_by_uid(devices.as_ptr(), uid.as_ptr())
        );
        let device = match raw {
            Ok(raw) => Device::new(&sdk, raw, "ob_device_list_get_device_by_uid")?,
            Err(_) => return Err(CameraError::NoDevice),
        };
        let pipeline = Pipeline::new(
            &sdk,
            ob!(sdk, ob_create_pipeline_with_device(device.as_ptr()))?,
            "ob_create_pipeline_with_device",
        )?;

        device.apply_capture_settings(settings)?;

        // Profiles: color (preferring native RGB), left infrared Y8, depth Y16.
        let color_formats: Vec<_> = COLOR_FORMATS.iter().map(|(f, _)| *f).collect();
        let color_list = ProfileList::for_sensor_type(&pipeline, ffi::OBSensorType_OB_SENSOR_COLOR)
            .map_err(|_| CameraError::Unsupported("a color stream"))?;
        let color = color_list
            .best(&color_formats)?
            .ok_or(CameraError::Unsupported(
                "a 30 fps RGB-convertible color stream",
            ))?;
        let convert = COLOR_FORMATS
            .iter()
            .find(|(f, _)| *f == color.format)
            .and_then(|(_, c)| *c);

        let mut ir = None;
        for source in IR_SOURCES {
            let Ok(list) = ProfileList::for_sensor_type(&pipeline, source.sensor) else {
                continue;
            };
            if let Some(choice) = list.best(&[ffi::OBFormat_OB_FORMAT_Y8])? {
                ir = Some((source, choice));
                break;
            }
        }
        let (ir_source, ir) = ir.ok_or(CameraError::Unsupported("a 30 fps Y8 infrared stream"))?;

        let depth_list = ProfileList::for_sensor_type(&pipeline, ffi::OBSensorType_OB_SENSOR_DEPTH)
            .map_err(|_| CameraError::Unsupported("a depth stream"))?;
        let depth = depth_list
            .best(&[ffi::OBFormat_OB_FORMAT_Y16])?
            .ok_or(CameraError::Unsupported("a 30 fps Y16 depth stream"))?;

        let geometry = CameraGeometry {
            color: color.profile.intrinsics()?,
            depth: depth.profile.intrinsics()?,
            ir: ir.profile.intrinsics()?,
            color_to_depth: color.profile.extrinsics_to(&depth.profile)?,
            ir_to_depth: ir.profile.extrinsics_to(&depth.profile)?,
        };

        let converter = match convert {
            None => None,
            Some(kind) => {
                let name = CString::new("FormatConverter").expect("literal has no NUL");
                let filter = Filter::new(
                    &sdk,
                    ob!(sdk, ob_create_filter(name.as_ptr()))?,
                    "format converter",
                )?;
                let key = CString::new("convertType").expect("literal has no NUL");
                ob!(
                    sdk,
                    ob_filter_set_config_value(filter.as_ptr(), key.as_ptr(), f64::from(kind))
                )?;
                Some(filter)
            }
        };

        let config = Config::new(&sdk, ob!(sdk, ob_create_config())?, "ob_create_config")?;
        for profile in [&color.profile, &ir.profile, &depth.profile] {
            ob!(
                sdk,
                ob_config_enable_stream_with_stream_profile(config.as_ptr(), profile.as_ptr())
            )?;
        }
        ob!(
            sdk,
            ob_config_set_align_mode(config.as_ptr(), ffi::OBAlignMode_ALIGN_DISABLE)
        )?;
        ob!(
            sdk,
            ob_config_set_frame_aggregate_output_mode(
                config.as_ptr(),
                ffi::OB_FRAME_AGGREGATE_OUTPUT_MODE_OB_FRAME_AGGREGATE_OUTPUT_ALL_TYPE_FRAME_REQUIRE
            )
        )?;
        ob!(sdk, ob_pipeline_enable_frame_sync(pipeline.as_ptr()))?;

        let projector = Projector::detect(&device);
        let imu_state = Arc::new(Mutex::new(ImuState::default()));

        // From here on `stream` owns everything, so an error below tears the
        // partially started camera down through `Drop`.
        let mut stream = Self {
            imu: Vec::new(),
            imu_state,
            converter,
            ir_frame: ir_source.frame,
            projector,
            pipeline,
            pipeline_started: false,
            device,
            geometry,
            _devices: devices,
            _context: context,
        };
        ob!(
            sdk,
            ob_pipeline_start_with_config(stream.pipeline.as_ptr(), config.as_ptr())
        )?;
        stream.pipeline_started = true;
        stream.start_imu(&color.profile);
        Ok(stream)
    }

    /// Starts both inertial sensors, or neither: the solver needs gyro and accel
    /// together, and a camera without them still calibrates without tilt help.
    fn start_imu(&mut self, color: &Profile) {
        let started = (|| -> Result<Vec<ImuSensor>, CameraError> {
            let sensors = self.device.sensor_list()?;
            let accel = ImuSensor::start(&sensors, ImuKind::Accel, color, &self.imu_state)?;
            let gyro = ImuSensor::start(&sensors, ImuKind::Gyro, color, &self.imu_state)?;
            Ok(accel
                .into_iter()
                .zip(gyro)
                .flat_map(|(a, g)| [a, g])
                .collect())
        })();
        if let Some(sensors) = best_effort("IMU start", started) {
            self.imu = sensors;
        }
    }

    /// Builds [`Frames`] from a frameset, or `None` while a stream is missing.
    fn assemble(&self, set: &Frame) -> Result<Option<Frames>, CameraError> {
        let (Some(color), Some(depth), Some(ir)) = (
            set.sub_frame(ffi::OBFrameType_OB_FRAME_COLOR),
            set.sub_frame(ffi::OBFrameType_OB_FRAME_DEPTH),
            set.sub_frame(self.ir_frame),
        ) else {
            return Ok(None);
        };
        let rgb = self.color_to_rgb(color)?;
        let (w, h, buf) = rgb.pixels(3)?;
        let color = RgbImage::from_raw(w, h, buf)
            .ok_or_else(|| CameraError::Sdk("color frame size mismatch".into()))?;
        let (depth, depth_unit_m) = depth_from_frame(&depth)?;
        Ok(Some(Frames {
            color,
            depth,
            depth_unit_m,
            ir: gray_from_frame(&ir)?,
        }))
    }

    fn color_to_rgb(&self, frame: Frame) -> Result<Frame, CameraError> {
        let frame = match &self.converter {
            Some(filter) if frame.format()? != ffi::OBFormat_OB_FORMAT_RGB => Frame::new(
                &filter.sdk,
                ob!(
                    filter.sdk,
                    ob_filter_process(filter.as_ptr(), frame.as_ptr())
                )?,
                "color format conversion",
            )?,
            _ => frame,
        };
        if frame.format()? != ffi::OBFormat_OB_FORMAT_RGB {
            return Err(CameraError::Unsupported("RGB color frames"));
        }
        Ok(frame)
    }
}

impl Drop for OrbbecStream {
    fn drop(&mut self) {
        // The projector setting persists in the camera, so hand it back as found.
        if let Some(projector) = self.projector {
            best_effort("restoring projector", projector.restore(&self.device));
        }
        self.imu.clear();
        if self.pipeline_started {
            best_effort(
                "ob_pipeline_stop",
                ob!(self.pipeline.sdk, ob_pipeline_stop(self.pipeline.as_ptr())),
            );
        }
    }
}

impl CameraStream for OrbbecStream {
    fn geometry(&self) -> &CameraGeometry {
        &self.geometry
    }

    fn next_frames(&mut self, timeout: Duration) -> Result<Frames, CameraError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait_ms = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
            let raw = ob!(
                self.pipeline.sdk,
                ob_pipeline_wait_for_frameset(self.pipeline.as_ptr(), wait_ms)
            )?;
            if let Some(set) = Frame::adopt(&self.pipeline.sdk, raw) {
                if let Some(frames) = self.assemble(&set)? {
                    return Ok(frames);
                }
            }
            if Instant::now() >= deadline {
                return Err(CameraError::Timeout);
            }
        }
    }

    fn drain_imu(&mut self) -> Vec<ImuSample> {
        std::mem::take(&mut self.imu_state.lock().samples).into()
    }

    fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), CameraError> {
        if !self
            .device
            .supports(ffi::OBPropertyID_OB_PROP_IR_EXPOSURE_INT)
        {
            return Err(CameraError::Unsupported("infrared exposure control"));
        }
        if self
            .device
            .supports(ffi::OBPropertyID_OB_PROP_IR_AUTO_EXPOSURE_BOOL)
        {
            self.device
                .set_bool(ffi::OBPropertyID_OB_PROP_IR_AUTO_EXPOSURE_BOOL, false)?;
        }
        let value = i32::try_from(exposure_us).unwrap_or(i32::MAX);
        self.device
            .set_int_clamped(ffi::OBPropertyID_OB_PROP_IR_EXPOSURE_INT, value)?;
        Ok(())
    }

    fn set_ir_auto_exposure(&mut self) -> Result<(), CameraError> {
        if !self
            .device
            .supports(ffi::OBPropertyID_OB_PROP_IR_AUTO_EXPOSURE_BOOL)
        {
            return Err(CameraError::Unsupported("infrared auto exposure"));
        }
        self.device.restore_ir_auto_exposure()
    }

    fn set_projector_enabled(&mut self, enabled: bool) -> Result<bool, CameraError> {
        match self.projector {
            Some(projector) => projector.set(&self.device, enabled).map(|()| true),
            None => Ok(false),
        }
    }
}
