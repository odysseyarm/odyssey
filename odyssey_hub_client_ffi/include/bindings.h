#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef enum ClientError {
  ClientErrorNone,
  ClientErrorConnectFailure,
  ClientErrorNotConnected,
  ClientErrorStreamEnd,
  ClientErrorEnd,
} ClientError;

typedef enum DeviceEventKindTag {
  AccelerometerEvent,
  TrackingEvent,
  ImpactEvent,
  ConnectEvent,
  DisconnectEvent,
  PacketEvent,
} DeviceEventKindTag;

typedef enum DeviceTag {
  Udp,
  Hid,
  Cdc,
} DeviceTag;

typedef enum EventTag {
  None,
  DeviceEvent,
} EventTag;

typedef enum PacketDataTag {
  Unsupported,
  VendorEvent,
} PacketDataTag;

typedef struct Client Client;

typedef struct Handle Handle;

typedef struct UserObj {
  const void *_0;
} UserObj;

typedef struct SocketAddr {
  const char *ip;
  uint16_t port;
} SocketAddr;

typedef struct UdpDevice {
  uint8_t id;
  struct SocketAddr addr;
  uint8_t uuid[6];
} UdpDevice;

typedef struct HidDevice {
  const char *path;
  uint8_t uuid[6];
} HidDevice;

typedef struct CdcDevice {
  const char *path;
  uint8_t uuid[6];
} CdcDevice;

typedef union DeviceU {
  struct UdpDevice udp;
  struct HidDevice hid;
  struct CdcDevice cdc;
} DeviceU;

typedef struct Device {
  enum DeviceTag tag;
  union DeviceU u;
} Device;

typedef struct Vector3f64 {
  double x;
  double y;
  double z;
} Vector3f64;

typedef struct AccelerometerEvent {
  uint32_t timestamp;
  struct Vector3f64 accel;
  struct Vector3f64 gyro;
  struct Vector3f64 euler_angles;
} AccelerometerEvent;

typedef struct Vector2f64 {
  double x;
  double y;
} Vector2f64;

typedef struct Matrix3f64 {
  double m11;
  double m12;
  double m13;
  double m21;
  double m22;
  double m23;
  double m31;
  double m32;
  double m33;
} Matrix3f64;

typedef struct Matrix3x1f64 {
  double x;
  double y;
  double z;
} Matrix3x1f64;

typedef struct Pose {
  struct Matrix3f64 rotation;
  struct Matrix3x1f64 translation;
} Pose;

typedef struct TrackingEvent {
  uint32_t timestamp;
  struct Vector2f64 aimpoint;
  struct Pose pose;
  bool pose_resolved;
  uint32_t screen_id;
} TrackingEvent;

typedef struct ImpactEvent {
  uint32_t timestamp;
} ImpactEvent;

typedef struct ConnectEvent {
  uint8_t _unused;
} ConnectEvent;

typedef struct DisconnectEvent {
  uint8_t _unused;
} DisconnectEvent;

typedef struct UnsupportedPacketData {
  uint8_t _unused;
} UnsupportedPacketData;

typedef struct VendorEventPacketData {
  uint8_t len;
  uint8_t data[98];
} VendorEventPacketData;

typedef union PacketDataU {
  struct UnsupportedPacketData unsupported;
  struct VendorEventPacketData vendor_event;
} PacketDataU;

typedef struct PacketData {
  enum PacketDataTag tag;
  union PacketDataU u;
} PacketData;

typedef struct PacketEvent {
  uint8_t ty;
  struct PacketData data;
} PacketEvent;

typedef union DeviceEventKindU {
  struct AccelerometerEvent accelerometer_event;
  struct TrackingEvent tracking_event;
  struct ImpactEvent impact_event;
  struct ConnectEvent connect_event;
  struct DisconnectEvent disconnect_event;
  struct PacketEvent packet_event;
} DeviceEventKindU;

typedef struct DeviceEventKind {
  enum DeviceEventKindTag tag;
  union DeviceEventKindU u;
} DeviceEventKind;

typedef struct DeviceEvent {
  struct Device device;
  struct DeviceEventKind kind;
} DeviceEvent;

typedef union EventU {
  uint8_t none;
  struct DeviceEvent device_event;
} EventU;

typedef struct Event {
  enum EventTag tag;
  union EventU u;
} Event;

struct Handle *init(void);

void free(struct Handle *handle);

struct Client *client_new(void);

void client_connect(const struct Handle *handle,
                    struct UserObj userdata,
                    struct Client *client,
                    void (*callback)(struct UserObj userdata, enum ClientError error));

void client_get_device_list(const struct Handle *handle,
                            struct UserObj userdata,
                            struct Client *client,
                            void (*callback)(struct UserObj userdata,
                                             enum ClientError error,
                                             struct Device *device_list,
                                             uintptr_t size));

void start_stream(const struct Handle *handle,
                  struct UserObj userdata,
                  struct Client *client,
                  void (*callback)(struct UserObj userdata,
                                   enum ClientError error,
                                   struct Event reply));
