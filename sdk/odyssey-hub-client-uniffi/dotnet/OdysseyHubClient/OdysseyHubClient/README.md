C# wrapper library over native Odyssey client library

Every native operation reports `ClientException`. `NotConnected` means no
connection has been established. `RpcFailure` exposes the original gRPC status
code and message through its source object, and `ConnectionFailure` keeps a
typed local transport/socket error. `InvalidArgument` rejects malformed device
records and accessory identities, so a bad record is an error you can catch
with no panic and no truncation. Regenerate the bindings after updating the
native library.

Build all target frameworks with the .NET 10 SDK (`dotnet build`). The runtime
targets are still .NET Standard 2.0/2.1 and .NET 8. The newer SDK is needed
because the .NET 8 SDK's compiler rejects the embedded attributes PolySharp 1.16
generates for the .NET Standard targets.

Subscription helpers write setup and streaming errors as the final channel item,
complete the channel on exit, and dispose the native stream. Keep consuming
until the channel completes. Stop early and you miss the terminal error.

Dispose the client before the scripting domain reloads or the process shuts
down, for example from Unity `OnDestroy`. Disposal ends every subscription with
`StreamEnd` and fails pending and later calls with `NotConnected`. Skip it, and
a native call left pending across a domain reload calls back into unloaded code.
That crashes the Editor.
