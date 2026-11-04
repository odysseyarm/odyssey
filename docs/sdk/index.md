# Client SDK

The Odyssey client SDK connects your application to the Odyssey hub, which runs
alongside Odyssey Desktop and talks to the devices. Through it you receive
device connections, tracking poses and events, and send commands back.

The SDK is written in Rust. Other languages use bindings generated from it:

| Language | Binding | Source |
|---|---|---|
| Rust | native crate | [`crates/odyssey-hub-client`](https://github.com/odysseyarm/odyssey/tree/master/crates/odyssey-hub-client) |
| C, C++ | C ABI with a C++ helper header | [`sdk/odyssey-hub-client-ffi`](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-ffi) |
| C#, Python | UniFFI | [`sdk/odyssey-hub-client-uniffi`](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-uniffi) |

Before writing tracking code, read [Coordinate systems](./coordinate-systems.md):
it explains which way each axis points in the poses the SDK returns and how to
convert them for engines such as Unity.

## Installing

### C#

Install the [`OdysseyArm.HubClient`](https://www.nuget.org/packages/OdysseyArm.HubClient)
package from NuGet:

```sh
dotnet add package OdysseyArm.HubClient
```

### Python

Install [`odysseyhubclient`](https://pypi.org/project/odysseyhubclient/) from PyPI:

```sh
pip install odysseyhubclient
```

The module is imported as `ohc_uniffi`. See the
[Python sample](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-uniffi/samples/py/odyhubclient).

### C and C++

With vcpkg:

1. Add the registry `https://github.com/odysseyarm/vcpkg-registry` to your
   project's `vcpkg-configuration.json`.
2. Install the `odyssey-hub-client` port.

Without vcpkg, download `odyssey-hub-client.zip` from the latest
[`odyssey-hub-client-ffi` release](https://github.com/odysseyarm/odyssey/releases?q=odyssey-hub-client-ffi).

Either way, the package provides a CMake config, so you can use:

```cmake
find_package(odyssey_hub_client CONFIG REQUIRED)
```

See the [C sample](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-ffi/samples/c/odyssey-hub-client)
and the [C++ sample](https://github.com/odysseyarm/odyssey/tree/master/sdk/odyssey-hub-client-ffi/samples/cpp/odyssey-hub-client).

### Rust

The crate is not on crates.io yet. Depend on it from git:

```toml
[dependencies]
odyssey-hub-client = { git = "https://github.com/odysseyarm/odyssey" }
```
