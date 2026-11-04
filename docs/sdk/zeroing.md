<!-- order: 6 -->
<!-- description: Line up each device's aimpoint with where its shots land. -->

# Zeroing

The Odyssey tracker sits below or beside the bore, and it is never exactly
parallel to it. **Zeroing** measures that difference once, so that a device's
[aimpoint](./coordinate-systems.md#2d-the-aimpoint) lands where its shots do.

![Side view. The Odyssey tracker sits below the bore and its line of sight is not parallel to it, so before zeroing the aimpoint misses the target. Zeroing records the offset from the tracker to the sight line and the angle that makes that line pass through the target.](./images/zeroing.svg)

When a device is zeroed, the hub stores two things for it:

- **an offset:** where the sight line starts, relative to the tracker;
- **an angle:** the rotation that makes the sight line pass through the target.

From then on, that device's `aimpoint` and `distance` are measured along the
sight line. Its `pose` does not change: it always describes the tracker.

A zero belongs to the device, not to an app. Once saved, it applies to every
app and survives restarts.

## Zeroing in Odyssey Desktop

Most users zero on the **Zero** page of Odyssey Desktop:

<!-- steps -->
1. Aim carefully at the target in the center of the screen.
2. Press **Z**, or fire with shot detection on.
3. Press **Save zero**.
<!-- /steps -->

If your app leaves zeroing to Odyssey Desktop, it needs no code for it.

## Zeroing from your app

| Call | Effect |
|---|---|
| `zero(device, translation, target)` | Zero `device` against `target`, using its current tracking. The hub answers with a `ZeroResult` event. |
| `reset_zero(device)` | Remove the device's zero. The hub answers with `ZeroResult`. |

- **`target`** is the point being aimed at, in aimpoint coordinates:
  `(0.5, 0.5)` is the center of the screen.
- **`translation`** is where the sight line starts relative to the tracker, in
  meters, in the **tracker's own sensor axes**. These are not the pose's axes:

  | Axis | Direction on the device |
  |---|---|
  | +X | right |
  | +Y | **down** |
  | +Z | **forward**, along the bore |

  A bore 6.35 cm above the tracker is therefore `(0, -0.0635, 0)`, which is
  what Odyssey Desktop always sends. Pass `(0, 0, 0)` if the offset doesn't
  matter for your setup.

The device must be tracking a screen when you call `zero`; otherwise the
`ZeroResult` is `false`.

<!-- tabs: lang -->

**C#**

```csharp
// Zero against the center of the screen, with Odyssey Desktop's offset.
await client.Zero(
    device,
    new Ohc.uniffi.Vector3f32(0f, -0.0635f, 0f),
    new Ohc.uniffi.Vector2f32(0.5f, 0.5f));
```

**Python**

```python
# Zero against the center of the screen, with Odyssey Desktop's offset.
await client.zero(device, ohc.Vector3f32(x=0.0, y=-0.0635, z=0.0), ohc.Vector2f32(x=0.5, y=0.5))
```

<!-- /tabs -->

> [!NOTE]
> A zero set from your app lasts until Odyssey Desktop restarts. To keep it, the
> user saves it on Odyssey Desktop's Zero page: the C#, Python, and C bindings
> cannot save a zero themselves.
