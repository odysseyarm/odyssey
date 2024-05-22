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
            Console.WriteLine("\t\tAddr: {0}", udpDevice.addr);
            Console.WriteLine("\t\tUUID: {0}", BitConverter.ToString(udpDevice.uuid));
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
}

Console.WriteLine("Devices printed!");

Channel<OdysseyHubClient.IEvent> eventChannel = Channel.CreateUnbounded<OdysseyHubClient.IEvent>();
client.StartStream(handle, eventChannel.Writer);

await foreach (var @event in eventChannel.Reader.ReadAllAsync()) {
    switch (@event) {
        case OdysseyHubClient.DeviceEvent deviceEvent:
            switch (deviceEvent.kind) {
                case OdysseyHubClient.DeviceEvent.Tracking tracking:
                    // Console.WriteLine(tracking);
                    Console.WriteLine("Printing tracking event:");
                    Console.WriteLine("\ttimestamp: {0}", tracking.timestamp);
                    Console.WriteLine("\taimpoint: {0} {1}", tracking.aimpoint.x, tracking.aimpoint.y);
                    if (tracking.pose != null) {
                        Console.WriteLine("\tpose: ");
                        Console.WriteLine("\t\trotation: ");
                        Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m11, tracking.pose.rotation.m12, tracking.pose.rotation.m13);
                        Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m21, tracking.pose.rotation.m22, tracking.pose.rotation.m23);
                        Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.rotation.m31, tracking.pose.rotation.m32, tracking.pose.rotation.m33);
                        Console.WriteLine("\t\ttranslation: ");
                        Console.WriteLine("\t\t\t{0} {1} {2}", tracking.pose.translation.x, tracking.pose.translation.y, tracking.pose.translation.z);
                        Console.WriteLine("\t\tscreen_id: {0}", tracking.screen_id);
                    } else {
                        Console.WriteLine("\tpose: Not resolved");
                    }
                    break;
                case OdysseyHubClient.DeviceEvent.Impact impact:
                    Console.WriteLine("Printing impact event:");
                    Console.WriteLine("\ttimestamp: {0}", impact.timestamp);
                    break;
                default:
                    break;
            }
            PrintDevice(deviceEvent.device);
            break;
        default:
            break;
    }
}
