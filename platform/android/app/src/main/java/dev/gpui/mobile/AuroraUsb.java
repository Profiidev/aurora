package dev.gpui.mobile;

import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbAccessory;
import android.hardware.usb.UsbManager;
import android.os.ParcelFileDescriptor;

/**
 * The USB accessory a computer running Aurora switched this phone to. Rust
 * polls {@link #open} and runs the link over the returned file descriptor.
 */
public final class AuroraUsb {
    private static final String ACTION_PERMISSION = "dev.gpui.mobile.USB_PERMISSION";
    private static final String MANUFACTURER = "Aurora";
    private static final String MODEL = "Aurora";
    /** Android's own "open Aurora?" dialog gets this long before we ask. */
    private static final long ASK_AFTER_MS = 5000;
    private static boolean asked;
    /** When the current accessory showed up, 0 while there is none. */
    private static long seenAt;

    private AuroraUsb() {}

    /** Whether an Aurora accessory is still plugged in. */
    public static boolean attached(Context context) {
        UsbManager usb = (UsbManager) context.getSystemService(Context.USB_SERVICE);
        if (usb == null || usb.getAccessoryList() == null) {
            return false;
        }
        for (UsbAccessory accessory : usb.getAccessoryList()) {
            if (MANUFACTURER.equals(accessory.getManufacturer())
                    && MODEL.equals(accessory.getModel())) {
                return true;
            }
        }
        return false;
    }

    /**
     * Opens the accessory and hands its descriptor to the caller, who owns it
     * from then on. -1 while there is none, or the user has not allowed it yet.
     */
    public static synchronized int open(Context context) {
        UsbManager usb = (UsbManager) context.getSystemService(Context.USB_SERVICE);
        if (usb == null) {
            return -1;
        }
        UsbAccessory[] list = usb.getAccessoryList();
        if (list == null) {
            asked = false;
            seenAt = 0;
            return -1;
        }
        for (UsbAccessory accessory : list) {
            if (!MANUFACTURER.equals(accessory.getManufacturer())
                    || !MODEL.equals(accessory.getModel())) {
                continue;
            }
            if (!usb.hasPermission(accessory)) {
                // accepting the attach dialog grants it; ask ourselves only if
                // that does not happen, once per plug, so two dialogs never stack
                long now = android.os.SystemClock.elapsedRealtime();
                if (seenAt == 0) {
                    seenAt = now;
                }
                if (!asked && now - seenAt >= ASK_AFTER_MS) {
                    asked = true;
                    Intent intent = new Intent(ACTION_PERMISSION).setPackage(context.getPackageName());
                    PendingIntent pending = PendingIntent.getBroadcast(
                            context, 0, intent, PendingIntent.FLAG_IMMUTABLE);
                    usb.requestPermission(accessory, pending);
                }
                return -1;
            }
            ParcelFileDescriptor pfd = usb.openAccessory(accessory);
            return pfd == null ? -1 : pfd.detachFd();
        }
        return -1;
    }
}
