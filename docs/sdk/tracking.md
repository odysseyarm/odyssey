<!-- order: 4 -->
<!-- description: Every event Odyssey Desktop sends to your app, field by field. -->

# Tracking events

After connecting, your app subscribes to one stream of events from the hub.
Each event says which device it is about and what happened.

## Tracking event

Sent continuously, many times a second, for each device that can see a
calibrated screen.

| Field | Type | Meaning |
|---|---|---|
| `aimpoint` | 2D vector | Where the device is aimed, as a fraction of the screen: `(0, 0)` is the top-left corner, `(1, 1)` the bottom-right. See [the aimpoint](./coordinate-systems.md#2d-the-aimpoint). |
| `distance` | meters | How far the device is from the screen, along its line of sight. |
| `pose` | 3×3 rotation, 3D translation | Where the device is in front of the screen and which way it points. See [the 3D frame](./coordinate-systems.md#3d-right-handed-y-up). |
| `screen_id` | integer, 0 to 5 | Which calibrated screen `aimpoint` and `pose` refer to. |
| `timestamp` | microseconds | The device's clock for this update. See [Timestamps](#timestamps). |

> [!NOTE]
> The aimpoint can be below 0 or above 1: that means the device is aimed past
> an edge of the screen. Clamp or ignore those values as your app needs.

A few more details:

- **`aimpoint` `(0, 0)` with `distance` 0** means the device was tracked but no
  aim could be worked out for that update. Treat it as "no aim".
- **Zeroing moves the aimpoint, not the pose.** A device's
  [zero](./zeroing.md) changes `aimpoint` and `distance`; `pose` always
  describes the device itself.
- **With more than one screen**, aiming well past the left or right edge of the
  current screen makes the hub look for the device on the other screens.
  Tracking events pause briefly and `screen_id` may change.

## Impact event

Sent when a device detects a shot by its recoil, or when an accessory such as a
Bluetooth magazine reports one.

| Field | Type | Meaning |
|---|---|---|
| `timestamp` | microseconds | The device's clock when the shot was detected. `0xFFFFFFFF` means the shot came from an accessory, which has no device clock. |

The impact arrives a moment after the trigger breaks, and the device may have
moved since. [Scoring shots](./shots.md) shows how to find where it was aimed.

## Other events

| Event | Meaning |
|---|---|
| `ZeroResult(bool)` | Answer to a [zero](./zeroing.md) or reset-zero request: whether it worked. |
| `SaveZeroResult(bool)` | Answer to saving or clearing a zero. |
| `AccelerometerEvent` | Raw motion: `accel` in m/s² and `gyro`, in the device's sensor axes, and the device `timestamp`. `euler_angles` is not filled in. |
| `BatteryEvent` | `percent` charged, and whether it is `charging`. |
| `CapabilitiesChanged` | The device's connection changed, for example it lost or regained its tracking channel. Re-read anything you cached about it. |

## Timestamps

Device timestamps are microseconds on the device's own clock: not wall-clock
time, and not comparable between devices. They are 32-bit, so they wrap around
to 0 about every 71 minutes; compare them modulo 2³².

> [!WARNING]
> Some tracking events currently have `timestamp` 0. Skip them when you look
> events up by time.

## Screens

`get_screen_info_by_id(id)` returns the four corners of a calibrated screen in
meters: `tl`, `tr`, `bl`, and `br` (in C, `bounds[0]` to `bounds[3]` in that
order). They come from the screen calibration, whose y axis points **down**;
see [Screen corners](./coordinate-systems.md#screen-corners) before using them
with a pose.
