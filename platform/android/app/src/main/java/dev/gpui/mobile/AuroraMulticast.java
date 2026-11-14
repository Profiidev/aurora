package dev.gpui.mobile;

import android.content.Context;
import android.net.wifi.WifiManager;

/** Wi-Fi multicast lock for mDNS, held only while device discovery is on. */
public final class AuroraMulticast {
    private static WifiManager.MulticastLock lock;

    private AuroraMulticast() {}

    public static synchronized void set(Context context, boolean on) {
        if (on && lock == null) {
            WifiManager wifi = (WifiManager) context.getApplicationContext()
                    .getSystemService(Context.WIFI_SERVICE);
            if (wifi == null) {
                return;
            }
            lock = wifi.createMulticastLock("aurora-mdns");
            lock.setReferenceCounted(false);
            lock.acquire();
        } else if (!on && lock != null) {
            lock.release();
            lock = null;
        }
    }
}
