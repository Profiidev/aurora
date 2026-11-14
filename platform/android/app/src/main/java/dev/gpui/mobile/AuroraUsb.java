package dev.gpui.mobile;

import android.app.Activity;
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
    private static boolean asked;

    private AuroraUsb() {}

    /** Whether an Aurora accessory is still plugged in. */
    public static boolean attached(Activity activity) {
        UsbManager usb = (UsbManager) activity.getSystemService(Context.USB_SERVICE);
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
    public static synchronized int open(Activity activity) {
        UsbManager usb = (UsbManager) activity.getSystemService(Context.USB_SERVICE);
        if (usb == null) {
            return -1;
        }
        UsbAccessory[] list = usb.getAccessoryList();
        if (list == null) {
            asked = false;
            return -1;
        }
        for (UsbAccessory accessory : list) {
            if (!MANUFACTURER.equals(accessory.getManufacturer())
                    || !MODEL.equals(accessory.getModel())) {
                continue;
            }
            if (!usb.hasPermission(accessory)) {
                // opened from the attach intent the permission is implicit;
                // otherwise ask once per plug
                if (!asked) {
                    asked = true;
                    Intent intent = new Intent(ACTION_PERMISSION).setPackage(activity.getPackageName());
                    PendingIntent pending = PendingIntent.getBroadcast(
                            activity, 0, intent, PendingIntent.FLAG_IMMUTABLE);
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
