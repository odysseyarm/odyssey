#include "odyssey-hub-client.h"

#include <stdio.h>
#include <string.h>
#include <threads.h>
#include <stdatomic.h>

#ifdef _WIN32
#include <Windows.h>
void msleep(unsigned int ms) {
    Sleep(ms);
}
#else
#include <unistd.h>
void msleep(unsigned int ms) {
    usleep(ms * 1000);
}
#endif
#include <odyssey_hub_client_lib.h>

void client_connect_callback(struct OdysseyHubClientUserObj _, enum OdysseyHubClientClientError error);
void start_streams(struct OdysseyHubClientHandle* handle, struct OdysseyHubClientClient* client);
void stream_callback(struct OdysseyHubClientUserObj userdata, enum OdysseyHubClientClientError error, struct OdysseyHubClientEvent reply);
int handle_connect_event(void* state);
int handle_stream_event(void* data);
void handle_device_event(OdysseyHubClientDeviceEvent event);

atomic_bool end = false;

struct State {
    struct OdysseyHubClientHandle* handle;
    struct OdysseyHubClientClient* client;
};

int main() {
    struct OdysseyHubClientHandle* handle = odyssey_hub_client_init();
    struct OdysseyHubClientClient* client = odyssey_hub_client_client_new();

    struct State state = {.handle = handle, .client = client};

    thrd_t connect_thread;
    if (thrd_create(&connect_thread, handle_connect_event, &state) == thrd_success) {
        thrd_detach(connect_thread);
    }

    while (!atomic_load(&end)) {
        msleep(10);
    }

    odyssey_hub_client_free(handle);
    return 0;
}

int handle_connect_event(void* data) {
    struct State* state = (struct State*)data;
    odyssey_hub_client_client_connect(state->handle, (struct OdysseyHubClientUserObj){._0 = state}, state->client, client_connect_callback);
    return 0;
}

void client_connect_callback(struct OdysseyHubClientUserObj userdata, enum OdysseyHubClientClientError error) {
    struct State *state = (struct State*)userdata._0;

    if (error == ODYSSEY_HUB_CLIENT_CLIENT_ERROR_CLIENT_ERROR_NONE) {
        start_streams(state->handle, state->client);
    } else {
        atomic_store(&end, true);
    }
}

void start_streams(struct OdysseyHubClientHandle *handle, struct OdysseyHubClientClient *client) {
    odyssey_hub_client_start_stream(handle, (struct OdysseyHubClientUserObj){._0 = NULL}, client, stream_callback);
}

void stream_callback(struct OdysseyHubClientUserObj _, enum OdysseyHubClientClientError error, struct OdysseyHubClientEvent reply) {
    thrd_t event_thread;
    if (thrd_create(&event_thread, handle_stream_event, &reply) == thrd_success) {
        thrd_detach(event_thread);
    } else {
        atomic_store(&end, true);
    }
}

int handle_stream_event(void* data) {
    struct OdysseyHubClientEvent* reply = (struct OdysseyHubClientEvent*)data;

    if (reply->tag == ODYSSEY_HUB_CLIENT_EVENT_TAG_DEVICE_EVENT) {
        OdysseyHubClientDeviceEvent event = reply->u.device_event;
        handle_device_event(event);
    }
    return 0;
}

void handle_device_event(OdysseyHubClientDeviceEvent event) {
    printf("Device: ");

    switch (event.device.tag) {
    case ODYSSEY_HUB_CLIENT_DEVICE_TAG_CDC:
        printf("%02X:%02X:%02X:%02X:%02X:%02X\n", event.device.u.cdc.uuid[0], event.device.u.cdc.uuid[1], event.device.u.cdc.uuid[2],
            event.device.u.cdc.uuid[3], event.device.u.cdc.uuid[4], event.device.u.cdc.uuid[5]);
        break;
    case ODYSSEY_HUB_CLIENT_DEVICE_TAG_UDP:
        printf("%02X:%02X:%02X:%02X:%02X:%02X\n", event.device.u.udp.uuid[0], event.device.u.udp.uuid[1], event.device.u.udp.uuid[2],
            event.device.u.udp.uuid[3], event.device.u.udp.uuid[4], event.device.u.udp.uuid[5]);
        break;
    case ODYSSEY_HUB_CLIENT_DEVICE_TAG_HID:
        // not implemented
        break;
    }

    switch (event.kind.tag) {
    case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_CONNECT_EVENT:
        printf("Connected\n");
        break;
    case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_DISCONNECT_EVENT:
        printf("Disconnected\n");
        break;
    case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_TRACKING_EVENT:
        printf("Tracking: aimpoint %f %f, screen_id %d\n", event.kind.u.tracking_event.aimpoint.x, event.kind.u.tracking_event.aimpoint.y, event.kind.u.tracking_event.screen_id);
        break;
    case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_IMPACT_EVENT:
        printf("Impact\n");
        break;
    }
}
