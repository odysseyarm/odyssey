using System.Diagnostics;
using System.Numerics;
using System.Threading.Channels;

using OdysseyHubClient = Radiosity.OdysseyHubClient;

OdysseyHubClient.Handle handle = new OdysseyHubClient.Handle();

OdysseyHubClient.Client client = new OdysseyHubClient.Client();

Task<OdysseyHubClient.ClientError> connectTask = client.Connect(handle);

switch (await connectTask) {
    case OdysseyHubClient.ClientError.None:
        Console.WriteLine("Connected!");
        break;
    case OdysseyHubClient.ClientError.ConnectFailure:
        Console.WriteLine("Failed to connect!");
        break;
    default:
        Environment.Exit(1);
        break;
}

Task<(OdysseyHubClient.ClientError, OdysseyHubClient.IDevice[])> deviceListTask = client.GetDeviceList(handle);

var (error, devices) = await deviceListTask;

switch (error) {
    case OdysseyHubClient.ClientError.None:
        break;
    case OdysseyHubClient.ClientError.ConnectFailure:
        Console.WriteLine("Failed to connect!");
        break;
    case OdysseyHubClient.ClientError.NotConnected:
        Console.WriteLine("Not connected!");
        break;
    default:
        Console.Error.WriteLine("This shouldn't be possible");
        Environment.Exit(1);
        break;
}

Console.WriteLine("Devices (count: {0}):", devices.Length);

void PrintDevice(OdysseyHubClient.IDevice device) {
    Console.WriteLine("\tDevice:");
    switch (device) {
        case OdysseyHubClient.UdpDevice udpDevice:
            Console.WriteLine("\t\tType: UDP");
            Console.WriteLine("\t\tID: {0}", udpDevice.id);
            Console.WriteLine("\t\tAddr: {0}", udpDevice.addr.ip + ":" + udpDevice.addr.port);
            break;
        case OdysseyHubClient.CdcDevice cdcDevice:
            Console.WriteLine("\t\tType: CDC");
            Console.WriteLine("\t\tPath: {0}", cdcDevice.path);
            Console.WriteLine("\t\tUUID: {0}", BitConverter.ToString(cdcDevice.uuid));
            break;
        case OdysseyHubClient.HidDevice hidDevice:
            Console.WriteLine("\t\tType: HID");
            Console.WriteLine("\t\tPath: {0}", hidDevice.path);
            Console.WriteLine("\t\tUUID: {0}", BitConverter.ToString(hidDevice.uuid));
            break;
        default:
            break;

    }
}

foreach (var device in devices) {
    PrintDevice(device);
    Task<OdysseyHubClient.ClientError> malfunction_zero_task = client.WriteVendor(handle, device, 0x84, [ 0x00 ]);
    await malfunction_zero_task;
}

Console.WriteLine("Devices printed!");

Channel<(OdysseyHubClient.IEvent, OdysseyHubClient.ClientError, string err_msg)> eventChannel = Channel.CreateUnbounded<(OdysseyHubClient.IEvent, OdysseyHubClient.ClientError, string err_msg)>();
client.StartStream(handle, eventChannel.Writer);

await foreach ((var @event, var err, var err_msg) in eventChannel.Reader.ReadAllAsync()) {
    switch (@event) {
        case OdysseyHubClient.DeviceEvent deviceEvent:
            switch (deviceEvent.kind) {
                // uncomment what you want to see
                case OdysseyHubClient.DeviceEvent.Accelerometer accelerometer:
                    // Console.WriteLine("Printing accelerometer event:");
                    // Console.WriteLine("\ttimestamp: {0}", accelerometer.timestamp);
                    // Console.WriteLine("\tacceleration: {0} {1} {2}", accelerometer.acceleration.x, accelerometer.acceleration.y, accelerometer.acceleration.z);
                    // Console.WriteLine("\tangular_velocity: {0} {1} {2}", accelerometer.angular_velocity.x, accelerometer.angular_velocity.y, accelerometer.angular_velocity.z);
                    // Console.WriteLine("\teuler_angles: {0} {1} {2}", accelerometer.euler_angles.x, accelerometer.euler_angles.y, accelerometer.euler_angles.z);
                    break;
                case OdysseyHubClient.DeviceEvent.Tracking tracking:
                    // Console.WriteLine("Printing tracking event:");
                    // Console.WriteLine("\ttimestamp: {0}", tracking.timestamp);
                    Console.WriteLine("\taimpoint: {0} {1}", tracking.aimpoint.x, tracking.aimpoint.y);
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
                case OdysseyHubClient.DeviceEvent.Impact impact:
                    // Console.WriteLine("Printing impact event:");
                    // Console.WriteLine("\ttimestamp: {0}", impact.timestamp);
                    break;
                case OdysseyHubClient.DeviceEvent.Connect _:
                    Console.WriteLine("Device connected");
                    // Resetting on connect is currently redundant because the device zero isometry is the identity on connect
                    await client.ResetZero(handle, deviceEvent.device);
                    _ = Task.Run(async () => {
                        await Task.Delay(5000);
                        // negative Y is up, positive X is right
                        // the following translation says the bore is 0.0381 meters above the device in the device's local coordinate system
                        var translation = new OdysseyHubClient.Vector3(0.0f, -0.0381f, 0.0f);
                        // zero target is middle of the screen
                        var target = new OdysseyHubClient.Vector2(0.5f, 0.5f);
                        await client.Zero(handle, deviceEvent.device, translation, target);
                        Console.WriteLine("Zeroed device");
                    });
                    break;
                case OdysseyHubClient.DeviceEvent.Disconnect _:
                    Console.WriteLine("Device disconnected");
                    break;
                case OdysseyHubClient.DeviceEvent.Packet packet:
                    switch (packet.data) {
                        case OdysseyHubClient.DeviceEvent.Packet.UnsupportedPacketData _:
                            Console.WriteLine("Unsupported packet");
                            break;
                        case OdysseyHubClient.DeviceEvent.Packet.VendorPacketData vendorPacketData:
                            Console.WriteLine("Printing vendor packet:");
                            Console.WriteLine("\ttype: {0}", packet.ty);
                            Console.WriteLine("\tdata: {0}", BitConverter.ToString(vendorPacketData.data));
                            break;
                    }
                    break;
                default:
                    break;
            }
            break;
        case OdysseyHubClient.NoneEvent:
            Console.WriteLine("Error: {0}, message: {1}", err, err_msg);
            break;
        default:
            break;
    }
}
