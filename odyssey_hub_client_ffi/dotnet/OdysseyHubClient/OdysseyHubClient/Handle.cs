namespace Radiosity.OdysseyHubClient
{
    public class Handle
    {
        unsafe internal CsBindgen.Handle* _handle;

        public Handle() {
            unsafe {
                _handle = CsBindgen.NativeMethods.init();
            }
        }

        ~Handle() {
            unsafe {
                CsBindgen.NativeMethods.free(_handle);
            }
        }
    }
}
