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
