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

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        delegate void ClientErrorDelegate(CsBindgen.ClientError error);

        public Task Connect() {
            unsafe {
                var completion_task_source = new TaskCompletionSource<bool>();
                ClientErrorDelegate completion_delegate = (CsBindgen.ClientError error) => {
                    completion_task_source.SetResult(true);
                };
                CsBindgen.NativeMethods.client_connect(_handle, (delegate* unmanaged[Cdecl]<CsBindgen.ClientError, void>)Marshal.GetFunctionPointerForDelegate(completion_delegate));
                return completion_task_source.Task;
            }
        }
    }
}
