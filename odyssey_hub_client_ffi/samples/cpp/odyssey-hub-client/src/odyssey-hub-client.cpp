#include <ohc.hpp>

#include <iostream>
#include <thread>
#include <atomic>
#include <memory>
#include <chrono>

std::atomic_bool end(false);

class ClientState {
public:
    std::shared_ptr<ohc::Handle> handle;
    std::shared_ptr<ohc::Client> client;

    ClientState() : handle(ohc::rt_init(), ohc::rt_free),
                    client(ohc::client_new(), ohc::client_free) {}
};

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error);
void startStreams(std::shared_ptr<ohc::Handle> handle, std::shared_ptr<ohc::Client> client);
void streamCallback(ohc::UserObj, ohc::ClientError error, ohc::Event reply);
void handleDeviceEvent(const ohc::DeviceEvent &event);

void handleConnectEvent(ClientState *state) {
    ohc::client_connect(state->handle.get(), state->client.get(), ohc::UserObj{state}, clientConnectCallback);
}

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error) {
    auto *state = const_cast<ClientState *>(static_cast<const ClientState *>(userdata.ptr));
    if (error == ohc::ClientError::NONE) {
        std::cout << "Connected to hub" << std::endl;
        startStreams(state->handle, state->client);
    } else {
        switch (error) {
        case ohc::ClientError::CONNECT_FAILURE:
            std::cout << "Connection failure" << std::endl;
            break;
        default:
            std::cout << "Unknown error" << std::endl;
            break;
        }
        end.store(true);
    }
}

void startStreams(std::shared_ptr<ohc::Handle> handle, std::shared_ptr<ohc::Client> client) {
    ohc::client_start_stream(handle.get(), client.get(), ohc::UserObj{nullptr}, streamCallback);
}

void streamCallback(ohc::UserObj, ohc::ClientError error, ohc::Event reply) {
    if (error != ohc::ClientError::NONE) {
        end.store(true);
        return;
    }
    
    std::thread([reply]() {
        if (reply.kind.tag == ohc::EventKind::Tag::DEVICE_EVENT) {
            handleDeviceEvent(reply.kind.DEVICE_EVENT._0);
        }
    }).detach();
}

void handleDeviceEvent(const ohc::DeviceEvent &event) {
    std::cout << "Device: ";

    std::cout << "0x" << std::hex << ohc::device_uuid(event.device) << std::endl;

    switch (event.kind.tag) {
    case ohc::DeviceEventKind::Tag::CONNECT_EVENT:
        std::cout << "Connected" << std::endl;
        break;
    case ohc::DeviceEventKind::Tag::DISCONNECT_EVENT:
        std::cout << "Disconnected" << std::endl;
        break;
    case ohc::DeviceEventKind::Tag::TRACKING_EVENT:
        std::cout << "Tracking: aimpoint " << event.kind.TRACKING_EVENT._0.aimpoint.x << " "
                  << event.kind.TRACKING_EVENT._0.aimpoint.y << ", screen_id "
                  << event.kind.TRACKING_EVENT._0.screen_id << std::endl;
        break;
    case ohc::DeviceEventKind::Tag::IMPACT_EVENT:
        std::cout << "Impact" << std::endl;
        break;
    case ohc::DeviceEventKind::Tag::ACCELEROMETER_EVENT:
        break;
    default:
        break;
    }
}

int main() {
    ClientState state;
    
    std::thread connectThread(handleConnectEvent, &state);
    connectThread.detach();

    while (!end.load()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    return 0;
}
