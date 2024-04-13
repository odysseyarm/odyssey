using System.Runtime.InteropServices;

namespace OdysseyHubClient
{
    public class Client {
        unsafe CsBindgen.Client* _handle;

        public Client() {
            unsafe {
                _handle = CsBindgen.NativeMethods.client_new();
            }
        }

        private List<GCHandle> gch_list = new List<GCHandle>();

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        delegate void ClientErrorDelegate(CsBindgen.ClientError error);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void DeviceListDelegate(CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        unsafe delegate void EventDelegate(CsBindgen.ClientError error, CsBindgen.Event ev);

        public Task<ClientError> Connect() {
            unsafe {
                var completion_task_source = new TaskCompletionSource<ClientError>();
                int i = gch_list.Count;
                ClientErrorDelegate completion_delegate = (CsBindgen.ClientError error) => {
                    completion_task_source.SetResult(Helpers.BindgenClientErrToClientErr(error));
                    gch_list[i].Free();
                };
                gch_list.Add(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.client_connect(_handle, (delegate* unmanaged[Cdecl]<CsBindgen.ClientError, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate_fp));
                return completion_task_source.Task;
            }
        }

        public Task<(ClientError, IDevice[])> GetDeviceList() {
            unsafe {
                var completion_task_source = new TaskCompletionSource<(ClientError, IDevice[])>();
                int i = gch_list.Count;
                DeviceListDelegate completion_delegate = (CsBindgen.ClientError error, CsBindgen.Device* devices, nuint count) => {
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
                    gch_list[i].Free();
                };
                gch_list.Add(GCHandle.Alloc(completion_delegate));
                CsBindgen.NativeMethods.client_get_device_list(_handle, (delegate* unmanaged[Cdecl]<CsBindgen.ClientError, CsBindgen.Device*, nuint, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }

    }
}
