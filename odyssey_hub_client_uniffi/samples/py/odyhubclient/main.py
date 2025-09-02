import asyncio
import ohc_uniffi as ohc # pyright: ignore[reportMissingTypeStubs]
import typing

async def main():
    while True:
        try:
            client = ohc.Client()
            await client.connect()
            print("Connected to Odyssey Hub")
            dl = await client.get_device_list()
            for dr in dl:
                d = ohc.Device(dr)
                shot_delay_ms = await client.get_shot_delay(dr)
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

        hist = ohc.TrackingHistory(1000)

        try:
            while True:
                evt = await stream.next()
                if evt.is_device_event():
                    d_evt = typing.cast(ohc.DeviceEvent, typing.cast(ohc.Event.DEVICE_EVENT, evt)[0])
                    d = ohc.Device(d_evt.device)
                    if d_evt.kind.is_tracking_event():
                        t_evt = typing.cast(ohc.TrackingEvent, typing.cast(ohc.DeviceEventKind.TRACKING_EVENT, d_evt.kind)[0])
                        hist.push(t_evt)
                        print(f"Tracking Event: device_id={d.uuid():x}, x={t_evt.aimpoint.x}, y={t_evt.aimpoint.y}")
                    elif d_evt.kind.is_impact_event():
                        i_evt = typing.cast(ohc.ImpactEvent, typing.cast(ohc.DeviceEventKind.IMPACT_EVENT, d_evt.kind)[0])
                        closest_t_evt = hist.get_closest(i_evt.timestamp)
                        if closest_t_evt is not None:
                            print(f"Impact Event: device_id={d.uuid():x}, x={closest_t_evt.aimpoint.x}, y={closest_t_evt.aimpoint.y}")
                        else:
                            print(f"Impact Event: device_id={d.uuid():x}")
                    elif d_evt.kind.is_connect_event():
                        shot_delay_ms = await client.get_shot_delay(d_evt.device)
                        print(f"Connect Event: device_id={d.uuid():x} shot_delay_ms={shot_delay_ms}")
                    elif d_evt.kind.is_disconnect_event():
                        print(f"Disconnect Event: device_id={d.uuid():x}")
        except ohc.AnyhowError as e:
            print(f"Event stream ended: {e.anyhow_message()}")

if __name__ == "__main__":
    asyncio.run(main())
