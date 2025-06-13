using CsBindgen;
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Radiosity.OdysseyHubClient
{
    public abstract class IDevice: IEquatable<IDevice> {
        internal CsBindgen.Device device;
        public abstract byte[] UUID { get; }

        public abstract bool Equals(IDevice? other);
    }

    /// <summary>
    /// Device that is connected to Odyssey Hub through UDP.
    /// </summary>
    public class UdpDevice: IDevice {
        public byte id;
        public SocketAddr addr;
        public byte[] uuid;

        public override byte[] UUID
        {
            get => uuid;
        }

        internal UdpDevice(CsBindgen.Device device) {
            this.device = device;
            var udpDevice = device.u.udp;
            id = udpDevice.id;
            addr = new SocketAddr(udpDevice.addr);
            unsafe {
                uuid = [
                    udpDevice.uuid[0],
                    udpDevice.uuid[1],
                    udpDevice.uuid[2],
                    udpDevice.uuid[3],
                    udpDevice.uuid[4],
                    udpDevice.uuid[5],
                ];
            }
        }

        public override bool Equals(IDevice? other) {
            if (other == null) return false;
            return this.uuid.SequenceEqual(other.UUID);
        }
    }

    /// <summary>
    /// Device that is connected to Odyssey Hub through HID (unimplemented).
    /// </summary>
    public class HidDevice: IDevice {
        public string? path;
        public byte[] uuid;

        public override byte[] UUID
        {
            get => uuid;
        }

        internal HidDevice(CsBindgen.Device device) {
            this.device = device;
            var hidDevice = device.u.hid;
            unsafe { path = Marshal.PtrToStringAnsi((IntPtr)hidDevice.path); }
            unsafe {
                uuid = [
                    hidDevice.uuid[0],
                    hidDevice.uuid[1],
                    hidDevice.uuid[2],
                    hidDevice.uuid[3],
                    hidDevice.uuid[4],
                    hidDevice.uuid[5],
                ];
            }
        }

        public override bool Equals(IDevice? other) {
            if (other == null) return false;
            return this.uuid.SequenceEqual(other.UUID);
        }
    }

    /// <summary>
    /// Device that is connected to Odyssey Hub through CDC.
    /// Could be a vision module or an ATS Lite dongle.
    /// </summary>
    public class CdcDevice: IDevice {
        public string? path;
        /// <value>Property <c>uuid</c> is the 6-byte unique identifier of the device.</value>
        public byte[] uuid;

        public override byte[] UUID
        {
            get => uuid;
        }

        internal CdcDevice(CsBindgen.Device device) {
            this.device = device;
            var cdcDevice = device.u.cdc;
            unsafe { path = Marshal.PtrToStringAnsi((IntPtr)cdcDevice.path); }
            unsafe {
                uuid = [
                    cdcDevice.uuid[0],
                    cdcDevice.uuid[1],
                    cdcDevice.uuid[2],
                    cdcDevice.uuid[3],
                    cdcDevice.uuid[4],
                    cdcDevice.uuid[5],
                ];
            }
        }

        public override bool Equals(IDevice? other) {
            if (other == null) return false;
            return this.uuid.SequenceEqual(other.UUID);
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
        public IDevice device;
        public IKind kind;

        internal DeviceEvent(CsBindgen.DeviceEvent deviceEvent) {
            switch (deviceEvent.device.tag) {
                case CsBindgen.DeviceTag.Udp:
                    device = new UdpDevice(deviceEvent.device);
                    break;
                case CsBindgen.DeviceTag.Hid:
                    device = new HidDevice(deviceEvent.device);
                    break;
                case CsBindgen.DeviceTag.Cdc:
                    device = new CdcDevice(deviceEvent.device);
                    break;
                default:
                    throw new Exception("Unknown device type");
            }
            switch (deviceEvent.kind.tag) {
                case CsBindgen.DeviceEventKindTag.AccelerometerEvent:
                    kind = new Accelerometer(deviceEvent.kind.u.accelerometer_event);
                    break;
                case CsBindgen.DeviceEventKindTag.TrackingEvent:
                    kind = new Tracking(deviceEvent.kind.u.tracking_event);
                    break;
                case CsBindgen.DeviceEventKindTag.ImpactEvent:
                    kind = new Impact(deviceEvent.kind.u.impact_event);
                    break;
                case CsBindgen.DeviceEventKindTag.ConnectEvent:
                    kind = new Connect();
                    break;
                case CsBindgen.DeviceEventKindTag.DisconnectEvent:
                    kind = new Disconnect();
                    break;
                case CsBindgen.DeviceEventKindTag.ZeroResult:
                    kind = new ZeroResult(deviceEvent.kind.u.zero_result);
                    break;
                case CsBindgen.DeviceEventKindTag.PacketEvent:
                    kind = new Packet(deviceEvent.kind.u.packet_event);
                    break;
                default:
                    throw new Exception("Unknown device tag");
            }
        }

        public interface IKind;

        public class Accelerometer : IKind
        {
            public uint timestamp;
            public Matrix3x1<float> acceleration;
            public Matrix3x1<float> angular_velocity;
            public Matrix3x1<float> euler_angles;

            internal Accelerometer(CsBindgen.AccelerometerEvent accelerometer) {
                timestamp = accelerometer.timestamp;
                acceleration = new Matrix3x1<float> { x = accelerometer.accel.x, y = accelerometer.accel.y, z = accelerometer.accel.z };
                angular_velocity = new Matrix3x1<float> { x = accelerometer.gyro.x, y = accelerometer.gyro.y, z = accelerometer.gyro.z };
                euler_angles = new Matrix3x1<float> { x = accelerometer.euler_angles.x, y = accelerometer.euler_angles.y, z = accelerometer.euler_angles.z };
            }
        }

        public class Tracking : IKind {
            public uint timestamp;
            public Matrix2x1<float> aimpoint;
            public Pose? pose;

            /// <value>Property <c>distance</c> is the distance in meters from the aimpoint on the screen</value></value>
            public float distance;

            /// <value>Property <c>screen_id</c> is between 0 and 5 (inclusive). It corresponds to the calibration file of the screen being tracked</value>
            public uint screen_id;

            internal Tracking(CsBindgen.TrackingEvent tracking) {
                timestamp = tracking.timestamp;
                aimpoint = new Matrix2x1<float> { x = tracking.aimpoint.x, y = tracking.aimpoint.y };
                if (tracking.pose_resolved) {
                    pose = new Pose(tracking.pose);
                }
                distance = tracking.distance;
                screen_id = tracking.screen_id;
            }
        }

        public class Impact : IKind {
            public uint timestamp;
            public Matrix2x1<float> aimpoint;

            internal Impact(CsBindgen.ImpactEvent impact) {
                timestamp = impact.timestamp;
            }
        }

        public class Connect : IKind
        {
            internal Connect() { }
        }

        public class Disconnect : IKind
        {
            internal Disconnect() { }
        }

        public class ZeroResult : IKind
        {
            public bool success;

            internal ZeroResult(bool success) {
                this.success = success;
            }
        }

        /// <summary>
        /// Only vendor packets are supported at the moment. The odyssey hub is currently hardcoded to subscribe to packets 0x81 through 0x83.
        /// </summary>
        public class Packet : IKind {
            public interface IPacketData;

            public class UnsupportedPacketData : IPacketData { }

            public class VendorPacketData : IPacketData
            {
                public byte[] data;

                internal VendorPacketData(CsBindgen.VendorEventPacketData vendorPacketData) {
                    data = new byte[vendorPacketData.len];
                    unsafe {
                        for (int i = 0; i < vendorPacketData.len; i++) {
                            data[i] = vendorPacketData.data[i];
                        }
                    }
                }
            }

            public uint ty;
            public IPacketData data;

            internal Packet(CsBindgen.PacketEvent packet) {
                ty = packet.ty;
                switch (packet.data.tag) {
                    case CsBindgen.PacketDataTag.Unsupported:
                        data = new UnsupportedPacketData();
                        break;
                    case CsBindgen.PacketDataTag.VendorEvent:
                        data = new VendorPacketData(packet.data.u.vendor_event);
                        break;
                    default:
                        throw new Exception("Unknown packet data tag");
                }
            }
        }
    }

    /// <summary>
    /// The pose is relative to the screen being aimed at. The translation matrix is in meters based on the assumed screen height.
    /// </summary>
    public class Pose {
        public Matrix3x1<float> translation;
        public Matrix3<float> rotation;

        internal Pose(CsBindgen.Pose pose) {
            translation = new Matrix3x1<float> { x = pose.translation.x, y = pose.translation.y, z = pose.translation.z };
            rotation = new Matrix3<float> {
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

    public class Vector3
    {
        public float x;
        public float y;
        public float z;

        public Vector3(float x, float y, float z) {
            this.x = x;
            this.y = y;
            this.z = z;
        }

        internal Vector3(CsBindgen.Vector3f32 vector3) {
            x = vector3.x;
            y = vector3.y;
            z = vector3.z;
        }

        internal CsBindgen.Vector3f32 ToFFI() {
            return new CsBindgen.Vector3f32 { x = x, y = y, z = z };
        }

        public override string ToString() {
            return $"[{x}, {y}, {z}]";
        }
    }

    public class Vector2
    {
        public float x;
        public float y;

        public Vector2(float x, float y) {
            this.x = x;
            this.y = y;
        }

        internal Vector2(CsBindgen.Vector2f32 vector3) {
            x = vector3.x;
            y = vector3.y;
        }

        internal CsBindgen.Vector2f32 ToFFI() {
            return new CsBindgen.Vector2f32 { x = x, y = y };
        }

        public override string ToString() {
            return $"[{x}, {y}]";
        }
    }

    public class ScreenInfo
    {
        public byte id;
        public Vector2 tl;
        public Vector2 tr;
        public Vector2 bl;
        public Vector2 br;

        internal ScreenInfo(CsBindgen.ScreenInfo screenInfo) {
            id = screenInfo.id;
            tl = new Vector2(screenInfo.tl);
            tr = new Vector2(screenInfo.tr);
            bl = new Vector2(screenInfo.bl);
            br = new Vector2(screenInfo.br);
        }
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
