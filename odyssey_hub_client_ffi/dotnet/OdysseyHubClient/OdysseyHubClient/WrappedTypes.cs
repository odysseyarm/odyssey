using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Radiosity.OdysseyHubClient
{
    public interface IDevice;

    public class UdpDevice: IDevice {
        public byte id;
        public SocketAddr addr;

        internal UdpDevice(CsBindgen.UdpDevice udpDevice) {
            id = udpDevice.id;
            addr = new SocketAddr(udpDevice.addr);
        }
    }

    public class HidDevice: IDevice {
        public string? path;

        internal HidDevice(CsBindgen.HidDevice hidDevice) {
            unsafe { path = Marshal.PtrToStringAnsi((IntPtr)hidDevice.path); }
        }
    }

    public class CdcDevice: IDevice {
        public string? path;

        internal CdcDevice(CsBindgen.CdcDevice cdcDevice) {
            unsafe { path = Marshal.PtrToStringAnsi((IntPtr)cdcDevice.path); }
        }
    }

    public class SocketAddr {
        public string? ip;
        public ushort port;

        internal SocketAddr(CsBindgen.SocketAddr socketAddr) {
            unsafe { ip = Marshal.PtrToStringAnsi((IntPtr)socketAddr.ip); }
            port = socketAddr.port;
        }
    }

    public enum ClientError {
        None,
        ConnectFailure,
        NotConnected,
        StreamEnd,
        End,
    }

    public interface IEvent;

    public class NoneEvent: IEvent {}

    public class DeviceEvent: IEvent {
        public IDevice? device;
        public IKind? kind;

        internal DeviceEvent(CsBindgen.DeviceEvent deviceEvent) {
            switch (deviceEvent.device.tag) {
                case CsBindgen.DeviceTag.Udp:
                    device = new UdpDevice(deviceEvent.device.u.udp);
                    break;
                case CsBindgen.DeviceTag.Hid:
                    device = new HidDevice(deviceEvent.device.u.hid);
                    break;
                case CsBindgen.DeviceTag.Cdc:
                    device = new CdcDevice(deviceEvent.device.u.cdc);
                    break;
            }
            switch (deviceEvent.kind.tag) {
                case CsBindgen.DeviceEventKindTag.TrackingEvent:
                    kind = new Tracking(deviceEvent.kind.u.tracking_event);
                    break;
            }
        }

        public interface IKind;

        public class Tracking : IKind {
            public uint timestamp;
            public Matrix2x1<double> aimpoint;
            public Pose? pose;

            internal Tracking(CsBindgen.TrackingEvent tracking) {
                timestamp = tracking.timestamp;
                aimpoint = new Matrix2x1<double> { x = tracking.aimpoint.x, y = tracking.aimpoint.y };
                if (tracking.pose_resolved) {
                    pose = new Pose(tracking.pose);
                }
            }
        }
    }

    public class Pose {
        public Matrix3x1<double> translation;
        public Matrix3<double> rotation;

        internal Pose(CsBindgen.Pose pose) {
            translation = new Matrix3x1<double> { x = pose.translation.x, y = pose.translation.y, z = pose.translation.z };
            rotation = new Matrix3<double> {
                m11 = pose.rotation.m11,
                m12 = pose.rotation.m12,
                m13 = pose.rotation.m13,
                m21 = pose.rotation.m21,
                m22 = pose.rotation.m22,
                m23 = pose.rotation.m23,
                m31 = pose.rotation.m31,
                m32 = pose.rotation.m32,
                m33 = pose.rotation.m33,
            };
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Matrix3<T>
    {
        public T m11;
        public T m12;
        public T m13;
        public T m21;
        public T m22;
        public T m23;
        public T m31;
        public T m32;
        public T m33;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Matrix3x1<T>
    {
        public T x;
        public T y;
        public T z;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Matrix2x1<T>
    {
        public T x;
        public T y;
    }

    internal class Helpers
    {
        static internal ClientError BindgenClientErrToClientErr(CsBindgen.ClientError error) {
            switch (error) {
                case CsBindgen.ClientError.ClientErrorNone:
                    return ClientError.None;
                case CsBindgen.ClientError.ClientErrorConnectFailure:
                    return ClientError.ConnectFailure;
                case CsBindgen.ClientError.ClientErrorNotConnected:
                    return ClientError.NotConnected;
                case CsBindgen.ClientError.ClientErrorStreamEnd:
                    return ClientError.StreamEnd;
                case CsBindgen.ClientError.ClientErrorEnd:
                default:
                    return ClientError.End;
            }
        }

        static internal IEvent EventFactory(CsBindgen.Event ev) {
            switch (ev.tag) {
                case CsBindgen.EventTag.None:
                default:
                    return new NoneEvent();
                case CsBindgen.EventTag.DeviceEvent:
                    return new DeviceEvent(ev.u.device_event);
            }
        }
    }
}
