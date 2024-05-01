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
  TrackingEvent,
  ImpactEvent,
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
} UdpDevice;

typedef struct HidDevice {
  const char *path;
} HidDevice;

typedef struct CdcDevice {
  const char *path;
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
} TrackingEvent;

typedef struct ImpactEvent {
  uint32_t timestamp;
} ImpactEvent;

typedef union DeviceEventKindU {
  struct TrackingEvent tracking_event;
  struct ImpactEvent impact_event;
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
