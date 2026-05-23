import asyncio
import ohc_uniffi as ohc # pyright: ignore[reportMissingTypeStubs]
import typing

ACCESSORY_TIMESTAMP = 0xFFFFFFFF

def lookup_timestamp(hist: ohc.TrackingHistory, impact_ts: int, shot_delay_ms: int) -> int:
    """Return the device timestamp to look up in the tracking history.

    For real device impacts, subtract shot delay (in µs) from the impact
    timestamp. For accessory impacts (sentinel u32::MAX), anchor to the
    latest tracked event and subtract shot delay from there, giving ~0–10ms
    staleness at 100 Hz.
    """
    shot_delay_us = shot_delay_ms * 1000
    if impact_ts == ACCESSORY_TIMESTAMP:
        latest = hist.latest()
        base_ts = latest.timestamp if latest is not None else ACCESSORY_TIMESTAMP
    else:
        base_ts = impact_ts
    return (base_ts - shot_delay_us) & 0xFFFFFFFF

async def main():
    while True:
        try:
            client = ohc.Client()
            await client.connect()
            print("Connected to Odyssey Hub")
            dl = await client.get_device_list()
            shot_delays: dict[int, int] = {}
            for dr in dl:
                d = ohc.Device(dr)
                shot_delay_ms = await client.get_shot_delay(dr)
                shot_delays[d.uuid()] = shot_delay_ms
                print(f"Found device: id={d.uuid():x} shot_delay_ms={shot_delay_ms}")
        except ohc.AnyhowError as e:
            print(f"Failed to connect: {e.anyhow_message()}")
            await asyncio.sleep(1)
            continue

        try:
            stream = await client.subscribe_events()
        except ohc.AnyhowError as e:
            print(f"Failed to subscribe to events: {e.anyhow_message()}")
            await asyncio.sleep(1)
            continue

        histories: dict[int, ohc.TrackingHistory] = {}

        try:
            while True:
                evt = await stream.next()
                if evt.is_device_event():
                    d_evt = typing.cast(ohc.DeviceEvent, typing.cast(ohc.Event.DEVICE_EVENT, evt)[0])
                    d = ohc.Device(d_evt.device)
                    uuid = d.uuid()
                    if d_evt.kind.is_tracking_event():
                        t_evt = typing.cast(ohc.TrackingEvent, typing.cast(ohc.DeviceEventKind.TRACKING_EVENT, d_evt.kind)[0])
                        if uuid not in histories:
                            histories[uuid] = ohc.TrackingHistory(1000)
                        histories[uuid].push(t_evt)
                        print(f"Tracking Event: device_id={uuid:x}, x={t_evt.aimpoint.x}, y={t_evt.aimpoint.y}")
                    elif d_evt.kind.is_impact_event():
                        i_evt = typing.cast(ohc.ImpactEvent, typing.cast(ohc.DeviceEventKind.IMPACT_EVENT, d_evt.kind)[0])
                        hist = histories.get(uuid)
                        if hist is not None:
                            delay_ms = shot_delays.get(uuid, 0)
                            ts = lookup_timestamp(hist, i_evt.timestamp, delay_ms)
                            closest_t_evt = hist.get_closest(ts)
                            if closest_t_evt is not None:
                                src = "accessory" if i_evt.timestamp == ACCESSORY_TIMESTAMP else "device"
                                print(f"Impact Event ({src}): device_id={uuid:x}, x={closest_t_evt.aimpoint.x}, y={closest_t_evt.aimpoint.y}")
                            else:
                                print(f"Impact Event: device_id={uuid:x} (no tracking data)")
                        else:
                            print(f"Impact Event: device_id={uuid:x} (no history)")
                    elif d_evt.kind.is_connect_event():
                        shot_delay_ms = await client.get_shot_delay(d_evt.device)
                        shot_delays[uuid] = shot_delay_ms
                        print(f"Connect Event: device_id={uuid:x} shot_delay_ms={shot_delay_ms}")
                    elif d_evt.kind.is_disconnect_event():
                        shot_delays.pop(uuid, None)
                        histories.pop(uuid, None)
                        print(f"Disconnect Event: device_id={uuid:x}")
        except ohc.AnyhowError as e:
            print(f"Event stream ended: {e.anyhow_message()}")

if __name__ == "__main__":
    asyncio.run(main())
