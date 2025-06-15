using System.Numerics;
using System.Runtime.InteropServices;
using System.Threading.Channels;

using ohc = Radiosity.OdysseyHubClient.uniffi;

namespace Radiosity.OdysseyHubClient
{
    public class Client {
        private ohc.Client _inner;

        public Client() {
            unsafe {
                _inner = new ohc.Client();
            }
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(ohc.Event? @event, ohc.ClientException? @error);

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
        public Task<List<ohc.Device>> GetDeviceList() {
            return _inner.GetDeviceList();
        }

        /// <summary>
        /// This function starts a stream of events from the device.
        /// </summary>
        /// <param name="channelWriter"></param>
        public Task RunStream(Handle handle, ChannelWriter<(ohc.Event, ohc.ClientException)> channelWriter) {
            unsafe {
                EventDelegate event_delegate = (ohc.Event? @event, ohc.ClientException? @error) => {
                    if (@event != null) {
                        channelWriter.WriteAsync((@event, @error));
                    }
                    if (error != null) {
                        switch (error) {
                            case ohc.ClientException.NotConnected:
                            case ohc.ClientException.StreamEnd:
                            default:
                                channelWriter.Complete();
                                return;
                        }
                    }
                };
                var callback = Marshal.GetFunctionPointerForDelegate(event_delegate);
                return _inner.RunStream(new ohc.EventCallback(callback));
            }
        }

        /// <summary>
        /// This function sends a vendor packet to the device.
        /// </summary>
        /// <param name="device"></param>
        /// <param name="tag"></param>
        /// <param name="data"></param>
        /// <returns></returns>
        public Task WriteVendor(ohc.Device device, byte tag, byte[] data) {
            return _inner.WriteVendor(device, tag, data);
        }

        public Task ResetZero(ohc.Device device) {
            return _inner.ResetZero(device);
        }

        public Task Zero(ohc.Device device, ohc.Vector3f32 translation, ohc.Vector2f32 target) {
            return _inner.Zero(device, translation, target);
        }

        public Task<ohc.ScreenInfo> GetScreenInfoById(Handle handle, byte screenId) {
            return _inner.GetScreenInfoById(screenId);
        }
    }
}
