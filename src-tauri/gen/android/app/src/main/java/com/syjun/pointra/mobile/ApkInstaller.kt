package com.syjun.pointra.mobile

import android.app.Activity
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File

/**
 * 把已经下载到应用私有目录的 APK 交给系统安装器。
 *
 * 为什么不用 tauri-plugin-opener：它的 openPath/openUrl 在 Android 上会被 Tauri 的 ACL 拦
 * （`plugin:opener|open_url not allowed by ACL`）。这个方法由 Rust 侧通过 JNI 直接调用，
 * 不经过任何 ACL / 权限配置。
 *
 * 需要的两件东西在模板里已经具备：
 * - AndroidManifest 里已声明 `androidx.core.content.FileProvider`（authority `${applicationId}.fileprovider`）
 * - `file_paths.xml` 里已包含 `cache-path path="."`，所以缓存目录下的 APK 能被 content:// 暴露
 */
object ApkInstaller {
    @JvmStatic
    fun install(activity: Activity, path: String) {
        val file = File(path)
        require(file.exists()) { "APK 不存在: $path" }

        val uri = FileProvider.getUriForFile(
            activity,
            activity.packageName + ".fileprovider",
            file
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            // content:// 必须显式授予读权限，否则系统安装器读不到文件
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        activity.startActivity(intent)
    }
}
