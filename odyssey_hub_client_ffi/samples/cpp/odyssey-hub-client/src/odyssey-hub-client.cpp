#include <ohc.hpp>
#include <ohc_extras.hpp>

#include <iostream>
#include <thread>
#include <atomic>
#include <memory>
#include <chrono>
#include <mutex>
#include <unordered_map>

std::atomic_bool end(false);

class ClientState {
public:
    std::shared_ptr<ohc::Handle> handle;
    std::shared_ptr<ohc::Client> client;

    ClientState()
        : handle(ohc::rt_init(), ohc::rt_free),
          client(ohc::client_new(), ohc::client_free) {}
};

ClientState *g_state = nullptr;

// Per-device tracking state

struct DeviceData {
    ohc::extras::TrackingHistory history;
    ohc::TrackingEvent           lastTracking{};
    uint32_t                     shotDelayMS  = 0;
    bool                         hasShotDelay = false;

    DeviceData(uint32_t capacity)
        : history(capacity) {}
};

std::mutex                                    g_mutex;
std::unordered_map<uint64_t, DeviceData*>     g_devices;

// Forward declarations

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error);
void startStreams(std::shared_ptr<ohc::Handle> handle,
                  std::shared_ptr<ohc::Client> client);
void streamCallback(ohc::UserObj, ohc::ClientError error, ohc::Event reply);
void handleDeviceEvent(const ohc::DeviceEvent &event);
void requestShotDelay(uint64_t uuid, const ohc::Device &device);

// Connection handling

void handleConnectEvent(ClientState *state) {
    ohc::client_connect(
        state->handle.get(),
        state->client.get(),
        ohc::UserObj{state},
        clientConnectCallback
    );
}

void clientConnectCallback(ohc::UserObj userdata, ohc::ClientError error) {
    auto *state = const_cast<ClientState *>(
        static_cast<const ClientState *>(userdata.ptr)
    );

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

void startStreams(std::shared_ptr<ohc::Handle> handle,
                  std::shared_ptr<ohc::Client> client) {
    ohc::client_start_stream(
        handle.get(),
        client.get(),
        ohc::UserObj{nullptr},   // no userdata needed here
        streamCallback
    );
}

void requestShotDelay(uint64_t uuid, const ohc::Device &device) {
    if (!g_state) return;

    // copy shared_ptrs to extend lifetime into the thread safely
    auto handle = g_state->handle;
    auto client = g_state->client;

    std::thread([handle, client, uuid, device]() {
        ohc::client_get_shot_delay(
            handle.get(),
            client.get(),
            ohc::UserObj{reinterpret_cast<void*>(uuid)},
            device,
            [](ohc::UserObj ud, ohc::ClientError err, uint16_t delayMs) {
                uint64_t u = reinterpret_cast<uint64_t>(ud.ptr);
                if (err != ohc::ClientError::NONE) {
                    std::cout << "Failed to get shot delay for device 0x"
                              << std::hex << u << std::dec
                              << " (error " << static_cast<int>(err) << ")\n";
                    return;
                }

                std::lock_guard<std::mutex> lk(g_mutex);
                auto it = g_devices.find(u);
                if (it != g_devices.end()) {
                    it->second->shotDelayMS  = delayMs;
                    it->second->hasShotDelay = true;
                    std::cout << "Shot delay for device 0x" << std::hex << u
                              << " = " << std::dec << delayMs << " ms\n";
                }
            }
        );
    }).detach();
}

// Streaming and events

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

void handleDeviceEvent(const ohc::DeviceEvent &ev) {
    std::lock_guard<std::mutex> lk(g_mutex);

    uint64_t uuid = ohc::device_uuid(ev.device);

    if (ev.kind.tag == ohc::DeviceEventKind::Tag::CONNECT_EVENT) {
        std::cout << "Device 0x" << std::hex << uuid << std::dec
                  << ": Connected" << std::endl;

        if (!g_devices.count(uuid)) {
            auto *dd = new DeviceData(1000); // Allocate once per device
            g_devices[uuid] = dd;
            requestShotDelay(uuid, ev.device);
        }
        return;
    }

    // ignore unknown devices
    if (!g_devices.count(uuid)) {
        return;
    }

    DeviceData *dd = g_devices[uuid];

    std::cout << "Device 0x" << std::hex << uuid << std::dec << ": ";

    switch (ev.kind.tag) {
    case ohc::DeviceEventKind::Tag::DISCONNECT_EVENT:
        std::cout << "Disconnected" << std::endl;
        // you could delete dd here if you want to free on disconnect, but in real code you probably will not use new and delete, and consider lock-free queues
        break;

    case ohc::DeviceEventKind::Tag::TRACKING_EVENT: {
        auto &t = ev.kind.TRACKING_EVENT._0;
        dd->lastTracking = t;
        dd->history.push(t);

        std::cout << "Tracking: aimpoint "
                  << t.aimpoint.x << " " << t.aimpoint.y
                  << ", screen_id " << t.screen_id << std::endl;
        break;
    }

    case ohc::DeviceEventKind::Tag::IMPACT_EVENT: {
        std::cout << "Impact" << std::endl;

        uint32_t impactTimestampUs = ev.kind.IMPACT_EVENT._0.timestamp;

        if (!dd->hasShotDelay) {
            std::cout << "  (Shot delay unknown yet; not backtracking.)\n";
            break;
        }

        uint32_t delayUs       = dd->shotDelayMS * 1000u;
        uint32_t shotTimestamp = (impactTimestampUs > delayUs)
                               ? (impactTimestampUs - delayUs)
                               : impactTimestampUs;

        ohc::TrackingEvent closest;
        if (dd->history.get_closest(shotTimestamp, closest)) {
            std::cout << "  Shot aimpoint (history, delay "
                      << dd->shotDelayMS << " ms): "
                      << closest.aimpoint.x << " " << closest.aimpoint.y
                      << ", screen_id " << closest.screen_id << std::endl;
        } else {
            std::cout << "  No tracking sample found near shot time" << std::endl;
        }
        break;
    }

    case ohc::DeviceEventKind::Tag::SHOT_DELAY_CHANGED:
        dd->shotDelayMS  = ev.kind.SHOT_DELAY_CHANGED._0;
        dd->hasShotDelay = true;
        std::cout << "Shot delay updated: " << dd->shotDelayMS << " ms\n";
        break;

    case ohc::DeviceEventKind::Tag::ACCELEROMETER_EVENT:
    default:
        // ignore
        break;
    }
}

int main() {
    ClientState state;
    g_state = &state; // used by requestShotDelay

    std::thread connectThread(handleConnectEvent, &state);
    connectThread.detach();

    while (!end.load()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // cleanup DeviceData allocations
    {
        std::lock_guard<std::mutex> lk(g_mutex);
        for (auto &kv : g_devices) {
            delete kv.second;
        }
        g_devices.clear();
    }

    return 0;
}
