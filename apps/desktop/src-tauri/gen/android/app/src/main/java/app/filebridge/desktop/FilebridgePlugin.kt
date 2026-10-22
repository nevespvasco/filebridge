package app.filebridge.desktop

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.PowerManager
import android.provider.DocumentsContract
import android.provider.Settings
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class SendingArgs {
  var active: Boolean = false
  var title: String = ""
  var text: String = ""
  /** 0–100, or -1 when the share sent isn't known yet. */
  var progress: Int = -1
}

@InvokeArg
class SecretArgs {
  lateinit var key: String
  var value: String? = null
}

/**
 * What the Rust side of filebridge needs from Android (ADR 0014): a folder the user picks, as a
 * path; "All files access", so Sources are plain paths; the foreground service that keeps a send
 * alive; and secrets kept with an Android Keystore key.
 */
@TauriPlugin(
  permissions = [
    // Android 8–10: plain read access is enough for paths; 11+ uses "All files access".
    Permission(strings = [Manifest.permission.READ_EXTERNAL_STORAGE], alias = "storage")
  ]
)
class FilebridgePlugin(private val activity: Activity) : Plugin(activity) {
  private val vault by lazy { SecretVault(activity.applicationContext) }

  @Command
  fun pickFolder(invoke: Invoke) {
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
    startActivityForResult(invoke, intent, "folderPicked")
  }

  @ActivityCallback
  private fun folderPicked(invoke: Invoke, result: ActivityResult) {
    val res = JSObject()
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) {
      res.put("path", null)
      invoke.resolve(res)
      return
    }
    val path = treeUriToPath(uri)
    if (path == null) {
      invoke.reject("not_local", "pick a folder on the phone's storage or SD card")
      return
    }
    res.put("path", path)
    invoke.resolve(res)
  }

  @Command
  fun storageAccess(invoke: Invoke) {
    val res = JSObject()
    res.put("granted", hasStorageAccess())
    invoke.resolve(res)
  }

  @Command
  fun requestStorageAccess(invoke: Invoke) {
    if (hasStorageAccess()) {
      storageAccess(invoke)
      return
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      val intent = Intent(
        Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
        Uri.parse("package:${activity.packageName}")
      )
      startActivityForResult(invoke, intent, "storageAccessAnswered")
    } else {
      requestPermissionForAlias("storage", invoke, "storagePermissionAnswered")
    }
  }

  @ActivityCallback
  private fun storageAccessAnswered(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
    storageAccess(invoke)
  }

  @PermissionCallback
  private fun storagePermissionAnswered(invoke: Invoke) {
    storageAccess(invoke)
  }

  @SuppressLint("BatteryLife")
  @Command
  fun requestBatteryExemption(invoke: Invoke) {
    val power = activity.getSystemService(PowerManager::class.java)
    if (!power.isIgnoringBatteryOptimizations(activity.packageName)) {
      val intent = Intent(
        Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
        Uri.parse("package:${activity.packageName}")
      )
      activity.startActivity(intent)
    }
    invoke.resolve()
  }

  /** Starts, updates or stops the foreground service that keeps a send alive. */
  @Command
  fun sending(invoke: Invoke) {
    val args = invoke.parseArgs(SendingArgs::class.java)
    val context = activity.applicationContext
    val intent = Intent(context, SendService::class.java)
    if (args.active) {
      intent.putExtra(SendService.EXTRA_TITLE, args.title)
      intent.putExtra(SendService.EXTRA_TEXT, args.text)
      intent.putExtra(SendService.EXTRA_PROGRESS, args.progress)
      ContextCompat.startForegroundService(context, intent)
    } else {
      context.stopService(intent)
    }
    invoke.resolve()
  }

  @Command
  fun secretGet(invoke: Invoke) {
    val args = invoke.parseArgs(SecretArgs::class.java)
    try {
      val res = JSObject()
      res.put("value", vault.get(args.key))
      invoke.resolve(res)
    } catch (e: Exception) {
      invoke.reject("keystore: ${e.javaClass.simpleName}")
    }
  }

  @Command
  fun secretSet(invoke: Invoke) {
    val args = invoke.parseArgs(SecretArgs::class.java)
    try {
      vault.set(args.key, args.value ?: "")
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject("keystore: ${e.javaClass.simpleName}")
    }
  }

  @Command
  fun secretDelete(invoke: Invoke) {
    val args = invoke.parseArgs(SecretArgs::class.java)
    vault.delete(args.key)
    invoke.resolve()
  }

  private fun hasStorageAccess(): Boolean =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      Environment.isExternalStorageManager()
    } else {
      ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_EXTERNAL_STORAGE) ==
        PackageManager.PERMISSION_GRANTED
    }

  /**
   * The file system path of a folder from the system picker. Only the phone's storage and SD
   * cards have one: their tree ids are `primary:DCIM/Camera` or `1A2B-3C4D:Music`; Downloads may
   * give `raw:/storage/…`. Cloud providers have no path.
   */
  private fun treeUriToPath(uri: Uri): String? {
    val id = DocumentsContract.getTreeDocumentId(uri) ?: return null
    if (id.startsWith("raw:")) return id.removePrefix("raw:")
    if (uri.authority != "com.android.externalstorage.documents") return null
    val volume = id.substringBefore(':')
    val rest = id.substringAfter(':', "")
    val base = if (volume == "primary") {
      @Suppress("DEPRECATION")
      Environment.getExternalStorageDirectory().absolutePath
    } else {
      "/storage/$volume"
    }
    return if (rest.isEmpty()) base else "$base/$rest"
  }
}
