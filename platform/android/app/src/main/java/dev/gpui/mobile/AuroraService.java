package dev.gpui.mobile;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.net.Uri;
import android.os.IBinder;
import android.os.PowerManager;
import android.provider.Settings;
import android.util.Log;

/**
 * Runs the Aurora engine in its own process (":engine"), so the phone stays
 * connected while the app is closed and after a reboot. The app UI talks to
 * it over the local IPC socket, like the desktop app talks to aurorad.
 *
 * Android requires a notification for a foreground service; it goes to a
 * silent channel, and Aurora never asks for the notification permission for
 * it, so it does not show in the shade.
 */
public final class AuroraService extends Service {
    private static final String TAG = "aurora";
    private static final String CHANNEL = "background";
    private static final String EXTRA_FROM_BOOT = "fromBoot";
    private static final int NOTIFICATION_ID = 1;
    private static boolean started;

    /** Set by the app UI from the "Run in background" setting. */
    static volatile boolean background = true;

    private static native boolean nativeStart(Context context, boolean fromBoot);

    public static void start(Context context) {
        start(context, false);
    }

    static void start(Context context, boolean fromBoot) {
        Intent intent = new Intent(context, AuroraService.class).putExtra(EXTRA_FROM_BOOT, fromBoot);
        try {
            context.startForegroundService(intent);
        } catch (RuntimeException e) {
            Log.w(TAG, "cannot start the background service", e);
        }
    }

    public static void stop(Context context) {
        context.stopService(new Intent(context, AuroraService.class));
    }

    public static void setBackground(boolean on) {
        background = on;
    }

    /** Android's "ignore battery optimizations" prompt, once ever. */
    @SuppressLint("BatteryLife")
    public static void askBatteryExemption(Activity activity) {
        PowerManager power = (PowerManager) activity.getSystemService(Context.POWER_SERVICE);
        SharedPreferences prefs = activity.getSharedPreferences("aurora", Context.MODE_PRIVATE);
        if (power == null || power.isIgnoringBatteryOptimizations(activity.getPackageName())
                || prefs.getBoolean("askedBattery", false)) {
            return;
        }
        prefs.edit().putBoolean("askedBattery", true).apply();
        Intent intent = new Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                .setData(Uri.parse("package:" + activity.getPackageName()));
        activity.startActivity(intent);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(
                CHANNEL, "Background connection", NotificationManager.IMPORTANCE_MIN));
        Notification notification = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(getApplicationInfo().icon)
                .setContentTitle("Aurora is connected")
                .setOngoing(true)
                .build();
        startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        boolean fromBoot = intent != null && intent.getBooleanExtra(EXTRA_FROM_BOOT, false);
        if (!started) {
            System.loadLibrary("aurora");
            if (!nativeStart(this, fromBoot)) {
                // started at boot with "Run in background" off
                stopSelf();
                return START_NOT_STICKY;
            }
            started = true;
        }
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        // the engine cannot be restarted inside this process; end it
        android.os.Process.killProcess(android.os.Process.myPid());
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
