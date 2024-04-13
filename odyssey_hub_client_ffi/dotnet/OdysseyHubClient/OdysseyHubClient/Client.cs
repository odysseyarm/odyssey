using System.Runtime.InteropServices;
using System.Threading.Channels;

namespace OdysseyHubClient
{
    public class Client {
        unsafe CsBindgen.Client* _handle;

        public Client() {
            unsafe {
                _handle = CsBindgen.NativeMethods.client_new();
            }
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        delegate void ClientErrorDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void DeviceListDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Event ev);

        public Task<ClientError> Connect() {
            unsafe {
                var completion_task_source = new TaskCompletionSource<ClientError>();
                ClientErrorDelegate completion_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error) => {
                    completion_task_source.SetResult(Helpers.BindgenClientErrToClientErr(error));
                    GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.client_connect(new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }

        public Task<(ClientError, IDevice[])> GetDeviceList() {
            unsafe {
                var completion_task_source = new TaskCompletionSource<(ClientError, IDevice[])>();
                DeviceListDelegate completion_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count) => {
                    var result = new IDevice[count];
                    for (nuint i = 0; i < count; i++) {
                        var device = devices[i];
                        switch (device.tag) {
                            case CsBindgen.DeviceTag.Udp:
                                result[i] = new UdpDevice(device.u.udp);
                                break;
                            case CsBindgen.DeviceTag.Hid:
                                result[i] = new HidDevice(device.u.hid);
                                break;
                            case CsBindgen.DeviceTag.Cdc:
                                result[i] = new CdcDevice(device.u.cdc);
                                break;
                        }
                    }
                    completion_task_source.SetResult((Helpers.BindgenClientErrToClientErr(error), result));
                    GCHandle.FromIntPtr((IntPtr)userdata.Item1).Free();
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.client_get_device_list(new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, CsBindgen.Device*, nuint, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }

        public void StartStream(ChannelWriter<IEvent> channelWriter) {
            unsafe {
                EventDelegate event_delegate = (CsBindgen.UserObj userdata, CsBindgen.ClientError error, CsBindgen.Event ev) => {
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
                    channelWriter.WriteAsync(Helpers.EventFactory(ev));
                };
                var _gc_handle = GCHandle.ToIntPtr(GCHandle.Alloc(event_delegate));
                CsBindgen.NativeMethods.start_stream(new CsBindgen.UserObj { Item1 = (void*)_gc_handle }, _handle, (delegate* unmanaged[Cdecl]<CsBindgen.UserObj, CsBindgen.ClientError, CsBindgen.Event, void>)Marshal.GetFunctionPointerForDelegate(event_delegate));
            }
        }
    }
}
