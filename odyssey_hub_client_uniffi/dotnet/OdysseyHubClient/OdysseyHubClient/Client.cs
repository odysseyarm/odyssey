using System.Runtime.InteropServices;
using System.Threading.Channels;

using Ohc = Radiosity.OdysseyHubClient;

// lol
using System.ComponentModel;

namespace System.Runtime.CompilerServices
{
    [EditorBrowsable(EditorBrowsableState.Never)]
    internal class IsExternalInit{}
}
// end lol

namespace Radiosity.OdysseyHubClient
{
    public class Client {
        private uniffi.Client _inner;

        public Client() {
            unsafe {
                _inner = new uniffi.Client();
            }
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(uniffi.Event? @event, uniffi.ClientException? @error);

        /// <summary>
        /// This function connects to the hub.
        /// </summary>
        /// <returns></returns>
        public Task Connect() {
            return _inner.Connect();
        }

        /// <summary>
        /// This function retrieves a list of devices connected to the hub.
        /// </summary>
        /// <returns></returns>
        public Task<List<uniffi.DeviceRecord>> GetDeviceList() {
            return _inner.GetDeviceList();
        }

        /// <summary>
        /// This function starts a stream of events from the device.
        /// </summary>
        /// <param name="channelWriter"></param>
        public Task RunStream(ChannelWriter<(uniffi.Event?, uniffi.ClientException?)> channelWriter) {
            return Task.Run(async () => {
                while (true) {
                    var (@event, error) = await _inner.PollEvent();
                    await channelWriter.WriteAsync((@event, @error));
                    if (error != null) {
                        switch (error) {
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

        public Task StopStream() {
            return _inner.StopStream();
        }

        /// <summary>
        /// This function sends a vendor packet to the device.
        /// </summary>
        /// <param name="device"></param>
        /// <param name="tag"></param>
        /// <param name="data"></param>
        /// <returns></returns>
        public Task WriteVendor(uniffi.DeviceRecord device, byte tag, byte[] data) {
            return _inner.WriteVendor(device, tag, data);
        }

        public Task ResetZero(uniffi.DeviceRecord device) {
            return _inner.ResetZero(device);
        }

        public Task Zero(uniffi.DeviceRecord device, uniffi.Vector3f32 translation, uniffi.Vector2f32 target) {
            return _inner.Zero(device, translation, target);
        }

        public Task<uniffi.ScreenInfo> GetScreenInfoById(byte screenId) {
            return _inner.GetScreenInfoById(screenId);
        }
    }
}
