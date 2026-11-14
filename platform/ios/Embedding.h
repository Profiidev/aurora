#import <UIKit/UIKit.h>
#include <stdbool.h>
void gpui_ios_register_app(void);
void gpui_ios_run_demo(void);
void gpui_ios_set_embedded(void);
void *gpui_ios_get_window(void);
void *gpui_ios_view_controller(void *window);
void gpui_ios_layout_view(void *window);
bool gpui_ios_request_frame(void *window);
void gpui_ios_set_frame_waker(void *window, void (*waker)(void *context), void *context);
void gpui_ios_did_become_active(void *app);
void gpui_ios_will_resign_active(void *app);

// Bluetooth fallback, crates/mobile/src/bluetooth_ios.rs
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
void aurora_bt_set_foreground(bool on);
bool aurora_bt_wanted(void);
bool aurora_bt_should_dial(const uint8_t *data, size_t len, uint16_t *psm);
void aurora_bt_opened(uint64_t handle);
void aurora_bt_received(uint64_t handle, const uint8_t *data, size_t len);
ssize_t aurora_bt_take_outgoing(uint64_t handle, uint8_t *buf, size_t cap);
void aurora_bt_closed(uint64_t handle);
