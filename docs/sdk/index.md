<!-- order: 0 -->
<!-- sidebar-label: Overview -->
<!-- description: Read where Odyssey devices aim, where they are, and when they fire, from your own game, simulator, or training app. -->

# Build with Odyssey

The Odyssey client SDK lets your game, simulator, or training app follow every
Odyssey device live: where it is aimed on the screen, where it is in front of
the screen, and when it fires.

![Odyssey devices connect to Odyssey Desktop over USB, Bluetooth, or the dongle. Odyssey Desktop runs the hub, which does the tracking. Your app connects to the hub on the same computer with the client SDK.](./images/data-flow.svg)

Your app never talks to a device directly. **Odyssey Desktop** runs the
**hub**: it talks to the devices, holds the screen calibration, does the
tracking and applies each device's zero. Your app connects to the hub on the
same computer and subscribes to its events.

> [!IMPORTANT]
> Before your app receives any tracking data, Odyssey Desktop must be running,
> the device connected, and the screen calibrated in Odyssey Desktop.

## What you get

- **Aimpoint:** where each device is aimed, as a fraction of the screen.
- **Pose:** where each device is in front of the screen and which way it
  points, in meters.
- **Shots:** an event each time a device fires, with the device's timestamp.
- **Zeroing:** line up each device's aimpoint with where its shots land.

## Languages

The SDK is written in Rust, with bindings for C#, Python, C, and C++. Every
language has the same events and fields; only the naming style changes, such
as `screen_id` in Rust, Python, and C versus `ScreenId` in C#.

| Language | Typical use |
|---|---|
| C# | Unity, Godot (.NET), other .NET apps |
| C++ | Unreal Engine, native apps |
| Python | Tools, prototypes, recording data |
| Rust | Bevy, native apps |

## Start here

<!-- cards -->
- [Installation](./install.md): add the SDK to your project.
- [Quickstart](./quickstart.md): connect and print where each device is aimed.
- [Coordinate system](./coordinate-systems.md): what aimpoints and poses mean, and how they map to Unity, Unreal, Godot, and Bevy.
- [Tracking events](./tracking.md): every event the hub sends, field by field.
- [Scoring shots](./shots.md): find where a device was aimed at the moment it fired.
- [Zeroing](./zeroing.md): make each device's aimpoint land where its shots do.
<!-- /cards -->
