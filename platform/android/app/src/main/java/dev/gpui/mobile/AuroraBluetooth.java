package dev.gpui.mobile;

import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothSocket;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanFilter;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.ParcelUuid;

import java.io.IOException;
import java.io.InputStream;
import java.util.Arrays;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Bluetooth LE for the Rust side: scans for desktops advertising the Aurora
 * service, opens insecure L2CAP channels to them and moves their bytes.
 * Must match crates/transport/src/bluetooth/mod.rs.
 */
public final class AuroraBluetooth {
    private static final String SERVICE = "6a1f3c2e-8b4d-4e7a-9c50-aa0b7d31e5c4";
    private static final int COMPANY = 0xffff;
    private static final int READ_BUF = 16 * 1024;

    /** Set by GpuiActivity's onResume / onPause. */
    static volatile boolean foreground;

    private static final ConcurrentLinkedQueue<String> seen = new ConcurrentLinkedQueue<>();
    private static final Map<Integer, BluetoothSocket> sockets = new ConcurrentHashMap<>();
    private static final AtomicInteger nextHandle = new AtomicInteger(1);
    private static BluetoothLeScanner scanner;
    private static ScanCallback callback;

    private AuroraBluetooth() {}

    public static boolean foreground() {
        return foreground;
    }

    private static BluetoothAdapter adapter(Activity activity) {
        BluetoothManager manager =
                (BluetoothManager) activity.getSystemService(Context.BLUETOOTH_SERVICE);
        return manager == null ? null : manager.getAdapter();
    }

    /** An LE adapter exists and is switched on. */
    public static boolean available(Activity activity) {
        BluetoothAdapter adapter = adapter(activity);
        return adapter != null
                && adapter.isEnabled()
                && activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE);
    }

    /** Asks for scan and connect permissions; blocks until answered. */
    public static boolean ensurePermissions(Activity activity) {
        String result = GpuiPermissions.requestPermissions(activity,
                "android.permission.BLUETOOTH_SCAN|android.permission.BLUETOOTH_CONNECT");
        for (String status : result.split("\\|")) {
            // GpuiPermissions: 0 = granted
            if (!"0".equals(status)) {
                return false;
            }
        }
        return true;
    }

    /** Starts or stops scanning for Aurora desktops. */
    public static synchronized void scan(Activity activity, boolean on) {
        try {
            if (on && callback == null) {
                BluetoothAdapter adapter = adapter(activity);
                if (adapter == null || adapter.getBluetoothLeScanner() == null) {
                    return;
                }
                scanner = adapter.getBluetoothLeScanner();
                callback = new ScanCallback() {
                    @Override
                    public void onScanResult(int type, ScanResult result) {
                        ScanRecord record = result.getScanRecord();
                        byte[] data = record == null ? null : record.getManufacturerSpecificData(COMPANY);
                        if (data != null) {
                            seen.add(result.getDevice().getAddress() + "|" + hex(data));
                        }
                    }
                };
                ScanFilter filter = new ScanFilter.Builder()
                        .setServiceUuid(ParcelUuid.fromString(SERVICE))
                        .build();
                ScanSettings settings = new ScanSettings.Builder()
                        .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
                        .build();
                scanner.startScan(Collections.singletonList(filter), settings, callback);
            } else if (!on && callback != null) {
                scanner.stopScan(callback);
                callback = null;
                seen.clear();
            }
        } catch (SecurityException e) {
            callback = null;
        }
    }

    /** Next advert as "address|hex manufacturer data", null when none. */
    public static String poll() {
        return seen.poll();
    }

    /** Blocks until connected; a handle for read/write/close, or -1. */
    public static int connect(Activity activity, String address, int psm) {
        BluetoothAdapter adapter = adapter(activity);
        if (adapter == null) {
            return -1;
        }
        try {
            BluetoothDevice device = adapter.getRemoteDevice(address);
            BluetoothSocket socket = device.createInsecureL2capChannel(psm);
            socket.connect();
            int handle = nextHandle.getAndIncrement();
            sockets.put(handle, socket);
            return handle;
        } catch (IOException | SecurityException | IllegalArgumentException e) {
            return -1;
        }
    }

    /** Blocks for the next bytes; null once the channel is closed. */
    public static byte[] read(int handle) {
        BluetoothSocket socket = sockets.get(handle);
        if (socket == null) {
            return null;
        }
        try {
            InputStream in = socket.getInputStream();
            byte[] buf = new byte[READ_BUF];
            int n = in.read(buf);
            return n < 0 ? null : Arrays.copyOf(buf, n);
        } catch (IOException e) {
            return null;
        }
    }

    public static boolean write(int handle, byte[] data) {
        BluetoothSocket socket = sockets.get(handle);
        if (socket == null) {
            return false;
        }
        try {
            socket.getOutputStream().write(data);
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    /** Closes the channel; a read blocked on it returns. */
    public static void close(int handle) {
        BluetoothSocket socket = sockets.remove(handle);
        if (socket != null) {
            try {
                socket.close();
            } catch (IOException ignored) {
            }
        }
    }

    private static String hex(byte[] data) {
        StringBuilder sb = new StringBuilder(data.length * 2);
        for (byte b : data) {
            sb.append(String.format("%02x", b));
        }
        return sb.toString();
    }
}
