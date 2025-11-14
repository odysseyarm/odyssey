using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading.Tasks;
using System.Threading.Channels;

using Ohc = Radiosity.OdysseyHubClient;

// lol
using System.ComponentModel;

namespace System.Runtime.CompilerServices
{
    [EditorBrowsable(EditorBrowsableState.Never)]
    internal class IsExternalInit { }
}
// end lol

namespace Radiosity.OdysseyHubClient
{
    public class Client
    {
        private uniffi.Client _inner;

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
        /// This function connects to the hub.
        /// </summary>
        /// <returns></returns>
        public Task Connect()
        {
            return _inner.Connect();
        }

        /// <summary>
        /// This function retrieves a list of devices connected to the hub.
        /// </summary>
        /// <returns></returns>
        public Task<uniffi.Device[]> GetDeviceList()
        {
            return _inner.GetDeviceList();
        }

        /// <summary>
        /// Get the current accessory map snapshot.
        /// </summary>
        public Task<uniffi.AccessoryMapEntry[]> GetAccessoryMap()
        {
            return _inner.GetAccessoryMap();
        }

        /// <summary>
        /// This function starts a stream of events from the device.
        /// </summary>
        /// <param name="channelWriter"></param>
        public Task SubscribeEvents(ChannelWriter<(uniffi.Event?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                var stream = await _inner.SubscribeEvents();
                while (true)
                {
                    try
                    {
                        var @event = await stream.Next();
                        await channelWriter.WriteAsync((@event, null));
                    }
                    catch (uniffi.ClientException @error)
                    {
                        await channelWriter.WriteAsync((null, @error));
                        switch (error)
                        {
                            case uniffi.ClientException.NotConnected:
                            case uniffi.ClientException.StreamEnd:
                            default:
                                channelWriter.Complete();
                                return;
                        }
                    }
                }
            });
        }

        /// <summary>
        /// Subscribe to accessory map updates, writing each snapshot to the provided channel.
        /// </summary>
        public Task SubscribeAccessoryMap(ChannelWriter<(uniffi.AccessoryMapEntry[]?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                var stream = await _inner.SubscribeAccessoryMap();
                while (true)
                {
                    try
                    {
                        var snapshot = await stream.Next();
                        await channelWriter.WriteAsync((snapshot, null));
                    }
                    catch (uniffi.ClientException error)
                    {
                        await channelWriter.WriteAsync((null, error));
                        switch (error)
                        {
                            case uniffi.ClientException.NotConnected:
                            case uniffi.ClientException.StreamEnd:
                            default:
                                channelWriter.Complete();
                                return;
                        }
                    }
                }
            });
        }

        public Task SubscribeShotDelay(uniffi.Device device, ChannelWriter<(ushort?, uniffi.ClientException?)> channelWriter)
        {
            return Task.Run(async () =>
            {
                var stream = await _inner.SubscribeShotDelay(device);
                while (true)
                {
                    try
                    {
                        var delay = await stream.Next();
                        await channelWriter.WriteAsync((delay, null));
                    }
                    catch (uniffi.ClientException error)
                    {
                        await channelWriter.WriteAsync((null, error));
                        switch (error)
                        {
                            case uniffi.ClientException.NotConnected:
                            case uniffi.ClientException.StreamEnd:
                            default:
                                channelWriter.Complete();
                                return;
                        }
                    }
                }
            });
        }

        /// <summary>
        /// This function sends a vendor packet to the device.
        /// </summary>
        /// <param name="device"></param>
        /// <param name="tag"></param>
        /// <param name="data"></param>
        /// <returns></returns>
        public Task WriteVendor(uniffi.Device device, byte tag, byte[] data)
        {
            return _inner.WriteVendor(device, tag, data);
        }

        public Task ResetZero(uniffi.Device device)
        {
            return _inner.ResetZero(device);
        }

        public Task Zero(uniffi.Device device, uniffi.Vector3f32 translation, uniffi.Vector2f32 target)
        {
            return _inner.Zero(device, translation, target);
        }

        public Task<ushort> ResetShotDelay(uniffi.Device device)
        {
            return _inner.ResetShotDelay(device);
        }

        public Task<ushort> GetShotDelay(uniffi.Device device)
        {
            return _inner.GetShotDelay(device);
        }

        public Task SetShotDelay(uniffi.Device device, ushort delay_ms)
        {
            return _inner.SetShotDelay(device, delay_ms);
        }

        public Task SaveShotDelay(uniffi.Device device)
        {
            return _inner.SaveShotDelay(device);
        }

        public Task<uniffi.ScreenInfo> GetScreenInfoById(byte screenId)
        {
            return _inner.GetScreenInfoById(screenId);
        }

        /// <summary>
        /// Update AccessoryInfo map entries on the server.
        /// </summary>
        public Task UpdateAccessoryInfoMap(uniffi.AccessoryInfoMapEntry[] entries)
        {
            return _inner.UpdateAccessoryInfoMap(entries);
        }
    }
}
