package org.aro.hello;

import android.app.Activity;
import android.content.Context;
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
                Log.i(TAG, "click " + taps);
                root.setBackgroundColor(colors[taps % colors.length]);
                tv.setText("Tapped " + taps);
            }
        });
        setContentView(root);
    }

    @Override protected void onStart() { super.onStart(); Log.i(TAG, "onStart"); }
    @Override protected void onResume() { super.onResume(); Log.i(TAG, "onResume"); }
}
