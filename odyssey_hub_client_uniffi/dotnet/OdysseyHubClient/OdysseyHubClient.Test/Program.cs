using System.Diagnostics;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Threading.Channels;

using Ohc = Radiosity.OdysseyHubClient;

Ohc.Client client = new Ohc.Client();

Task connectTask = client.Connect();
try {
    await connectTask;
    Console.WriteLine("Connected!");
} catch (Ohc.uniffi.AnyhowException ex) {
    Console.WriteLine(ex.AnyhowMessage());
    Environment.Exit(1);
}

Task<List<Ohc.uniffi.DeviceRecord>> deviceListTask = client.GetDeviceList();
List<Ohc.uniffi.DeviceRecord> devices = new List<Ohc.uniffi.DeviceRecord>();
try {
    devices = await deviceListTask;
    Console.WriteLine("Devices (count: {0}):", devices.Count);
} catch (Ohc.uniffi.AnyhowException ex) {
    Console.WriteLine(ex.AnyhowMessage());
    Environment.Exit(1);
}

void PrintDevice(Ohc.uniffi.DeviceRecord deviceR) {
    Console.WriteLine("\tDevice:");
    switch (deviceR) {
        case Ohc.uniffi.DeviceRecord.Udp udpDevice:
            Console.WriteLine("\t\tType: UDP");
            Console.WriteLine("\t\tID: {0}", udpDevice.id);
            Console.WriteLine("\t\tAddr: {0}", udpDevice.addr);
            break;
        case Ohc.uniffi.DeviceRecord.Cdc cdcDevice:
            Console.WriteLine("\t\tType: CDC");
            Console.WriteLine("\t\tPath: {0}", cdcDevice.path);
            break;
        case Ohc.uniffi.DeviceRecord.Hid hidDevice:
            Console.WriteLine("\t\tType: HID");
            Console.WriteLine("\t\tPath: {0}", hidDevice.path);
            break;
        default:
            break;

    }
    var device = new Ohc.uniffi.Device(deviceR);
    Console.WriteLine("\t\tUUID: 0x{0:X}", device.Uuid());
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
    Console.WriteLine("\tID: {0}", screen_info.id);
    Console.WriteLine("\tBounds:");
    Console.WriteLine("\t\tTop Left: \t{0}", screen_info.tl);
    Console.WriteLine("\t\tTop Right: \t{0}", screen_info.tr);
    Console.WriteLine("\t\tBottom Left: \t{0}", screen_info.bl);
    Console.WriteLine("\t\tBottom Right: \t{0}", screen_info.br);
}

Channel<(Ohc.uniffi.Event?, Ohc.uniffi.ClientException?)> eventChannel = Channel.CreateUnbounded<(Ohc.uniffi.Event?, Ohc.uniffi.ClientException?)>();
await Task.WhenAny(
    client.RunStream(eventChannel.Writer),
    Task.Run(async () => {
        Console.WriteLine("Starting event loop");
        await foreach ((var @event, var err) in eventChannel.Reader.ReadAllAsync()) {
            Console.WriteLine("Received event:");
            if (err == null) {
                switch (@event) {
                    case Ohc.uniffi.Event.DeviceEvent deviceEvent:
                        switch (deviceEvent.v1.kind) {
                            // uncomment desired behavior
                            case Ohc.uniffi.DeviceEventKind.AccelerometerEvent accelerometer:
                                Console.WriteLine("Printing accelerometer event:");
                                Console.WriteLine("\ttimestamp: {0}", accelerometer.v1.timestamp);
                                Console.WriteLine("\tacceleration: {0} {1} {2}", accelerometer.v1.accel.x, accelerometer.v1.accel.y, accelerometer.v1.accel.z);
                                // Console.WriteLine("\tangular_velocity: {0} {1} {2}", accelerometer.angular_velocity.x, accelerometer.angular_velocity.y, accelerometer.angular_velocity.z);
                                // Console.WriteLine("\teuler_angles: {0} {1} {2}", accelerometer.euler_angles.x, accelerometer.euler_angles.y, accelerometer.euler_angles.z);
                                break;
                            case Ohc.uniffi.DeviceEventKind.TrackingEvent tracking:
                                // Console.WriteLine("Printing tracking event:");
                                // Console.WriteLine("\ttimestamp: {0}", tracking.timestamp);
                                Console.WriteLine("\taimpoint: {0} {1}", tracking.v1.aimpoint.x, tracking.v1.aimpoint.y);
                                // if (tracking.pose != null) {
                                //     Console.WriteLine("\tpose: ");
                                //     Console.WriteLine("\t\trotation: ");
                                //     Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m11, tracking.pose.rotation.m12, tracking.pose.rotation.m13);
                                //     Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m21, tracking.pose.rotation.m22, tracking.pose.rotation.m23);
                                //     Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m31, tracking.pose.rotation.m32, tracking.pose.rotation.m33);
                                //     Console.WriteLine("\t\ttranslation: ");
                                //     Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.translation.x, tracking.pose.translation.y, tracking.pose.translation.z);
                                //     Console.WriteLine("\t\tscreen_id: {0}", tracking.screen_id);
                                // } else {
                                //     Console.WriteLine("\tpose: Not resolved");
                                // }
                                // Console.WriteLine("\tdistance: {0}", tracking.distance);
                                break;
                            case Ohc.uniffi.DeviceEventKind.ImpactEvent impact:
                                // Console.WriteLine("Printing impact event:");
                                // Console.WriteLine("\ttimestamp: {0}", impact.timestamp);
                                break;
                            case Ohc.uniffi.DeviceEventKind.ConnectEvent _:
                                Console.WriteLine("Device connected");
                                // Resetting on connect is currently redundant because the device zero isometry is the identity on connect
                                // await client.ResetZero(deviceEvent.v1.device);
                                // _ = Task.Run(async () => {
                                //     await Task.Delay(5000);
                                //     // negative Y is up, positive X is right
                                //     // the following translation says the bore is 0.0381 meters above the device in the device's local coordinate system
                                //     var translation = new Ohc.uniffi.Vector3f32(0, -0.0381f, 0);
                                //     // zero target is middle of the screen
                                //     var target = new Ohc.uniffi.Vector2f32(0.5f, 0.5f);
                                //     await client.Zero(deviceEvent.v1.device, translation, target);
                                //     Console.WriteLine("Zeroed device");
                                // });
                                Task malfunction_zero_task = client.WriteVendor(deviceEvent.v1.device, 0x84, [0x00]);
                                await malfunction_zero_task;
                                Task device_type_task = client.WriteVendor(deviceEvent.v1.device, 0x90, []);
                                await device_type_task;
                                break;
                            case Ohc.uniffi.DeviceEventKind.DisconnectEvent _:
                                Console.WriteLine("Device disconnected");
                                break;
                            case Ohc.uniffi.DeviceEventKind.ZeroResult zeroResult:
                                if (zeroResult.v1) {
                                    Console.WriteLine("Zero result: success");
                                } else {
                                    Console.WriteLine("Zero result: failure");
                                }
                                break;
                            case Ohc.uniffi.DeviceEventKind.PacketEvent packet:
                                switch (packet.v1.data) {
                                    case Ohc.uniffi.PacketData.Unsupported _:
                                        Console.WriteLine("Unsupported packet");
                                        break;
                                    case Ohc.uniffi.PacketData.VendorEvent vendorPacketData:
                                        Console.WriteLine("Printing vendor packet:");
                                        Console.WriteLine("\ttype: {0}", packet.v1.ty);
                                        Console.WriteLine("\tdata: {0}", BitConverter.ToString(vendorPacketData.v1.data));
                                        break;
                                }
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

