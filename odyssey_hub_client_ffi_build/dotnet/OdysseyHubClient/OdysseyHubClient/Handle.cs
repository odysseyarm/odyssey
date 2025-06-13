namespace Radiosity.OdysseyHubClient
{
    public class Handle
    {
        unsafe internal CsBindgen.Handle* _handle;

        public Handle() {
            unsafe {
                _handle = CsBindgen.NativeMethods.odyssey_hub_client_init();
            }
        }

        ~Handle() {
            unsafe {
                CsBindgen.NativeMethods.odyssey_hub_client_free(_handle);
            }
        }
    }
}
