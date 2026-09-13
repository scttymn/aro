package org.aro.host;
import android.app.Activity;
import android.os.Bundle;
import android.content.Intent;
import android.net.*;
import android.widget.*;
import java.io.*;

public class MainActivity extends Activity {
    TextView output;
    void report(String text) { android.util.Log.i("AROHost", text); runOnUiThread(() -> output.append(text + "\n")); }
    void button(LinearLayout box, String label, Runnable action) { Button b = new Button(this); b.setText(label); b.setOnClickListener(v -> action.run()); box.addView(b); }
    public void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout box = new LinearLayout(this); box.setOrientation(1);
        output = new TextView(this);
        button(box, "Check network and storage", () -> new Thread(this::probe).start());
        button(box, "Pick a text file", () -> startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT).setType("text/plain").addCategory(Intent.CATEGORY_OPENABLE), 41));
        button(box, "Open host browser", () -> startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com/"))));
        button(box, "Record microphone (2 seconds)", () -> new Thread(this::record).start());
        button(box, "Request location", this::locate);
        box.addView(output); setContentView(box);
        new Thread(this::probe).start();
        String test = getIntent().getDataString();
        if ("arotest://record".equals(test)) new android.os.Handler(getMainLooper()).postDelayed(() -> new Thread(this::record).start(), 1500);
        if ("arotest://host".equals(test)) new android.os.Handler(getMainLooper()).postDelayed(() -> startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com/"))), 1500);
        if ("arotest://location".equals(test)) new android.os.Handler(getMainLooper()).postDelayed(this::locate, 1500);
        if ("arotest://isolation".equals(test)) new android.os.Handler(getMainLooper()).postDelayed(() -> new Thread(this::isolation).start(), 1500);
    }
    android.os.IBinder service(String name) throws Exception {
        return (android.os.IBinder)Class.forName("android.os.ServiceManager").getMethod("getService", String.class).invoke(null, name);
    }
    void isolation() {
        try {
            java.util.Set<Integer> pids = new java.util.HashSet<>();
            String[] names = {"audio", "media.audio_flinger", "media.audio_policy", "media.player", "connectivity", "mount", "clipboard", "location", "aro.media.provider", "aro.calendar.provider", "aro.settings.provider"};
            for (String name : names) {
                android.os.IBinder binder = service(name);
                android.os.Parcel data = android.os.Parcel.obtain(), reply = android.os.Parcel.obtain();
                try {
                    if (!binder.transact(0x5f504944, data, reply, 0)) throw new IOException("debug PID unsupported: " + name);
                    int pid = reply.readInt();
                    if (pid <= 0 || !pids.add(pid)) throw new IOException("services share a process: " + name);
                    report("WORKER " + name + " pid=" + pid + " descriptor=" + binder.getInterfaceDescriptor());
                } finally { data.recycle(); reply.recycle(); }
            }
            report("ISOLATION distinct_workers=" + pids.size());
            try (android.database.Cursor c = getContentResolver().query(Uri.parse("content://com.android.calendar/calendars"), new String[]{"_id"}, null, null, null)) {
                report("CALENDAR_PROVIDER readable=" + (c != null));
            }
            report("SETTINGS_PROVIDER readable=" + (android.provider.Settings.Global.getInt(getContentResolver(), "device_provisioned", 0) == 1));
            // Direct calls to the supervisor-only file picker control must be rejected.
            android.os.Parcel data = android.os.Parcel.obtain(), reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken("org.aro.IDocuments"); data.writeString(getPackageName()); data.writeString("text/plain");
                service("aro.documents").transact(1, data, reply, 0);
                report("DOCUMENT_CONTROL rejected=false");
            } catch (SecurityException | android.os.RemoteException expected) { report("DOCUMENT_CONTROL rejected=true"); }
            finally { data.recycle(); reply.recycle(); }
            // Forge only the package name: the real caller PID must prevent this
            // from reaching first-use consent, installation, or real geolocation.
            android.os.Binder callback = new android.os.Binder() {
                { attachInterface(null, "android.location.ILocationCallback"); }
                protected boolean onTransact(int code, android.os.Parcel data, android.os.Parcel reply, int flags) throws android.os.RemoteException {
                    if (code != 1) return super.onTransact(code, data, reply, flags);
                    data.enforceInterface("android.location.ILocationCallback");
                    report("LOCATION_INVALID_CALLER fix=" + (data.readInt() != 0));
                    return true;
                }
            };
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken("android.location.ILocationManager");
                data.writeString("network"); data.writeInt(0); data.writeStrongBinder(callback); data.writeString("org.aro.invalid");
                service("location").transact(2, data, reply, 0); reply.readException();
            } finally { data.recycle(); reply.recycle(); }
        } catch (Throwable e) { report("ISOLATION_FAIL=" + e); android.util.Log.e("AROHost", "isolation", e); }
    }
    void probe() {
        try {
            ConnectivityManager cm = getSystemService(ConnectivityManager.class);
            Network n = cm.getActiveNetwork();
            NetworkCapabilities c = cm.getNetworkCapabilities(n);
            report("NETWORK active=" + (n != null) + " validated=" + (c != null && c.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) + " metered=" + cm.isActiveNetworkMetered() + " count=" + cm.getAllNetworks().length);
            report("NETWORK_CAPS=" + c);
            javax.net.ssl.HttpsURLConnection con = (javax.net.ssl.HttpsURLConnection)new java.net.URL("https://example.com/").openConnection();
            con.setConnectTimeout(10000); con.setReadTimeout(10000);
            report("HTTPS status=" + con.getResponseCode()); con.disconnect();
        } catch (Throwable e) { report("NETWORK_FAIL=" + e); android.util.Log.e("AROHost", "network", e); }
        try {
            File f = new File(getFilesDir(), "m4-private.txt");
            try (FileOutputStream out = new FileOutputStream(f)) { out.write("ARO private storage".getBytes(java.nio.charset.StandardCharsets.UTF_8)); }
            try (FileInputStream in = new FileInputStream(f)) { report("PRIVATE_STORAGE=" + new String(in.readAllBytes(), java.nio.charset.StandardCharsets.UTF_8)); }
            report("SDCARD readable=" + android.os.Environment.getExternalStorageDirectory().canRead());
            File marker = new File(android.os.Environment.getExternalStorageDirectory(), "ARO-M4-test.txt");
            if (marker.isFile()) try (FileInputStream in = new FileInputStream(marker)) { report("SDCARD fixture=" + new String(in.readAllBytes(), java.nio.charset.StandardCharsets.UTF_8).equals("ARO file-chooser portal test\n")); }
        } catch (Throwable e) { report("STORAGE_FAIL=" + e); }
    }
    void record() {
        android.media.MediaRecorder rec = null;
        try {
            File f = new File(getFilesDir(), "m4-recording.m4a");
            rec = new android.media.MediaRecorder(this);
            rec.setAudioSource(android.media.MediaRecorder.AudioSource.MIC);
            rec.setOutputFormat(android.media.MediaRecorder.OutputFormat.MPEG_4);
            rec.setAudioEncoder(android.media.MediaRecorder.AudioEncoder.AAC);
            rec.setAudioSamplingRate(44100); rec.setAudioChannels(1); rec.setAudioEncodingBitRate(96000);
            rec.setOutputFile(f); rec.prepare(); rec.start();
            Thread.sleep(2000); rec.stop();
            report("RECORDING bytes=" + f.length());
        } catch (Throwable e) { report("RECORDING_FAIL=" + e); android.util.Log.e("AROHost", "recording", e); }
        finally { if (rec != null) rec.release(); }
    }
    void locate() {
        try {
            android.location.LocationManager lm = getSystemService(android.location.LocationManager.class);
            report("LOCATION requesting");
            lm.getCurrentLocation(android.location.LocationManager.NETWORK_PROVIDER, null, getMainExecutor(), loc -> report("LOCATION fix=" + (loc != null) + (loc == null ? "" : " accuracy=" + loc.getAccuracy())));
        } catch (Throwable e) { report("LOCATION_FAIL=" + e); android.util.Log.e("AROHost", "location", e); }
    }
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != 41) return;
        if (result != RESULT_OK || data == null) { report("PICKER canceled"); return; }
        report("PICKER type=" + getContentResolver().getType(data.getData()));
        try (android.database.Cursor cursor = getContentResolver().query(data.getData(), new String[]{"_display_name", "_size"}, null, null, null)) { report("PICKER metadata=" + (cursor != null && cursor.moveToFirst() && cursor.getLong(1) > 0)); }
        catch (Throwable e) { report("PICKER_METADATA_FAIL=" + e); }
        try (InputStream in = getContentResolver().openInputStream(data.getData())) {
            byte[] bytes = new byte[128]; int count = in.read(bytes);
            report("PICKER readable=" + (count >= 0));
        } catch (Throwable e) { report("PICKER_FAIL=" + e); }
    }
}
