#ifndef OHC_EXTRAS_HPP
#define OHC_EXTRAS_HPP

#include <cstdint>
#include <ohc.hpp>

/// Since: FFI SDK 0.1.0 (earliest tagged release).
namespace ohc {
/// Since: FFI SDK 0.1.0 (earliest tagged release).
namespace extras {

/// Since: FFI SDK 0.1.0 (earliest tagged release).
class TrackingHistory {
public:
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    explicit TrackingHistory(uint32_t capacity) {
        ptr = tracking_history_new(capacity);
    }

    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    ~TrackingHistory() {
        if (ptr) {
            tracking_history_free(ptr);
            ptr = nullptr;
        }
    }

    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    void push(const TrackingEvent& event) {
        tracking_history_push(ptr, event);
    }

    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    bool latest(TrackingEvent& out) {
        return tracking_history_latest(ptr, &out);
    }

    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    bool get_closest(uint32_t timestamp, TrackingEvent& out) {
        return tracking_history_get_closest(ptr, timestamp, &out);
    }

    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    TrackingHistory(const TrackingHistory&) = delete;
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    TrackingHistory& operator=(const TrackingHistory&) = delete;

private:
    ohc::TrackingHistory* ptr = nullptr;
};

} // namespace extras
} // namespace ohc

#endif // OHC_EXTRAS_HPP
