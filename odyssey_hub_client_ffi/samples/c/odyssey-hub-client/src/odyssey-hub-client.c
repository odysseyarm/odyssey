// odyssey-hub-client.cpp : Defines the entry point for the application.
//

#include "odyssey-hub-client.h"

#include <stdio.h>
#include <string.h>

#include <odyssey_hub_client_lib.h>

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

void client_connect_callback(struct OdysseyHubClientUserObj _, enum OdysseyHubClientClientError error);
void start_streams(struct OdysseyHubClientHandle* handle, struct OdysseyHubClientClient* client);
void stream_callback(struct OdysseyHubClientUserObj userdata, enum OdysseyHubClientClientError error, struct OdysseyHubClientEvent reply);
void log_write(const char* str);

size_t log_pos;
char log_buf[1024];
bool end = false;

struct State {
    struct OdysseyHubClientHandle* handle;
    struct OdysseyHubClientClient* client;
};

int main()
{
    struct OdysseyHubClientHandle* handle = odyssey_hub_client_init();
    struct OdysseyHubClientClient* client = odyssey_hub_client_client_new();

    struct State state = { .handle = handle, .client = client };

    odyssey_hub_client_client_connect(handle, (struct OdysseyHubClientUserObj) { ._0 = &state }, client, client_connect_callback);

    while (!end) {
        if (log_pos > 0) {
            printf("%s", log_buf);
            log_pos = 0;
        }
        msleep(10);
    }

    if (log_pos > 0) {
        printf("%s", log_buf);
        log_pos = 0;
    }

    odyssey_hub_client_free(handle);

	return 0;
}

void client_connect_callback(struct OdysseyHubClientUserObj userdata, enum OdysseyHubClientClientError error)
{
    switch (error)
    {
    case ODYSSEY_HUB_CLIENT_CLIENT_ERROR_CLIENT_ERROR_NONE:
        log_write("Connected\n");
        struct State *state = (struct State*)userdata._0;
        start_streams(state->handle, state->client);
        break;
    case ODYSSEY_HUB_CLIENT_CLIENT_ERROR_CLIENT_ERROR_CONNECT_FAILURE:
        log_write("Connection failed\n");
        end = true;
        break;
    default:
        printf("Unknown error\n");
        end = true;
        break;
    }
}

void start_streams(struct OdysseyHubClientHandle *handle, struct OdysseyHubClientClient *client) {
    odyssey_hub_client_start_stream(handle, (struct OdysseyHubClientUserObj) { ._0 = NULL }, client, stream_callback);
}

void stream_callback(struct OdysseyHubClientUserObj _, enum OdysseyHubClientClientError error, struct OdysseyHubClientEvent reply) {
    switch (error) {
    case ODYSSEY_HUB_CLIENT_CLIENT_ERROR_CLIENT_ERROR_NONE:
        break;
    case ODYSSEY_HUB_CLIENT_CLIENT_ERROR_CLIENT_ERROR_NOT_CONNECTED:
        log_write("Not connected\n");
        end = true;
        return;
    default:
        printf("Unknown error\n");
        end = true;
        return;
    }

    switch (reply.tag) {
    case ODYSSEY_HUB_CLIENT_EVENT_TAG_DEVICE_EVENT:
        ;
        OdysseyHubClientDeviceEvent event = reply.u.device_event;

        log_write("Device: ");
        switch (event.device.tag) {
        case ODYSSEY_HUB_CLIENT_DEVICE_TAG_CDC:
            log_write(event.device.u.cdc.uuid);
            break;
        case ODYSSEY_HUB_CLIENT_DEVICE_TAG_UDP:
            log_write(event.device.u.udp.uuid);
            break;
        case ODYSSEY_HUB_CLIENT_DEVICE_TAG_HID:
            // not implemented
            break;
        }
        log_write("\n");

        switch (event.kind.tag) {
        case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_CONNECT_EVENT:
            log_write("Connected\n");
            break;
        case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_DISCONNECT_EVENT:
            log_write("Disconnected\n");
            break;
        case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_TRACKING_EVENT:
            ;
            OdysseyHubClientTrackingEvent tracking = event.kind.u.tracking_event;
            char buf[17];
            snprintf(buf, sizeof(buf), "Tracking: %f %f\n", tracking.aimpoint.x, tracking.aimpoint.y);
            log_write(buf);
            break;
        case ODYSSEY_HUB_CLIENT_DEVICE_EVENT_KIND_TAG_IMPACT_EVENT:
            log_write("Impact\n");
            break;
        }

        break;
    case ODYSSEY_HUB_CLIENT_EVENT_TAG_NONE:
        break;
    }
}

void log_write(const char* str)
{
    size_t len = strlen(str);
    memcpy(log_buf + log_pos, str, len);
    log_pos += len;
    log_buf[log_pos] = '\0';
}
