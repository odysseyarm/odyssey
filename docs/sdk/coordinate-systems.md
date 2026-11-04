# Coordinate Systems

This page documents every coordinate frame used across the tracking stack, how they relate to each other, and the transforms between them.

---

## Quick-reference table

| Frame | X+ | Y+ | Z+ | Origin | Units | Source |
|---|---|---|---|---|---|---|
| **ESKF World** | right | **down** | toward viewer | screen center | meters | `odyssey-cv: crates/odyssey-cv/src/foveated.rs` (private) |
| **Physical Screen Space** | right | **down** | — (screen is Z=0 plane) | screen center | meters | `crates/odyssey-common/src/screen_calibration.rs` |
| **Screen Unit Square** | right | **down** | — | top-left corner | [0,1] | `crates/odyssey-common/src/screen_calibration.rs` |
| **Client Pose** (uniffi) | right | **up** | into screen | screen center | meters | `crates/odyssey-hub-server/src/device_tasks/tracking.rs` |
| **Unity World** | right | **up** | into scene | scene origin | meters | — |

---

## ESKF World Frame

<img width="700" height="460" alt="ESKF World Frame diagram" src="https://github.com/user-attachments/assets/cbb08762-d989-48b4-a901-def33ca8ccb8" />

The Error-State Kalman Filter in `odyssey-cv` (private) tracks the device in a world frame anchored to the physical screen.

| Axis | Direction | Note |
|---|---|---|
| X+ | right | |
| Y+ | **physically down** | gravity vector = `(0, +9.81, 0)` |
| Z+ | toward viewer | out of the screen plane toward the shooter |

The screen lies on the **Z = 0 plane**.  Corner positions in this frame:

```
top-left     (−W/2, −H/2, 0)      ← Y is negative at the top (Y-down)
top-right    (+W/2, −H/2, 0)
bottom-left  (−W/2, +H/2, 0)
bottom-right (+W/2, +H/2, 0)
```

**Source:** `odyssey-cv: crates/odyssey-cv/src/foveated.rs` (private)
```rust
filter.gravity = Vector3::new(0., 9.81, 0.);   // +Y = physically down
```
**ESKF default** (for reference) uses Z-down: `gravity: Vector3::new(0f32, 0f32, 9.81)` (in the `eskf` crate).

The `transvec` passed to `transform_pose_for_client` is `fv_state.filter.position.coords` — the device's position in this world frame (`odyssey-cv: crates/odyssey-cv/src/helpers.rs` (private) (private)).

---

## Physical Screen Space

The physical screen space is the coordinate frame used by `ScreenCalibration`:

> *"`object_points` is the positions of the markers in **meters** in physical screen space, where the physical screen lies on the XY plane."*
> — `crates/odyssey-common/src/screen_calibration.rs`

`ScreenCalibration.rotation` is the quaternion that **rotates world-space coordinates into physical screen space**.

The **homography** maps from physical screen space (meters) → screen unit square [0, 1]²:

```
Physical TL (−W/2, −H/2)  →  unit (0, 0)   ← top-left in image convention
Physical TR (+W/2, −H/2)  →  unit (1, 0)
Physical BL (−W/2, +H/2)  →  unit (0, 1)   ← bottom-left
Physical BR (+W/2, +H/2)  →  unit (1, 1)
```

`bounds()` (`crates/odyssey-common/src/screen_calibration.rs`) applies the **inverse** homography to retrieve the four corners in physical screen space:

```rust
/// tl, tr, bl, br
pub fn bounds(&self) -> [Point2<F>; 4] {
    [(0,0), (1,0), (0,1), (1,1)]
        .map(|p| inv_homography.transform_point(&p))
}
```

The server packages these as `ScreenInfo.{tl, tr, bl, br}` — values are **still in physical screen space (Y-down, meters)**.

---

## Screen Unit Square

<img width="760" height="400" alt="Screen coordinate spaces" src="https://github.com/user-attachments/assets/3b85fbd7-69f1-4e05-92a5-015aec4295b9" />

The normalized 2D screen space used for aimpoints and the homography output.

- `(0, 0)` = **top-left** corner of screen
- `(1, 1)` = **bottom-right** corner of screen
- Y increases **downward** (image convention)

The **aimpoint** in `TrackingEvent` is in this space.  Converting to Unity screen pixels:

```csharp
// InputHandlers.cs — PerformShoot
Vector2 screenPoint = new Vector2(
    screenPointNormal.x * Screen.width,
    Screen.height - screenPointNormal.y * Screen.height   // flip Y for Unity screen
);
```

---

## Client Pose Frame (uniffi `TrackingEvent.pose`)

<img width="700" height="460" alt="Client Pose Frame diagram" src="https://github.com/user-attachments/assets/824cc032-fc66-4040-b04a-d54d5877dae7" />

This is the frame delivered to all uniffi clients via `TrackingEvent.pose`.

| Axis | Direction | Derived from |
|---|---|---|
| X+ | right | unchanged from world frame |
| Y+ | **physically up** | world Y negated |
| Z+ | into screen (away from viewer) | world Z negated |

Produced by `transform_pose_for_client` in `crates/odyssey-hub-server/src/device_tasks/tracking.rs`:

```rust
fn transform_pose_for_client(rotmat: Matrix3<f32>, transvec: Vector3<f32>) -> Pose {
    let flip = Matrix3::new(1.0, 0.0, 0.0,
                            0.0,-1.0, 0.0,
                            0.0, 0.0,-1.0);   // negates Y and Z
    Pose {
        rotation:    flip * rotmat * flip,
        translation: flip * transvec,
    }
}
```

**Key property:** `pose.translation.y` **increases** when the physical device moves **up**.  This matches Unity's Y-up convention.

To undo the flip (e.g. when computing zero offsets server-side), apply the same flip again — it is its own inverse:

```rust
// crates/odyssey-hub-server/src/device_tasks/tracking.rs
let raw_translation = flip * te.pose.translation;   // undo flip
```

---

## Transform Pipeline

<img width="900" height="320" alt="Coordinate transform pipeline" src="https://github.com/user-attachments/assets/92996eae-6ae3-4b8b-8b64-be634f018adb" />

```
odyssey-cv ESKF (private)      transform_pose_for_client           uniffi client             Unity
────────────────────           ─────────────────────────           ────────────────          ─────────────────────
X-right, Y-down, Z→viewer  →  flip = diag(1, −1, −1)         →   X-right, Y-up        →   use translation directly
gravity = (0, +9.81, 0)        t′ = flip · t                       Z+ into screen            do NOT negate Y
filter.position (world)         R′ = flip · R · flip                                          flipY for rotation only
```

---

## Unity Integration Notes

When integrating client pose data in Unity (`PoseUtils.ConvertOdyPoseToUnity`):

**The client pose translation is already Y-up.  Do not negate Y.**

```csharp
// CORRECT — use raw translation; Y-up matches Unity
position = new Vector3(pose.translation.x, pose.translation.y, pose.translation.z);
```

```csharp
// WRONG — applying flipY to the full matrix negates Y again, inverting vertical movement
Matrix4x4 transformed = flipY * raw * flipY;
position = t;  // t.y is now negated: camera goes DOWN when device goes UP
```

The `flipY` sandwich is still used for the **rotation matrix** to convert from right-handed (odyssey-cv world) to Unity's left-handed coordinate system.  It must **not** be applied to the translation.

### `HandleScreenZeroInfo` and `zero_translation`

`ScreenInfo.{tl, tr, bl, br}` arrive in **physical screen space (Y-down)**.  The helper function `f()` converts to Unity Y-up:

```csharp
float unityY = -(vec.y - centerY);   // flip Y: physical Y-down → Unity Y-up
```

This is correct because `screenInfo.tl.y < screenInfo.bl.y` (top has smaller/negative Y in Y-down physical space).

`zero_translation` is the Unity-space position of the Odyssey coordinate origin, with `z = distance_offset` (1 m) as the initial camera depth.

---

## Source reference index

| Symbol | Source location |
|---|---|
| `filter.gravity = (0, 9.81, 0)` | `odyssey-cv: crates/odyssey-cv/src/foveated.rs` (private) |
| `filter.position` (ESKF world position) | `odyssey-cv: crates/odyssey-cv/src/helpers.rs` (private) |
| `calculate_screen_3dpoints` (screen corners in world frame) | `odyssey-cv: crates/odyssey-cv/src/lib.rs` (private) |
| `ScreenCalibration` definition and docs | `crates/odyssey-common/src/screen_calibration.rs` |
| `bounds()` — physical screen corners | `crates/odyssey-common/src/screen_calibration.rs` |
| `transform_pose_for_client` | `crates/odyssey-hub-server/src/device_tasks/tracking.rs` |
| `calculate_zero_offset_from_tracking_event` (undo flip) | `crates/odyssey-hub-server/src/device_tasks/tracking.rs` |
| Unity `PoseUtils.ConvertOdyPoseToUnity` | `Assets/Scripts/Utils/PoseUtils.cs` |
| Unity `HandleScreenZeroInfo` | `Assets/Scripts/InputHandlers.cs` |
