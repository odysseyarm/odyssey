#include <ohc.hpp>

#include <iostream>
#include <thread>
#include <atomic>
#include <memory>
#include <chrono>
#include <cstdio>

std::atomic_bool end_flag(false);

class ClientState {
public:
    std::shared_ptr<ohc::Handle> handle;
    std::shared_ptr<ohc::Client> client;

    ClientState() : handle(ohc::rt_init(), ohc::rt_free),
                    client(ohc::client_new(), ohc::client_free) {}
};

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error);
void startStreams(std::shared_ptr<ohc::Handle> handle, std::shared_ptr<ohc::Client> client);
void streamCallback(ohc::UserObj, ohc::ClientError error, ohc::Event event);
void deviceListCallback(ohc::UserObj, ohc::ClientError error, ohc::Device *devices, uintptr_t len);
void handleDeviceEvent(const ohc::DeviceEvent &event);

static void printUuid(const uint8_t uuid[6]) {
    std::printf("%02x:%02x:%02x:%02x:%02x:%02x", uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]);
}

void handleConnectEvent(ClientState *state) {
    ohc::client_connect(state->handle.get(), state->client.get(), ohc::UserObj{state}, clientConnectCallback);
}

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error) {
    auto *state = const_cast<ClientState *>(static_cast<const ClientState *>(userdata.ptr));
    if (error == ohc::ClientError::NONE) {
        std::cout << "Connected to hub" << std::endl;
        startStreams(state->handle, state->client);
        // Subscribe to device list changes
        ohc::client_subscribe_device_list(state->handle.get(), state->client.get(), ohc::UserObj{nullptr}, deviceListCallback);
    } else {
        switch (error) {
        case ohc::ClientError::CONNECT_FAILURE:
            std::cout << "Connection failure" << std::endl;
            break;
        default:
            std::cout << "Unknown error" << std::endl;
            break;
        }
        end_flag.store(true);
    }
}

void startStreams(std::shared_ptr<ohc::Handle> handle, std::shared_ptr<ohc::Client> client) {
    ohc::client_start_stream(handle.get(), client.get(), ohc::UserObj{nullptr}, streamCallback);
}

void deviceListCallback(ohc::UserObj, ohc::ClientError error, ohc::Device *devices, uintptr_t len) {
    if (error != ohc::ClientError::NONE) {
        return;
    }
    std::cout << "Device list updated (" << len << " devices):" << std::endl;
    for (uintptr_t i = 0; i < len; i++) {
        std::cout << "  ";
        printUuid(devices[i].uuid);
        std::cout << " pid=0x" << std::hex << devices[i].product_id << std::dec;
        std::cout << " fw=" << devices[i].firmware_version[0] << "."
                  << devices[i].firmware_version[1] << "."
                  << devices[i].firmware_version[2];
        std::cout << std::endl;
    }
}

void streamCallback(ohc::UserObj, ohc::ClientError error, ohc::Event event) {
    if (error != ohc::ClientError::NONE) {
        end_flag.store(true);
        return;
    }

    if (event.tag == ohc::Event::Tag::DEVICE_EVENT) {
        handleDeviceEvent(event.DEVICE_EVENT._0);
    }
}

void handleDeviceEvent(const ohc::DeviceEvent &event) {
    std::cout << "Device ";
    printUuid(event.device.uuid);

    switch (event.kind.tag) {
    case ohc::DeviceEventKind::Tag::TRACKING_EVENT:
        std::cout << " Tracking: aimpoint " << event.kind.TRACKING_EVENT._0.aimpoint.x << " "
                  << event.kind.TRACKING_EVENT._0.aimpoint.y << ", screen_id "
                  << event.kind.TRACKING_EVENT._0.screen_id;
        break;
    case ohc::DeviceEventKind::Tag::IMPACT_EVENT:
        std::cout << " Impact";
        break;
    case ohc::DeviceEventKind::Tag::ACCELEROMETER_EVENT:
        break;
    case ohc::DeviceEventKind::Tag::ZERO_RESULT:
        std::cout << " Zero result: " << (event.kind.ZERO_RESULT._0 ? "success" : "failure");
        break;
    case ohc::DeviceEventKind::Tag::CAPABILITIES_CHANGED:
        std::cout << " Capabilities changed";
        break;
    default:
        break;
    }
    std::cout << std::endl;
}

int main() {
    ClientState state;

    std::thread connectThread(handleConnectEvent, &state);
    connectThread.detach();

    while (!end_flag.load()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    return 0;
}
