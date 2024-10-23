using System.Runtime.InteropServices;
using System.Threading.Channels;

namespace Radiosity.OdysseyHubClient
{
    public class Client {
        unsafe private CsBindgen.Client* _handle;

        public Client() {
            unsafe {
                _handle = CsBindgen.NativeMethods.odyssey_hub_client_client_new();
            }
        }

        ~Client() {
            unsafe {
                CsBindgen.NativeMethods.odyssey_hub_client_client_free(_handle);
            }
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        delegate void ClientErrorDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void DeviceListDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error, byte* err_msg, CsBindgen.Event ev);

        /// <summary>
        /// This function connects to the hub.
        /// </summary>
        /// <param name="handle">
        /// The handle of the tokio runtime.
        /// </param>
        /// <returns></returns>
        public Task<ClientError> Connect(Handle handle) {
            unsafe {
                var completion_task_source = new TaskCompletionSource<ClientError>();
                ClientErrorDelegate completion_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error) => {
                    completion_task_source.SetResult(Helpers.BindgenClientErrToClientErr(error));
                    GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.odyssey_hub_client_client_connect(handle._handle, new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }

        /// <summary>
        /// This function retrieves a list of devices connected to the hub.
        /// </summary>
        /// <param name="handle"></param>
        /// <returns></returns>
        public Task<(ClientError, IDevice[])> GetDeviceList(Handle handle) {
            unsafe {
                var completion_task_source = new TaskCompletionSource<(ClientError, IDevice[])>();
                DeviceListDelegate completion_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count) => {
                    var result = new IDevice[count];
                    for (nuint i = 0; i < count; i++) {
                        var device = devices[i];
                        switch (device.tag) {
                            case CsBindgen.DeviceTag.Udp:
                                result[i] = new UdpDevice(device);
                                break;
                            case CsBindgen.DeviceTag.Hid:
                                result[i] = new HidDevice(device);
                                break;
                            case CsBindgen.DeviceTag.Cdc:
                                result[i] = new CdcDevice(device);
                                break;
                        }
                    }
                    completion_task_source.SetResult((Helpers.BindgenClientErrToClientErr(error), result));
                    GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.odyssey_hub_client_client_get_device_list(handle._handle, new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, CsBindgen.Device*, nuint, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }

        /// <summary>
        /// This function starts a stream of events from the device.
        /// </summary>
        /// <param name="handle"></param>
        /// <param name="channelWriter"></param>
        public void StartStream(Handle handle, ChannelWriter<(IEvent, ClientError, string)> channelWriter) {
            unsafe {
                EventDelegate event_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error, byte *err_msg, CsBindgen.Event ev) => {
                    // convert err_msg to string
                    var err_msg_string = Marshal.PtrToStringAnsi((IntPtr)err_msg);
                    if (err_msg_string == null) {
                        err_msg_string = "";
                    }
                    channelWriter.WriteAsync((Helpers.EventFactory(ev), (OdysseyHubClient.ClientError)error, err_msg_string));
                    switch (error) {
                        case CsBindgen.ClientError.ClientErrorNone:
                            break;
                        case CsBindgen.ClientError.ClientErrorNotConnected:
                        case CsBindgen.ClientError.ClientErrorStreamEnd:
                        default:
                            channelWriter.Complete();
                            GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                            return;
                    }
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(event_delegate));
                CsBindgen.NativeMethods.odyssey_hub_client_start_stream(handle._handle, new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, byte*, CsBindgen.Event, void>)Marshal.GetFunctionPointerForDelegate(event_delegate));
            }
        }

        /// <summary>
        /// This function sends a vendor packet to the device.
        /// </summary>
        /// <param name="handle"></param>
        /// <param name="device"></param>
        /// <param name="tag"></param>
        /// <param name="data"></param>
        /// <returns></returns>
        public Task<ClientError> WriteVendor(Handle handle, IDevice device, byte tag, byte[] data) {
            unsafe {
                var completion_task_source = new TaskCompletionSource<ClientError>();
                ClientErrorDelegate completion_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error) => {
                    completion_task_source.SetResult(Helpers.BindgenClientErrToClientErr(error));
                    GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                };
                var ffi_device = device.device;
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(completion_delegate));
                fixed (byte* pData = data) {
                    CsBindgen.NativeMethods.odyssey_hub_client_write_vendor(handle._handle, new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, &ffi_device, tag, pData, (nuint)data.Length, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                }
                return completion_task_source.Task;
            }
        }
    }
}
