<!-- order: 2 -->
<!-- description: Connect to Odyssey Desktop and print where every device is aimed. -->

# Quickstart

This program connects to Odyssey Desktop and prints the aimpoint of every
device it tracks. Start Odyssey Desktop first, with a device connected and the
screen calibrated.

<!-- tabs: lang -->

**C#**

```csharp
using System.Threading.Channels;
using Ohc = OdysseyArm.HubClient;

using var client = new Ohc.Client();
await client.Connect();

var events = Channel.CreateUnbounded<(Ohc.uniffi.Event?, Ohc.uniffi.ClientException?)>();
_ = client.SubscribeEvents(events.Writer);

await foreach (var (ev, error) in events.Reader.ReadAllAsync())
{
    if (error != null)
    {
        Console.WriteLine($"Stream ended: {error.Message}");
        break;
    }
    if (ev is Ohc.uniffi.Event.DeviceEvent deviceEvent &&
        deviceEvent.V1.Kind is Ohc.uniffi.DeviceEventKind.TrackingEvent tracking)
    {
        var aim = tracking.V1.Aimpoint;
        Console.WriteLine($"{deviceEvent.V1.Device.Uuid:X}: aimpoint ({aim.X:F3}, {aim.Y:F3})");
    }
}
```

**Python**

```python
import asyncio
import ohc_uniffi as ohc

async def main():
    client = ohc.Client()
    await client.connect()
    stream = await client.subscribe_events()
    while True:
        event = await stream.next()
        if not event.is_device_event():
            continue
        device_event = event[0]
        if device_event.kind.is_tracking_event():
            tracking = device_event.kind[0]
            uuid = ohc.Device(device_event.device).uuid()
            print(f"{uuid:x}: aimpoint ({tracking.aimpoint.x:.3f}, {tracking.aimpoint.y:.3f})")

asyncio.run(main())
```

**C++**

```cpp
#include <ohc.hpp>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <memory>
#include <thread>

static std::atomic_bool done{false};

struct State {
    ohc::Handle *handle;
    ohc::Client *client;
};

static void onEvent(ohc::UserObj, ohc::ClientError error, ohc::Event event) {
    if (error != ohc::ClientError::NONE) {
        done = true;
        return;
    }
    if (event.tag != ohc::Event::Tag::DEVICE_EVENT) {
        return;
    }
    const ohc::DeviceEvent &device_event = event.DEVICE_EVENT._0;
    if (device_event.kind.tag == ohc::DeviceEventKind::Tag::TRACKING_EVENT) {
        const ohc::TrackingEvent &tracking = device_event.kind.TRACKING_EVENT._0;
        std::printf("aimpoint (%.3f, %.3f)\n", tracking.aimpoint.x, tracking.aimpoint.y);
    }
}

static void onConnect(ohc::UserObj userdata, ohc::ClientError error) {
    auto *state = static_cast<const State *>(userdata.ptr);
    if (error != ohc::ClientError::NONE) {
        std::puts("Could not connect to Odyssey Desktop");
        done = true;
        return;
    }
    ohc::client_start_stream(state->handle, state->client, ohc::UserObj{nullptr}, onEvent);
}

int main() {
    std::unique_ptr<ohc::Handle, decltype(&ohc::rt_free)> handle(ohc::rt_init(), ohc::rt_free);
    std::unique_ptr<ohc::Client, decltype(&ohc::client_free)> client(ohc::client_new(), ohc::client_free);
    State state{handle.get(), client.get()};
    ohc::client_connect(handle.get(), client.get(), ohc::UserObj{&state}, onConnect);
    while (!done) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
}
```

The SDK calls your callbacks on its own threads; hand data to your main thread
yourself.

**Rust**

```rust
use futures::StreamExt as _;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect().await?;
    let mut events = client.subscribe_events().await?;
    while let Some(event) = events.next().await {
        if let Event::DeviceEvent(DeviceEvent(device, DeviceEventKind::TrackingEvent(tracking))) = event? {
            let aim = tracking.aimpoint;
            println!("{:02x?}: aimpoint ({:.3}, {:.3})", device.uuid, aim.x, aim.y);
        }
    }
    Ok(())
}
```

This also needs `tokio` (with the `macros` and `rt-multi-thread` features) and
`futures`.

<!-- /tabs -->

Aim a device around the screen and watch the numbers change: `(0, 0)` is the
top-left corner of the screen and `(1, 1)` the bottom-right.

> [!TIP]
> No output? Check that Odyssey Desktop shows the device as tracking. Tracking
> events only arrive while a device can see a calibrated screen.

These programs follow the samples in the repository, which show more of the
API: [Python](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-uniffi/samples/py/odyhubclient),
[C#](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-uniffi/dotnet/OdysseyHubClient/OdysseyHubClient.Test),
[C](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-ffi/samples/c/odyssey-hub-client),
[C++](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-ffi/samples/cpp/odyssey-hub-client)
and [Rust](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-hub-client/src/bin/client.rs).

## Next

- [Coordinate system](./coordinate-systems.md): what the aimpoint and pose mean.
- [Tracking events](./tracking.md): every other event and field.
