using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading.Tasks;
using System.Threading.Channels;

using Ohc = OdysseyArm.HubClient;

namespace OdysseyArm.HubClient
{
    /// <summary>Hub client.</summary>
    /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
    public class Client : IDisposable
    {
        private uniffi.Client _inner;

        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Client()
        {
            unsafe
            {
                _inner = new uniffi.Client();
            }
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(uniffi.Event? @event, uniffi.ClientException? @error);

        /// <summary>
        /// Connects to the hub. Every other call fails with NotConnected until this completes.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task Connect()
        {
            return _inner.Connect();
        }

        /// <summary>
        /// The devices currently connected to the hub.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task<uniffi.Device[]> GetDeviceList()
        {
            return _inner.GetDeviceList();
        }

        /// <summary>
        /// A snapshot of the accessory map.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task<uniffi.AccessoryMapEntry[]> GetAccessoryMap()
        {
            return _inner.GetAccessoryMap();
        }

        /// <summary>
        /// Streams events from every device on the hub into <paramref name="channelWriter"/>.
        /// An error ends the stream: it is written to the channel, then the channel is completed.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SubscribeEvents(ChannelWriter<(uniffi.Event?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                try
                {
                    using var stream = await _inner.SubscribeEvents();
                    while (true)
                    {
                        var @event = await stream.Next();
                        await channelWriter.WriteAsync((@event, null));
                    }
                }
                catch (uniffi.ClientException error)
                {
                    await channelWriter.WriteAsync((null, error));
                }
                finally
                {
                    channelWriter.TryComplete();
                }
            });
        }

        /// <summary>
        /// Writes the full device list to <paramref name="channelWriter"/> whenever a device
        /// connects or disconnects. An error is written, then the channel is completed.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SubscribeDeviceList(ChannelWriter<(uniffi.Device[]?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                try
                {
                    using var stream = await _inner.SubscribeDeviceList();
                    while (true)
                    {
                        var deviceList = await stream.Next();
                        await channelWriter.WriteAsync((deviceList, null));
                    }
                }
                catch (uniffi.ClientException error)
                {
                    await channelWriter.WriteAsync((null, error));
                }
                finally
                {
                    channelWriter.TryComplete();
                }
            });
        }

        /// <summary>
        /// Writes each accessory map snapshot to <paramref name="channelWriter"/>. An error is
        /// written, then the channel is completed.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SubscribeAccessoryMap(ChannelWriter<(uniffi.AccessoryMapEntry[]?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                try
                {
                    using var stream = await _inner.SubscribeAccessoryMap();
                    while (true)
                    {
                        var snapshot = await stream.Next();
                        await channelWriter.WriteAsync((snapshot, null));
                    }
                }
                catch (uniffi.ClientException error)
                {
                    await channelWriter.WriteAsync((null, error));
                }
                finally
                {
                    channelWriter.TryComplete();
                }
            });
        }

        /// <summary>Streams shot-delay updates from device.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SubscribeShotDelay(uniffi.Device device, ChannelWriter<(ushort?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                try
                {
                    using var stream = await _inner.SubscribeShotDelay(device);
                    while (true)
                    {
                        var delay = await stream.Next();
                        await channelWriter.WriteAsync((delay, null));
                    }
                }
                catch (uniffi.ClientException error)
                {
                    await channelWriter.WriteAsync((null, error));
                }
                finally
                {
                    channelWriter.TryComplete();
                }
            });
        }

        /// <summary>
        /// Sends a vendor packet with the given tag to the device.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task WriteVendor(uniffi.Device device, byte tag, byte[] data)
        {
            return _inner.WriteVendor(device, tag, data);
        }

        /// <summary>Resets device zeroing.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task ResetZero(uniffi.Device device)
        {
            return _inner.ResetZero(device);
        }

        /// <summary>Sets device zeroing offset and target.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task Zero(uniffi.Device device, uniffi.Vector3f32 translation, uniffi.Vector2f32 target)
        {
            return _inner.Zero(device, translation, target);
        }

        /// <summary>Resets device shot delay.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task<ushort> ResetShotDelay(uniffi.Device device)
        {
            return _inner.ResetShotDelay(device);
        }

        /// <summary>Gets device shot delay.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task<ushort> GetShotDelay(uniffi.Device device)
        {
            return _inner.GetShotDelay(device);
        }

        /// <summary>Sets device shot delay in milliseconds.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SetShotDelay(uniffi.Device device, ushort delay_ms)
        {
            return _inner.SetShotDelay(device, delay_ms);
        }

        /// <summary>Saves device shot delay.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task SaveShotDelay(uniffi.Device device)
        {
            return _inner.SaveShotDelay(device);
        }

        /// <summary>Gets screen information by ID.</summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task<uniffi.ScreenInfo> GetScreenInfoById(byte screenId)
        {
            return _inner.GetScreenInfoById(screenId);
        }

        /// <summary>
        /// Update AccessoryInfo map entries on the server.
        /// </summary>
        /// <since>UniFFI SDK 0.2.1 (earliest tagged release).</since>
        public Task UpdateAccessoryInfoMap(uniffi.AccessoryInfoMapEntry[] entries)
        {
            return _inner.UpdateAccessoryInfoMap(entries);
        }

        /// <summary>
        /// Ends every subscription task and fails every pending call with NotConnected.
        /// Call it (e.g. from Unity <c>OnDestroy</c>) before the scripting domain unloads.
        /// A native call still pending afterwards calls back into unloaded code.
        /// </summary>
        /// <since>UniFFI SDK 0.3.0.</since>
        public void Dispose()
        {
            _inner.Close();
            _inner.Dispose();
        }
    }
}
