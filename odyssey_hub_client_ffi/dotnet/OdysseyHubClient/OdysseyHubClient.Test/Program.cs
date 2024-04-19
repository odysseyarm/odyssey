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

foreach (var device in devices) {
    Console.WriteLine(device);
}

Console.WriteLine("Devices printed!");

Channel<OdysseyHubClient.IEvent> eventChannel = Channel.CreateUnbounded<OdysseyHubClient.IEvent>();
client.StartStream(handle, eventChannel.Writer);

await foreach (var @event in eventChannel.Reader.ReadAllAsync()) {
    switch (@event) {
        case OdysseyHubClient.DeviceEvent deviceEvent:
            switch (deviceEvent.kind) {
                case OdysseyHubClient.DeviceEvent.Tracking tracking:
                    Console.WriteLine("Tracking event: {0}", tracking);
                    break;
                default:
                    break;
            }
            break;
        default:
            break;
    }
}
