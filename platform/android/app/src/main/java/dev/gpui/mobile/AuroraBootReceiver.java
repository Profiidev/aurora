package dev.gpui.mobile;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Starts the engine after a reboot or an app update; it stops itself when
 *  "Run in background" is off. */
public final class AuroraBootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        if (Intent.ACTION_BOOT_COMPLETED.equals(action)
                || Intent.ACTION_MY_PACKAGE_REPLACED.equals(action)) {
            AuroraService.start(context, true);
        }
    }
}
