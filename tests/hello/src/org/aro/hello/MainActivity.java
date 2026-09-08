package org.aro.hello;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;
import android.widget.TextView;

/** The smallest app that exercises the real Activity lifecycle. */
public class MainActivity extends Activity {
    private static final String TAG = "HelloARO";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Log.i(TAG, "onCreate");
        TextView tv = new TextView(this);
        tv.setText("Hello from ARO");
        tv.setTextSize(32);
        setContentView(tv);
    }

    @Override
    protected void onStart() {
        super.onStart();
        Log.i(TAG, "onStart");
    }

    @Override
    protected void onResume() {
        super.onResume();
        Log.i(TAG, "onResume");
    }

    @Override
    protected void onPause() {
        super.onPause();
        Log.i(TAG, "onPause");
    }
}
