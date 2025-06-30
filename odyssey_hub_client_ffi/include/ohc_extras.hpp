#ifndef OHC_EXTRAS_HPP
#define OHC_EXTRAS_HPP

#include <cstdint>
#include <ohc.hpp>

namespace ohc {
namespace extras {

class TrackingHistory {
public:
    explicit TrackingHistory(uint32_t capacity) {
        ptr = tracking_history_new(capacity);
    }

    ~TrackingHistory() {
        if (ptr) {
            tracking_history_free(ptr);
            ptr = nullptr;
        }
    }

    void push(const TrackingEvent& event) {
        tracking_history_push(ptr, event);
    }

    bool get_closest(uint32_t timestamp, TrackingEvent& out) {
        return tracking_history_get_closest(ptr, timestamp, &out);
    }

    TrackingHistory(const TrackingHistory&) = delete;
    TrackingHistory& operator=(const TrackingHistory&) = delete;

private:
    ohc::TrackingHistory* ptr = nullptr;
};

} // namespace extras
} // namespace ohc

#endif // OHC_EXTRAS_HPP
