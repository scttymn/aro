package org.aro.hello;

import android.app.Activity;
import android.graphics.Color;
import android.os.Bundle;
import android.util.Log;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.TextView;

/** Launched from MainActivity via startActivity — proves the intent dispatcher. */
public class SecondActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Log.i("HelloARO", "SecondActivity.onCreate");
        LinearLayout root = new LinearLayout(this);
        root.setBackgroundColor(0xFFF9A825);
        root.setGravity(Gravity.CENTER);
        TextView tv = new TextView(this);
        tv.setText("Second Activity\nstartActivity works");
        tv.setTextSize(44);
        tv.setTextColor(Color.BLACK);
        tv.setGravity(Gravity.CENTER);
        root.addView(tv);
        setContentView(root);
    }
    @Override protected void onResume() { super.onResume(); Log.i("HelloARO", "SecondActivity.onResume"); }
}
