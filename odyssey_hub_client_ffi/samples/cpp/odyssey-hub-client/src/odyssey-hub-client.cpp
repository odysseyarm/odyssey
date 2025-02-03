#include "odyssey_hub_client_lib.hpp"

#include <iostream>
#include <thread>
#include <atomic>
#include <memory>
#include <chrono>

using namespace OdysseyHubClient;

std::atomic_bool end(false);

class ClientState {
public:
    std::shared_ptr<Handle> handle;
    std::shared_ptr<Client> client;

    ClientState() : handle(odyssey_hub_client_init(), odyssey_hub_client_free),
                    client(odyssey_hub_client_client_new(), odyssey_hub_client_client_free) {}
};

void clientConnectCallback(UserObj userdata, ClientError error);
void startStreams(std::shared_ptr<Handle> handle, std::shared_ptr<Client> client);
void streamCallback(UserObj userdata, ClientError error, const char *error_msg, Event reply);
void handleDeviceEvent(const DeviceEvent &event);

void handleConnectEvent(ClientState *state) {
    odyssey_hub_client_client_connect(state->handle.get(), UserObj{state}, state->client.get(), clientConnectCallback);
}

void clientConnectCallback(UserObj userdata, ClientError error) {
    auto *state = const_cast<ClientState *>(static_cast<const ClientState *>(userdata._0));
    if (error == ClientError::ClientErrorNone) {
        std::cout << "Connected to hub" << std::endl;
        startStreams(state->handle, state->client);
    } else {
        switch (error) {
        case ClientError::ClientErrorConnectFailure:
            std::cout << "Connection failure" << std::endl;
            break;
        default:
            std::cout << "Unknown error" << std::endl;
            break;
        }
        end.store(true);
    }
}

void startStreams(std::shared_ptr<Handle> handle, std::shared_ptr<Client> client) {
    odyssey_hub_client_start_stream(handle.get(), UserObj{nullptr}, client.get(), streamCallback);
}

void streamCallback(UserObj, ClientError error, const char *error_msg, Event reply) {
    if (error != ClientError::ClientErrorNone) {
        end.store(true);
        return;
    }
    
    std::thread([reply]() {
        if (reply.tag == EventTag::DeviceEvent) {
            handleDeviceEvent(reply.u.device_event);
        }
    }).detach();
}

void handleDeviceEvent(const DeviceEvent &event) {
    std::cout << "Device: ";
    
    switch (event.device.tag) {
    case DeviceTag::Cdc:
    case DeviceTag::Udp:
        std::cout << std::hex << static_cast<int>(event.device.u.cdc.uuid[0]) << ":"
                  << static_cast<int>(event.device.u.cdc.uuid[1]) << ":"
                  << static_cast<int>(event.device.u.cdc.uuid[2]) << ":"
                  << static_cast<int>(event.device.u.cdc.uuid[3]) << ":"
                  << static_cast<int>(event.device.u.cdc.uuid[4]) << ":"
                  << static_cast<int>(event.device.u.cdc.uuid[5]) << std::endl;
        break;
    case DeviceTag::Hid:
        // Not implemented
        break;
    }

    switch (event.kind.tag) {
    case DeviceEventKindTag::ConnectEvent:
        std::cout << "Connected" << std::endl;
        break;
    case DeviceEventKindTag::DisconnectEvent:
        std::cout << "Disconnected" << std::endl;
        break;
    case DeviceEventKindTag::TrackingEvent:
        std::cout << "Tracking: aimpoint " << event.kind.u.tracking_event.aimpoint.x << " "
                  << event.kind.u.tracking_event.aimpoint.y << ", screen_id "
                  << event.kind.u.tracking_event.screen_id << std::endl;
        break;
    case DeviceEventKindTag::ImpactEvent:
        std::cout << "Impact" << std::endl;
        break;
    case DeviceEventKindTag::AccelerometerEvent:
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
