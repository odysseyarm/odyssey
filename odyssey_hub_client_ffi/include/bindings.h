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

typedef enum FfiDeviceEventKindTag {
  TrackingEvent,
} FfiDeviceEventKindTag;

typedef enum FfiDeviceTag {
  Udp,
  Hid,
  Cdc,
} FfiDeviceTag;

typedef enum FfiEventTag {
  None,
  DeviceEvent,
} FfiEventTag;

typedef struct Client Client;

typedef struct FfiSocketAddr {
  const char *ip;
  uint16_t port;
} FfiSocketAddr;

typedef struct FfiUdpDevice {
  uint8_t id;
  struct FfiSocketAddr addr;
} FfiUdpDevice;

typedef struct FfiHidDevice {
  const char *path;
} FfiHidDevice;

typedef struct FfiCdcDevice {
  const char *path;
} FfiCdcDevice;

typedef union FfiU {
  struct FfiUdpDevice udp;
  struct FfiHidDevice hid;
  struct FfiCdcDevice cdc;
} FfiU;

typedef struct FfiDevice {
  enum FfiDeviceTag tag;
  union FfiU u;
} FfiDevice;

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

typedef struct FfiPose {
  struct Matrix3f64 rotation;
  struct Matrix3x1f64 translation;
} FfiPose;

typedef struct FfiTrackingEvent {
  struct Vector2f64 aimpoint;
  struct FfiPose pose;
  bool pose_resolved;
} FfiTrackingEvent;

typedef union FfiDeviceEventKindU {
  struct FfiTrackingEvent tracking_event;
} FfiDeviceEventKindU;

typedef struct FfiDeviceEventKind {
  enum FfiDeviceEventKindTag tag;
  union FfiDeviceEventKindU u;
} FfiDeviceEventKind;

typedef struct FfiDeviceEvent {
  struct FfiDevice device;
  struct FfiDeviceEventKind kind;
} FfiDeviceEvent;

typedef union FfiEventU {
  uint8_t none;
  struct FfiDeviceEvent device_event;
} FfiEventU;

typedef struct FfiEvent {
  enum FfiEventTag tag;
  union FfiEventU u;
} FfiEvent;

struct Client *client_new(void);

void client_connect(struct Client *client, void (*callback)(enum ClientError error));

void client_get_device_list(struct Client *client, void (*callback)(enum ClientError error,
                                                                    struct FfiDevice *device_list,
                                                                    uintptr_t size));

void start_stream(struct Client *client, void (*callback)(enum ClientError error,
                                                          struct FfiEvent reply));
