using System.Diagnostics;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Threading.Channels;
using uniffi.odyssey_hub_common;
using Ohc = OdysseyArm.HubClient;

Ohc.Client client = new Ohc.Client();

Task connectTask = client.Connect();
try {
    await connectTask;
    Console.WriteLine("Connected!");
} catch (Ohc.uniffi.AnyhowException ex) {
    Console.WriteLine(ex.AnyhowMessage());
    Environment.Exit(1);
}

Task<Ohc.uniffi.Device[]> deviceListTask = client.GetDeviceList();
Ohc.uniffi.Device[] devices = {};
try {
    devices = await deviceListTask;
    Console.WriteLine("Devices (count: {0}):", devices.Length);
} catch (Ohc.uniffi.AnyhowException ex) {
    Console.WriteLine(ex.AnyhowMessage());
    Environment.Exit(1);
}

void PrintDevice(Ohc.uniffi.Device device) {
    Console.WriteLine("\tDevice:");
    switch (device.Transport) {
        case Transport.UdpMux:
            Console.WriteLine("\t\tType: UDP Mux");
            break;
        case Transport.Usb:
            Console.WriteLine("\t\tType: USB");
            break;
        case Transport.UsbMux:
            Console.WriteLine("\t\tType: USB Mux");
            break;
        default:
            break;

    }
    Console.WriteLine("\t\tUUID: 0x{0:X}", device.Uuid);
}

foreach (var device in devices) {
    PrintDevice(device);
    Task malfunction_zero_task = client.WriteVendor(device, 0x84, [ 0x00 ]);
    await malfunction_zero_task;
    Task device_type_task = client.WriteVendor(device, 0x90, []);
    await device_type_task;
}

Console.WriteLine("Devices printed!");

{
    Console.WriteLine("Attempting to print screen_0 info");
    var screen_info = await client.GetScreenInfoById(0);
    Console.WriteLine("Screen info:");
    Console.WriteLine("\tID: {0}", screen_info.Id);
    Console.WriteLine("\tBounds:");
    Console.WriteLine("\t\tTop Left: \t{0}", screen_info.Tl);
    Console.WriteLine("\t\tTop Right: \t{0}", screen_info.Tr);
    Console.WriteLine("\t\tBottom Left: \t{0}", screen_info.Bl);
    Console.WriteLine("\t\tBottom Right: \t{0}", screen_info.Br);
}

// Track connected devices to detect connect/disconnect
HashSet<byte[]> connectedDeviceUuids = new HashSet<byte[]>(new ByteArrayComparer());

Channel<(Ohc.uniffi.Device[]?, Ohc.uniffi.ClientException?)> deviceListChannel = Channel.CreateUnbounded<(Ohc.uniffi.Device[]?, Ohc.uniffi.ClientException?)>();
Channel<(Ohc.uniffi.Event?, Ohc.uniffi.ClientException?)> eventChannel = Channel.CreateUnbounded<(Ohc.uniffi.Event?, Ohc.uniffi.ClientException?)>();

await Task.WhenAny(
    client.SubscribeDeviceList(deviceListChannel.Writer),
    client.SubscribeEvents(eventChannel.Writer),
    Task.Run(async () => {
        Console.WriteLine("Starting device list loop");
        await foreach ((var deviceList, var err) in deviceListChannel.Reader.ReadAllAsync()) {
            if (err == null && deviceList != null) {
                var currentUuids = new HashSet<byte[]>(deviceList.Select(d => d.Uuid), new ByteArrayComparer());

                // Find newly connected devices
                foreach (var device in deviceList) {
                    if (!connectedDeviceUuids.Contains(device.Uuid)) {
                        Console.WriteLine("Device connected");
                        PrintDevice(device);
                        // Resetting on connect is currently redundant because the device zero isometry is the identity on connect
                        // await client.ResetZero(device);
                        // _ = Task.Run(async () => {
                        //     await Task.Delay(5000);
                        //     // negative Y is up, positive X is right
                        //     // the following translation says the bore is 0.0381 meters above the device in the device's local coordinate system
                        //     var translation = new Ohc.uniffi.Vector3f32(0, -0.0381f, 0);
                        //     // zero target is middle of the screen
                        //     var target = new Ohc.uniffi.Vector2f32(0.5f, 0.5f);
                        //     await client.Zero(device, translation, target);
                        //     Console.WriteLine("Zeroed device");
                        // });
                        Task malfunction_zero_task = client.WriteVendor(device, 0x84, [0x00]);
                        await malfunction_zero_task;
                        Task device_type_task = client.WriteVendor(device, 0x90, []);
                        await device_type_task;
                    }
                }

                // Find disconnected devices
                foreach (var uuid in connectedDeviceUuids) {
                    if (!currentUuids.Contains(uuid)) {
                        Console.WriteLine("Device disconnected");
                        Console.WriteLine("\tUUID: 0x{0}", BitConverter.ToString(uuid));
                    }
                }

                connectedDeviceUuids = currentUuids;
            } else if (err != null) {
                Console.WriteLine(err.Message);
            }
        }
    }),
    Task.Run(async () => {
        Console.WriteLine("Starting event loop");
        await foreach ((var @event, var err) in eventChannel.Reader.ReadAllAsync()) {
            Console.WriteLine("Received event:");
            if (err == null) {
                switch (@event) {
                    case Ohc.uniffi.Event.DeviceEvent deviceEvent:
                        Console.WriteLine($"Kind: {deviceEvent.V1.Kind.GetType().Name}");
                        switch (deviceEvent.V1.Kind) {
                            // uncomment desired behavior
                            case Ohc.uniffi.DeviceEventKind.AccelerometerEvent accelerometer:
                                // Console.WriteLine("Printing accelerometer event:");
                                // Console.WriteLine("\ttimestamp: {0}", accelerometer.V1.Timestamp);
                                // Console.WriteLine("\tacceleration: {0} {1} {2}", accelerometer.V1.Accel.X, accelerometer.V1.Accel.Y, accelerometer.V1.Accel.Z);
                                // Console.WriteLine("\tangular_velocity: {0} {1} {2}", accelerometer.V1.Gyro.X, accelerometer.V1.Gyro.Y, accelerometer.V1.Gyro.Z);
                                // Console.WriteLine("\teuler_angles: {0} {1} {2}", accelerometer.V1.EulerAngles.X, accelerometer.V1.EulerAngles.Y, accelerometer.V1.EulerAngles.Z);
                                break;
                            case Ohc.uniffi.DeviceEventKind.TrackingEvent tracking:
                                Console.WriteLine("Printing tracking event:");
                                Console.WriteLine("\ttimestamp: {0}", tracking.V1.Timestamp);
                                Console.WriteLine("\taimpoint: {0} {1}", tracking.V1.Aimpoint.X, tracking.V1.Aimpoint.Y);
                                Console.WriteLine("\tpose: ");
                                Console.WriteLine("\t\trotation: ");
                                Console.WriteLine("\t\t\t{0} {1} {2}", tracking.V1.Pose.Rotation.M11, tracking.V1.Pose.Rotation.M12, tracking.V1.Pose.Rotation.M13);
                                Console.WriteLine("\t\t\t{0} {1} {2}", tracking.V1.Pose.Rotation.M21, tracking.V1.Pose.Rotation.M22, tracking.V1.Pose.Rotation.M23);
                                Console.WriteLine("\t\t\t{0} {1} {2}", tracking.V1.Pose.Rotation.M31, tracking.V1.Pose.Rotation.M32, tracking.V1.Pose.Rotation.M33);
                                Console.WriteLine("\t\ttranslation: ");
                                Console.WriteLine("\t\t\t{0} {1} {2}", tracking.V1.Pose.Translation.X, tracking.V1.Pose.Translation.Y, tracking.V1.Pose.Translation.Z);
                                Console.WriteLine("\t\tscreen_id: {0}", tracking.V1.ScreenId);
                                Console.WriteLine("\tdistance: {0}", tracking.V1.Distance);
                                break;
                            case Ohc.uniffi.DeviceEventKind.ImpactEvent impact:
                                Console.WriteLine("Printing impact event:");
                                Console.WriteLine("\ttimestamp: {0}", impact.V1.Timestamp);
                                break;
                            case Ohc.uniffi.DeviceEventKind.ZeroResult zeroResult:
                                if (zeroResult.V1) {
                                    Console.WriteLine("Zero result: success");
                                } else {
                                    Console.WriteLine("Zero result: failure");
                                }
                                break;
                            case Ohc.uniffi.DeviceEventKind.PacketEvent packet:
                                switch (packet.V1.Data) {
                                    case Ohc.uniffi.PacketData.Unsupported _:
                                        Console.WriteLine("Unsupported packet");
                                        break;
                                    case Ohc.uniffi.PacketData.VendorEvent vendorPacketData:
                                        Console.WriteLine("Printing vendor packet:");
                                        Console.WriteLine("\ttype: {0}", packet.V1.Ty);
                                        Console.WriteLine("\tdata: {0}", BitConverter.ToString(vendorPacketData.V1.Data));
                                        break;
                                }
                                break;
                            case Ohc.uniffi.DeviceEventKind.CapabilitiesChanged _:
                                Console.WriteLine("Device capabilities changed");
                                break;
                            default:
                                break;
                        }
                        break;
                    default:
                        break;
                }
            } else {
                Console.WriteLine(err.Message);
            }
        }
    })
);

// Helper class to compare byte arrays in HashSet
class ByteArrayComparer : IEqualityComparer<byte[]>
{
    public bool Equals(byte[]? x, byte[]? y)
    {
        if (x == null || y == null) return x == y;
        return x.SequenceEqual(y);
    }

    public int GetHashCode(byte[] obj)
    {
        if (obj == null) return 0;
        int hash = 17;
        foreach (byte b in obj)
            hash = hash * 31 + b;
        return hash;
    }
}
