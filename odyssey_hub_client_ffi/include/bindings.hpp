#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <ostream>
#include <new>

enum class ClientError {
  ClientErrorNone,
  ClientErrorConnectFailure,
  ClientErrorNotConnected,
  ClientErrorStreamEnd,
  ClientErrorEnd,
};

enum class FfiDeviceEventKindTag {
  TrackingEvent,
};

enum class FfiDeviceTag {
  Udp,
  Hid,
  Cdc,
};

enum class FfiEventTag {
  None,
  DeviceEvent,
};

struct Client;

struct FfiSocketAddr {
  const char *ip;
  uint16_t port;
};

struct FfiUdpDevice {
  uint8_t id;
  FfiSocketAddr addr;
};

struct FfiHidDevice {
  const char *path;
};

struct FfiCdcDevice {
  const char *path;
};

union FfiU {
  FfiUdpDevice udp;
  FfiHidDevice hid;
  FfiCdcDevice cdc;
};

struct FfiDevice {
  FfiDeviceTag tag;
  FfiU u;
};

struct Vector2f64 {
  double x;
  double y;
};

struct Matrix3f64 {
  double m11;
  double m12;
  double m13;
  double m21;
  double m22;
  double m23;
  double m31;
  double m32;
  double m33;
};

struct Matrix3x1f64 {
  double x;
  double y;
  double z;
};

struct FfiPose {
  Matrix3f64 rotation;
  Matrix3x1f64 translation;
};

struct FfiTrackingEvent {
  Vector2f64 aimpoint;
  FfiPose pose;
  bool pose_resolved;
};

union FfiDeviceEventKindU {
  FfiTrackingEvent tracking_event;
};

struct FfiDeviceEventKind {
  FfiDeviceEventKindTag tag;
  FfiDeviceEventKindU u;
};

struct FfiDeviceEvent {
  FfiDevice device;
  FfiDeviceEventKind kind;
};

union FfiEventU {
  uint8_t none;
  FfiDeviceEvent device_event;
};

struct FfiEvent {
  FfiEventTag tag;
  FfiEventU u;
};

extern "C" {

Client *client_new();

void client_connect(Client *client, void (*callback)(ClientError error));

void client_get_device_list(Client *client, void (*callback)(ClientError error,
                                                             FfiDevice *device_list,
                                                             uintptr_t size));

void start_stream(Client *client, void (*callback)(ClientError error, FfiEvent reply));

} // extern "C"
