// Replace: vault_app/android/app/src/main/kotlin/<your package path>/MainActivity.kt
// Keep the `package` line that is already in YOUR file if it differs.
package com.example.vault_app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.os.PersistableBundle
import android.view.WindowManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Blocks screenshots and screen recording of this app, and hides its
        // contents in the "recent apps" thumbnail.
        window.setFlags(
            WindowManager.LayoutParams.FLAG_SECURE,
            WindowManager.LayoutParams.FLAG_SECURE
        )
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "vault/clipboard")
            .setMethodCallHandler { call, result ->
                if (call.method == "copySensitive") {
                    val text = call.arguments as? String
                    if (text == null) {
                        result.error("bad_args", "text is required", null)
                        return@setMethodCallHandler
                    }
                    val clipboard =
                        getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    // Empty label so nothing like "password" is shown anywhere.
                    val clip = ClipData.newPlainText("", text)
                    if (Build.VERSION.SDK_INT >= 23) {
                        // Tells Android 13+ (and some keyboards) not to show
                        // or keep this content.
                        val extras = PersistableBundle()
                        extras.putBoolean("android.content.extra.IS_SENSITIVE", true)
                        clip.description.extras = extras
                    }
                    clipboard.setPrimaryClip(clip)
                    result.success(null)
                } else {
                    result.notImplemented()
                }
            }
    }
}