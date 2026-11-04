<!-- order: 1 -->
<!-- description: Add the Odyssey client SDK to a C#, Python, C, C++, or Rust project. -->

# Installation

Your app talks to Odyssey Desktop, so first install
[Odyssey Desktop](https://odysseyarm.com/downloads/) on the computer your app
runs on. Then add the SDK for your language.

<!-- tabs: lang -->

**C#**

Install the [`OdysseyArm.HubClient`](https://www.nuget.org/packages/OdysseyArm.HubClient)
package from NuGet:

```sh
dotnet add package OdysseyArm.HubClient
```

It targets .NET Standard 2.0 and 2.1 and .NET 8, so it also works in Unity and
in the .NET version of Godot.

**Python**

Install [`odysseyhubclient`](https://pypi.org/project/odysseyhubclient/) from
PyPI:

```sh
pip install odysseyhubclient
```

The module is imported as `ohc_uniffi`.

**C and C++**

With vcpkg: add `https://github.com/odysseyarm/vcpkg-registry` to your
project's `vcpkg-configuration.json` as a registry for the `odyssey-hub-client`
port, and add `odyssey-hub-client` to your `vcpkg.json`. Without vcpkg:
download `odyssey-hub-client.zip` from the latest
[`odyssey-hub-client-ffi` release](https://github.com/odysseyarm/odyssey/releases?q=odyssey-hub-client-ffi).

Either way the package provides a CMake config and an `ohc` target:

```cmake
find_package(odyssey_hub_client CONFIG REQUIRED)
target_link_libraries(your_app PRIVATE ohc)
```

Include `ohc.h` from C or `ohc.hpp` from C++.

**Rust**

The crates are not on crates.io yet. Depend on them from git:

```toml
[dependencies]
odyssey-hub-client = { git = "https://github.com/odysseyarm/odyssey" }
odyssey-hub-common = { git = "https://github.com/odysseyarm/odyssey" }
```

Building them needs the Protocol Buffers compiler, `protoc`, on your `PATH`.

<!-- /tabs -->

> [!WARNING]
> **Unity:** dispose the client in `OnDestroy`. A call still running when Unity
> reloads its scripts calls back into unloaded code and crashes the Editor.

Next, connect and print some tracking data: [Quickstart](./quickstart.md).
