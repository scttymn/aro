package org.aro.hello;

import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import java.io.InputStream;
import java.net.URL;
import javax.net.ssl.HttpsURLConnection;
import android.os.Environment;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import java.io.File;
import java.io.FileWriter;
import java.io.BufferedReader;
import java.io.FileReader;
import android.content.Context;
import android.content.Intent;
import android.graphics.Color;
import android.os.Bundle;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;

public class MainActivity extends Activity {
    private static final String TAG = "HelloARO";
    private int taps = 0;
    private final int[] colors = { 0xFF1E88E5, 0xFF43A047, 0xFFE53935, 0xFF8E24AA, 0xFFF9A825 };

    class Root extends LinearLayout {
        Root(Context c) { super(c); }
        @Override public boolean onTouchEvent(MotionEvent e) {
            boolean r = super.onTouchEvent(e);
            Log.i(TAG, "Root.onTouchEvent action=" + e.getActionMasked() + " -> " + r + " pressed=" + isPressed() + " clickable=" + isClickable());
            return r;
        }
        @Override public boolean performClick() {
            Log.i(TAG, "Root.performClick");
            return super.performClick();
        }
    }

    private TextView tv;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Log.i(TAG, "onCreate");
        final Root root = new Root(this);
        root.setBackgroundColor(colors[0]);
        root.setGravity(Gravity.CENTER);
        tv = new TextView(this);
        tv.setText("Hello from ARO\nTap me");
        tv.setTextSize(48);
        tv.setTextColor(Color.WHITE);
        tv.setGravity(Gravity.CENTER);
        root.addView(tv);
        root.setClickable(true);
        root.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                taps++;
                Log.i(TAG, "click " + taps + " -> startActivity(SecondActivity)");
                startActivity(new Intent("org.aro.hello.action.SHOW"));
                root.setBackgroundColor(colors[taps % colors.length]);
                tv.setText("Tapped " + taps);
                NotificationManager nm = getSystemService(NotificationManager.class);
                nm.createNotificationChannel(new NotificationChannel("taps", "Taps", NotificationManager.IMPORTANCE_DEFAULT));
                Notification n = new Notification.Builder(MainActivity.this, "taps")
                        .setSmallIcon(android.R.drawable.ic_dialog_info)
                        .setContentTitle("Hello from ARO")
                        .setContentText("Tapped " + taps + " time" + (taps == 1 ? "" : "s"))
                        .build();
                nm.notify(1, n);
            }
        });
        setContentView(root);
        probeStorage();
        probeNetwork();
    }


    private void probeStorage() {
        try {
            File priv = new File(getFilesDir(), "aro-private.txt");
            FileWriter w = new FileWriter(priv);
            w.write("private-ok"); w.close();
            BufferedReader r = new BufferedReader(new FileReader(priv));
            Log.i(TAG, "storage: private " + priv + " -> " + r.readLine());
            r.close();
        } catch (Exception e) { Log.w(TAG, "storage: private failed: " + e); }
        Log.i(TAG, "storage: externalState=" + Environment.getExternalStorageState());
        try {
            File sd = Environment.getExternalStorageDirectory();
            String[] list = sd.list();
            Log.i(TAG, "storage: /sdcard=" + sd + " entries=" + (list == null ? "null" : list.length));
            File out = new File(sd, "aro-hello.txt");
            FileWriter w = new FileWriter(out);
            w.write("hello from ARO to the host home"); w.close();
            Log.i(TAG, "storage: wrote " + out);
        } catch (Exception e) { Log.w(TAG, "storage: /sdcard failed: " + e); }
    }

    private void probeNetwork() {
        ConnectivityManager cm = getSystemService(ConnectivityManager.class);
        Network active = cm.getActiveNetwork();
        NetworkCapabilities nc = active == null ? null : cm.getNetworkCapabilities(active);
        NetworkInfo ni = cm.getActiveNetworkInfo();
        Log.i(TAG, "net: activeNetwork=" + active
                + " internet=" + (nc != null && nc.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET))
                + " validated=" + (nc != null && nc.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED))
                + " wifi=" + (nc != null && nc.hasTransport(NetworkCapabilities.TRANSPORT_WIFI))
                + " info=" + (ni == null ? "null" : ni.getTypeName() + "/" + ni.getState()));
        new Thread(new Runnable() {
            @Override public void run() {
                httpGet("https://1.1.1.1/");        // literal IP: socket + TLS, no DNS
                httpGet("https://example.com/");    // hostname: needs DNS
            }
        }).start();
    }

    private void httpGet(String url) {
        try {
            HttpsURLConnection c = (HttpsURLConnection) new URL(url).openConnection();
            c.setConnectTimeout(5000);
            c.setReadTimeout(5000);
            int code = c.getResponseCode();
            InputStream in = c.getInputStream();
            int n = 0, b;
            while ((b = in.read()) != -1 && n < 64) n++;
            in.close();
            Log.i(TAG, "http " + url + " -> " + code + " (" + n + "+ bytes)");
        } catch (Exception e) {
            Log.w(TAG, "http " + url + " failed: " + e);
        }
    }

    @Override protected void onStart() { super.onStart(); Log.i(TAG, "onStart"); }
    @Override protected void onResume() { super.onResume(); Log.i(TAG, "onResume"); }
}
