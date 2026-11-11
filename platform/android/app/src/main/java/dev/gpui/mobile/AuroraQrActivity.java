package dev.gpui.mobile;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.media.Image;
import android.os.Bundle;
import android.util.Log;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.activity.ComponentActivity;
import androidx.camera.core.CameraSelector;
import androidx.camera.core.ImageAnalysis;
import androidx.camera.core.ImageProxy;
import androidx.camera.core.Preview;
import androidx.camera.lifecycle.ProcessCameraProvider;
import androidx.camera.view.PreviewView;
import androidx.core.content.ContextCompat;

import com.google.android.gms.tasks.Task;
import com.google.common.util.concurrent.ListenableFuture;
import com.google.mlkit.vision.barcode.BarcodeScanner;
import com.google.mlkit.vision.barcode.BarcodeScannerOptions;
import com.google.mlkit.vision.barcode.BarcodeScanning;
import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.common.InputImage;

import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Full screen live camera view that scans for an {@code aurora://} QR code.
 *
 * <p>Same approach as tauri-plugin-barcode-scanner: a CameraX {@link PreviewView} plus an
 * {@link ImageAnalysis} feeding ML Kit. The plugin draws into its host webview's window; a
 * NativeActivity window is owned by the native surface and never draws views, so the scanner is
 * an activity of its own. Rust starts it with {@link #start}, then polls {@link #poll}:
 * {@code null} while scanning, {@code ""} when the user cancelled, else the scanned text.</p>
 */
public final class AuroraQrActivity extends ComponentActivity {

    private static final String TAG = "AuroraQr";
    private static final String PREFIX = "aurora://";

    private static volatile String result;
    private static volatile AuroraQrActivity current;

    private BarcodeScanner scanner;
    private ExecutorService analysis;

    /** Opens the scanner. */
    public static void start(Activity from) {
        result = null;
        from.startActivity(new Intent(from, AuroraQrActivity.class));
    }

    /** {@code null} while scanning, {@code ""} when cancelled, else the scanned text. */
    public static String poll() {
        return result;
    }

    /** Closes a running scanner; {@link #poll} then returns {@code ""}. */
    public static void cancel() {
        final AuroraQrActivity a = current;
        if (a != null) a.runOnUiThread(a::finish);
    }

    private static int dp(Context c, float v) {
        return (int) TypedValue.applyDimension(
                TypedValue.COMPLEX_UNIT_DIP, v, c.getResources().getDisplayMetrics());
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        current = this;

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        PreviewView preview = new PreviewView(this);
        root.addView(preview, fill());
        root.addView(new Frame(this), fill());

        TextView hint = new TextView(this);
        hint.setText("Scan the QR code shown on your computer");
        hint.setTextColor(Color.WHITE);
        hint.setTextSize(16);
        hint.setGravity(Gravity.CENTER);
        FrameLayout.LayoutParams hintLp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP);
        hintLp.topMargin = dp(this, 96);
        root.addView(hint, hintLp);

        Button cancel = new Button(this);
        cancel.setText("Cancel");
        cancel.setOnClickListener(v -> finish());
        FrameLayout.LayoutParams cancelLp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM | Gravity.CENTER_HORIZONTAL);
        cancelLp.bottomMargin = dp(this, 64);
        root.addView(cancel, cancelLp);
        setContentView(root);

        analysis = Executors.newSingleThreadExecutor();
        scanner = BarcodeScanning.getClient(new BarcodeScannerOptions.Builder()
                .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                .build());

        final ListenableFuture<ProcessCameraProvider> future = ProcessCameraProvider.getInstance(this);
        future.addListener(() -> {
            try {
                ProcessCameraProvider provider = future.get();
                Preview p = new Preview.Builder().build();
                p.setSurfaceProvider(preview.getSurfaceProvider());
                ImageAnalysis a = new ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build();
                a.setAnalyzer(analysis, this::analyze);
                provider.unbindAll();
                provider.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, p, a);
            } catch (Exception e) {
                Log.e(TAG, "camera failed", e);
                finish();
            }
        }, ContextCompat.getMainExecutor(this));
    }

    private static FrameLayout.LayoutParams fill() {
        return new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
    }

    @SuppressLint("UnsafeOptInUsageError")
    private void analyze(ImageProxy proxy) {
        Image image = proxy.getImage();
        BarcodeScanner s = scanner;
        if (image == null || s == null) {
            proxy.close();
            return;
        }
        InputImage input = InputImage.fromMediaImage(image, proxy.getImageInfo().getRotationDegrees());
        Task<List<Barcode>> task = s.process(input);
        task.addOnSuccessListener(codes -> {
            for (Barcode code : codes) {
                String raw = code.getRawValue();
                // Keep scanning past unrelated codes.
                if (raw != null && raw.startsWith(PREFIX) && result == null) {
                    result = raw;
                    runOnUiThread(this::finish);
                    return;
                }
            }
        });
        task.addOnCompleteListener(t -> proxy.close());
    }

    @Override
    protected void onDestroy() {
        // Back button, Cancel, errors: no code means cancelled.
        if (result == null) result = "";
        if (current == this) current = null;
        if (scanner != null) scanner.close();
        if (analysis != null) analysis.shutdown();
        super.onDestroy();
    }

    /** Dimmed surround with a square window, the same framing as the positron scan page. */
    private static final class Frame extends View {
        private final Paint dim = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path path = new Path();

        Frame(Context c) {
            super(c);
            dim.setColor(0xCC000000);
            stroke.setColor(0xFF3B6BFF);
            stroke.setStyle(Paint.Style.STROKE);
            stroke.setStrokeWidth(dp(c, 4));
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float side = Math.min(getWidth(), getHeight()) * 0.7f;
            float left = (getWidth() - side) / 2f;
            float top = (getHeight() - side) / 2f;
            RectF hole = new RectF(left, top, left + side, top + side);
            float r = dp(getContext(), 24);
            path.reset();
            path.setFillType(Path.FillType.EVEN_ODD);
            path.addRect(0, 0, getWidth(), getHeight(), Path.Direction.CW);
            path.addRoundRect(hole, r, r, Path.Direction.CW);
            canvas.drawPath(path, dim);
            canvas.drawRoundRect(hole, r, r, stroke);
        }
    }
}
