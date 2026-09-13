package org.aro.webview;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;
import android.webkit.ConsoleMessage;
import android.webkit.WebChromeClient;
import android.webkit.WebView;
import android.webkit.WebViewClient;

/** Local content separates provider/renderer failures from network failures. */
public class MainActivity extends Activity {
    private WebView web;
    private boolean checkedProviderService;

    private void checkProviderService() {
        if (checkedProviderService) return;
        checkedProviderService = true;
        android.content.Intent intent = new android.content.Intent().setComponent(
                new android.content.ComponentName("com.android.webview",
                    "org.chromium.android_webview.services.VariationsSeedServer"));
        boolean bound = bindService(intent, new android.content.ServiceConnection() {
            @Override public void onServiceConnected(android.content.ComponentName name, android.os.IBinder service) {
                Log.i("AROWebView", "PROVIDER_SERVICE_CONNECTED=" + name);
                unbindService(this);
                Log.i("AROWebView", "PROVIDER_SERVICE_UNBOUND");
            }
            @Override public void onServiceDisconnected(android.content.ComponentName name) {
                Log.i("AROWebView", "PROVIDER_SERVICE_DISCONNECTED=" + name);
            }
        }, BIND_AUTO_CREATE);
        Log.i("AROWebView", "PROVIDER_SERVICE_BIND=" + bound);
    }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        Log.i("AROWebView", "creating WebView");
        web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.setWebChromeClient(new WebChromeClient() {
            @Override public boolean onConsoleMessage(ConsoleMessage message) {
                Log.i("AROWebView", "console: " + message.message());
                return true;
            }
        });
        web.setWebViewClient(new WebViewClient() {
            @Override public void onReceivedError(WebView view, android.webkit.WebResourceRequest request,
                    android.webkit.WebResourceError error) {
                if (request.isForMainFrame()) Log.e("AROWebView", "PAGE_ERROR=" + error.getDescription());
            }
            @Override public void onPageFinished(WebView view, String url) {
                Log.i("AROWebView", "PAGE_FINISHED=" + url);
                Log.i("AROWebView", "renderer=" + (view.getWebViewRenderProcess() != null));
                view.postDelayed(() -> checkProviderService(), 2000);
                view.evaluateJavascript("document.title + ':' + (6 * 7)",
                        result -> Log.i("AROWebView", "JS_RESULT=" + result));
            }
        });
        android.widget.LinearLayout layout = new android.widget.LinearLayout(this);
        layout.setOrientation(android.widget.LinearLayout.VERTICAL);
        android.widget.EditText nativeField = new android.widget.EditText(this);
        nativeField.setSingleLine(true);
        nativeField.setHint("Native text field — Enter moves to the web form");
        nativeField.setTextSize(18);
        nativeField.addTextChangedListener(new android.text.TextWatcher() {
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            public void onTextChanged(CharSequence s, int start, int before, int count) {
                Log.i("AROWebView", "NATIVE_TEXT=" + s);
            }
            public void afterTextChanged(android.text.Editable text) {}
        });
        nativeField.setOnEditorActionListener((view, action, event) -> {
            if (event == null || event.getKeyCode() == android.view.KeyEvent.KEYCODE_ENTER) {
                web.requestFocus();
                web.evaluateJavascript("document.getElementById('entry').focus()", null);
                return true;
            }
            return false;
        });
        layout.addView(nativeField, new android.widget.LinearLayout.LayoutParams(-1, -2));
        layout.addView(web, new android.widget.LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(layout);
        nativeField.requestFocus();
        web.loadDataWithBaseURL("https://aro.invalid/",
                "<html><head><title>ARO WebView</title>"
                + "<meta name='viewport' content='width=device-width,initial-scale=1'></head>"
                + "<body onscroll='console.log(\"SCROLL_Y=\"+window.scrollY)' style='background:#142034;color:white;font:24px sans-serif;padding:36px;min-height:2400px'>"
                + "<h1>WebView on ARO</h1><p>Android web content in a native Linux window.</p>"
                + "<p><input id='entry' aria-label='Web text field' placeholder='Web text field' "
                + "style='font:24px sans-serif;width:90%' oninput='console.log(\"WEB_TEXT=\"+this.value)'></p>"
                + "<button style='font:24px sans-serif' onclick='this.textContent=\"JavaScript works\";"
                + "console.log(\"CLICK_OK\")'>Test interaction</button>"
                + "<p><button style='font:24px sans-serif' onclick='location.href=\"https://example.com/\"'>Load HTTPS page</button></p>"
                + "</body></html>",
                "text/html", "UTF-8", null);
    }

    @Override public void onDestroy() {
        if (web != null) web.destroy();
        super.onDestroy();
    }
}
