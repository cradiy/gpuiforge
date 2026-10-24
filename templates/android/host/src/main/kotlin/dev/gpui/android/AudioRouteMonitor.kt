package dev.gpui.android

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaRoute2Info
import android.media.MediaRouter2
import android.media.RouteDiscoveryPreference
import android.os.Build
import android.os.Handler
import android.os.SystemClock
import android.provider.Settings
import org.json.JSONArray
import org.json.JSONObject

/** Session-owned observation of system media routes. Does not implement remote playback protocols. */
internal class AudioRouteMonitor(
    private val context: Context,
    private val handler: Handler,
    private val changed: (Long, String, String?) -> Unit
) : AutoCloseable {
    private val observers = mutableSetOf<Long>()
    private var native: Routes? = null
    private var closed = false

    fun subscribe(token: Long) {
        check(!closed)
        observers.add(token)
        if (Build.VERSION.SDK_INT >= 30 && native == null) {
            try {
                native = Routes(context) { error -> publish(error) }
            } catch (error: Exception) {
                android.util.Log.w("GPUI", "Could not observe audio routes", error)
                publish("Audio outputs are unavailable.")
                return
            }
        }
        publish(if (Build.VERSION.SDK_INT < 30) "Audio routing requires Android 11 or later." else null)
    }

    fun unsubscribe(token: Long) {
        observers.remove(token)
        if (observers.isEmpty()) {
            native?.close()
            native = null
            handler.removeCallbacksAndMessages(this)
        }
    }

    fun select(id: String) {
        check(!closed)
        val current = checkNotNull(native) { "Observe audio outputs before selecting a device" }
        current.select(id)
    }

    fun refresh() { publish(null) }

    private fun publish(error: String?) {
        // Native callbacks must not reenter Rust during an operation on GpuiSession.
        handler.postAtTime({
            if (!closed && observers.isNotEmpty()) {
                try {
                    val json = native?.snapshot() ?: JSONObject().put("devices", JSONArray())
                        .put("authorization_required", authorizationRequired(context)).toString()
                    observers.toList().forEach { if (it in observers) changed(it, json, error) }
                } catch (failure: Exception) {
                    android.util.Log.w("GPUI", "Could not read audio routes", failure)
                    observers.toList().forEach { if (it in observers) changed(it, "", "Audio outputs are unavailable.") }
                }
            }
        }, this, SystemClock.uptimeMillis())
    }

    override fun close() {
        if (closed) return
        closed = true
        handler.removeCallbacksAndMessages(this)
        observers.clear()
        native?.close()
        native = null
    }

    @androidx.annotation.RequiresApi(30)
    private class Routes(context: Context, private val changed: (String?) -> Unit) : AutoCloseable {
        private val router = MediaRouter2.getInstance(context)
        private val app = context.applicationContext
        private val routes = object : MediaRouter2.RouteCallback() {
            override fun onRoutesAdded(routes: List<MediaRoute2Info>) = changed(null)
            override fun onRoutesRemoved(routes: List<MediaRoute2Info>) = changed(null)
            override fun onRoutesChanged(routes: List<MediaRoute2Info>) = changed(null)
        }
        private val controller = object : MediaRouter2.ControllerCallback() {
            override fun onControllerUpdated(controller: MediaRouter2.RoutingController) = changed(null)
        }
        private val transfers = object : MediaRouter2.TransferCallback() {
            override fun onTransfer(old: MediaRouter2.RoutingController, new: MediaRouter2.RoutingController) = changed(null)
            override fun onTransferFailure(route: MediaRoute2Info) = changed("Could not switch to ${route.name}.")
        }
        init {
            try {
                router.registerRouteCallback(app.mainExecutor, routes,
                    RouteDiscoveryPreference.Builder(listOf(MediaRoute2Info.FEATURE_LIVE_AUDIO), false).build())
                router.registerControllerCallback(app.mainExecutor, controller)
                router.registerTransferCallback(app.mainExecutor, transfers)
            } catch (error: Exception) {
                close()
                throw error
            }
        }

        private fun available(): List<MediaRoute2Info> =
            (router.systemController.selectedRoutes + router.routes.filter { it.isSystemRoute }).distinctBy { it.id }

        fun snapshot(): String {
            val selected = router.systemController.selectedRoutes.map { it.id }.toSet()
            val devices = JSONArray()
            for (route in available().sortedByDescending { it.id in selected }) {
                devices.put(JSONObject().put("id", route.id).put("name", route.name.toString())
                    .put("kind", route.type).put("selected", route.id in selected))
            }
            return JSONObject().put("devices", devices)
                .put("authorization_required", authorizationRequired(app)).toString()
        }

        fun select(id: String) {
            val route = requireNotNull(available().firstOrNull { it.id == id }) { "Audio output is no longer available" }
            if (router.systemController.selectedRoutes.any { it.id == id }) return
            router.transferTo(route)
        }

        override fun close() {
            router.unregisterRouteCallback(routes)
            router.unregisterControllerCallback(controller)
            router.unregisterTransferCallback(transfers)
        }
    }

    companion object {
        fun authorizationRequired(context: Context): Boolean = Build.VERSION.SDK_INT >= 31 &&
            (context.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED ||
             context.checkSelfPermission(Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED)

        fun openSettings(context: Context) {
            if (Build.VERSION.SDK_INT >= 34 && MediaRouter2.getInstance(context).showSystemOutputSwitcher()) return
            context.startActivity(Intent(Settings.ACTION_BLUETOOTH_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }
}
