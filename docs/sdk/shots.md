<!-- order: 5 -->
<!-- description: Find where a device was aimed at the moment it fired, using the shot delay and the tracking history. -->

# Scoring shots

An impact event arrives a moment after the trigger breaks: the device detects
the shot by its recoil, and by then it may already have moved. To score a shot,
use the aimpoint from the moment the trigger broke, not the latest one.

![A timeline. Tracking events arrive about every 10 ms. The trigger breaks; the impact is detected a little later, after the shot delay. Subtract the shot delay from the impact's timestamp and use the tracking event closest to that time.](./images/shot-timing.svg)

Each device has a **shot delay**: how long after the trigger breaks its recoil
is detected, in milliseconds. Users set it per device in Odyssey Desktop; your
app reads it with `get_shot_delay`.

<!-- steps -->
1. Keep the recent tracking events of each device. The SDK's `TrackingHistory`
   does this: create one per device and `push` every tracking event into it.
   It keeps the last N events.
2. When an impact arrives, subtract the device's shot delay, converted to
   microseconds, from the impact's `timestamp`.
3. Ask the history for the event closest to that time with `get_closest`, and
   score its `aimpoint`.
<!-- /steps -->

Impacts from accessories, such as Bluetooth magazines, have no device clock:
their `timestamp` is `0xFFFFFFFF`. For those, start from the latest tracking
event's timestamp instead of the impact's.

<!-- tabs: lang -->

**C#**

```csharp
const uint AccessoryTimestamp = 0xFFFFFFFF;

// The device timestamp at which the trigger broke.
static uint ShotTimestamp(Ohc.uniffi.TrackingHistory history, uint impactTs, ushort shotDelayMs)
{
    if (impactTs == AccessoryTimestamp && history.Latest() is { } latest)
        impactTs = latest.Timestamp;
    return unchecked(impactTs - (uint)shotDelayMs * 1000u);
}

// For each device: var history = new Ohc.uniffi.TrackingHistory(1000), then
// history.Push(tracking) for every tracking event, and
// var shotDelayMs = await client.GetShotDelay(device). On an impact:
var shot = history.GetClosest(ShotTimestamp(history, impact.V1.Timestamp, shotDelayMs));
if (shot != null)
    Console.WriteLine($"hit at ({shot.Aimpoint.X:F3}, {shot.Aimpoint.Y:F3})");
```

**Python**

```python
ACCESSORY_TIMESTAMP = 0xFFFFFFFF

def shot_timestamp(history: ohc.TrackingHistory, impact_ts: int, shot_delay_ms: int) -> int:
    """The device timestamp at which the trigger broke."""
    if impact_ts == ACCESSORY_TIMESTAMP:
        latest = history.latest()
        impact_ts = latest.timestamp if latest is not None else impact_ts
    return (impact_ts - shot_delay_ms * 1000) & 0xFFFFFFFF

# For each device: history = ohc.TrackingHistory(1000), then
# history.push(tracking_event) for every tracking event. On an impact:
ts = shot_timestamp(history, impact.timestamp, shot_delay_ms)
shot = history.get_closest(ts)
if shot is not None:
    print(f"hit at ({shot.aimpoint.x:.3f}, {shot.aimpoint.y:.3f})")
```

**C**

```c
#define ACCESSORY_TIMESTAMP 0xFFFFFFFFu

/* The device timestamp at which the trigger broke. */
static uint32_t shot_timestamp(struct ohc_TrackingHistory *history, uint32_t impact_ts, uint16_t shot_delay_ms) {
    struct ohc_TrackingEvent latest;
    if (impact_ts == ACCESSORY_TIMESTAMP && tracking_history_latest(history, &latest)) {
        impact_ts = latest.timestamp;
    }
    return impact_ts - (uint32_t)shot_delay_ms * 1000u; /* wraps like the device clock */
}

/* For each device: history = tracking_history_new(1000), then
   tracking_history_push(history, tracking) for every tracking event, and read
   its shot delay with client_get_shot_delay. On an impact: */
struct ohc_TrackingEvent shot;
if (tracking_history_get_closest(history, shot_timestamp(history, impact.timestamp, shot_delay_ms), &shot)) {
    printf("hit at (%.3f, %.3f)\n", shot.aimpoint.x, shot.aimpoint.y);
}
```

**C++**

```cpp
#include <ohc_extras.hpp>

constexpr uint32_t kAccessoryTimestamp = 0xFFFFFFFF;

// The device timestamp at which the trigger broke.
uint32_t shotTimestamp(ohc::extras::TrackingHistory &history, uint32_t impactTs, uint16_t shotDelayMs) {
    ohc::TrackingEvent latest;
    if (impactTs == kAccessoryTimestamp && history.latest(latest)) {
        impactTs = latest.timestamp;
    }
    return impactTs - uint32_t{shotDelayMs} * 1000u; // wraps like the device clock
}

// For each device: ohc::extras::TrackingHistory history(1000), then
// history.push(tracking) for every tracking event, and read its shot delay
// with ohc::client_get_shot_delay. On an impact:
ohc::TrackingEvent shot;
if (history.get_closest(shotTimestamp(history, impact.timestamp, shotDelayMs), shot)) {
    std::printf("hit at (%.3f, %.3f)\n", shot.aimpoint.x, shot.aimpoint.y);
}
```

**Rust**

```rust
use odyssey_hub_client::tracking_history::TrackingHistory;

const ACCESSORY_TIMESTAMP: u32 = u32::MAX;

/// The device timestamp at which the trigger broke.
fn shot_timestamp(history: &TrackingHistory, impact_ts: u32, shot_delay_ms: u16) -> u32 {
    let impact_ts = match history.latest() {
        Some(latest) if impact_ts == ACCESSORY_TIMESTAMP => latest.timestamp,
        _ => impact_ts,
    };
    impact_ts.wrapping_sub(u32::from(shot_delay_ms) * 1000)
}

// For each device: let mut history = TrackingHistory::new(1000), then
// history.push(tracking) for every tracking event, and
// let shot_delay_ms = client.device(device.clone()).get_shot_delay().await?. On an impact:
if let Some(shot) = history.get_closest(shot_timestamp(&history, impact.timestamp, shot_delay_ms)) {
    println!("hit at ({:.3}, {:.3})", shot.aimpoint.x, shot.aimpoint.y);
}
```

<!-- /tabs -->

The [Python sample](https://github.com/odysseyarm/odyssey/blob/master/sdk/odyssey-hub-client-uniffi/samples/py/odyhubclient/main.py)
shows this in a complete program.

> [!WARNING]
> Some tracking events currently have `timestamp` 0 (see
> [Timestamps](./tracking.md#timestamps)). Don't push those into the history,
> or a lookup near 0 can return one of them.

Shot delays change when users adjust them in Odyssey Desktop. To keep yours
current, subscribe to them with `subscribe_shot_delay`.
