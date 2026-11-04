C# wrapper library over native Odyssey client library

All native operations now report `ClientException` consistently. `NotConnected`
means no connection has been established; `RpcFailure` exposes the original gRPC
status code and message through its source object, and `ConnectionFailure`
retains a typed local transport/socket error. `InvalidArgument` rejects malformed
device records and accessory identities rather than panicking or truncating them.
Regenerate the bindings after updating the native library.

Build all target frameworks with the .NET 10 SDK (`dotnet build`).
The runtime targets remain .NET Standard 2.0/2.1 and .NET 8; the .NET 8 SDK's
compiler rejects PolySharp 1.16's generated embedded attributes for the
.NET Standard targets.

Subscription helpers write setup and streaming errors as the final channel item,
complete the channel on exit, and dispose the native stream. Keep consuming until
the channel completes to observe the terminal error.

Dispose the client before the scripting domain reloads or the process shuts down
(e.g. from Unity `OnDestroy`). Disposal ends every subscription with `StreamEnd`
and fails pending and later calls with `NotConnected`; a native call left pending
across a domain reload calls back into unloaded code and crashes the Editor.