import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// Release 签名：密钥与密码放在**仓库外**的 keystore.properties（已 gitignore）。
// 文件不存在时 release 包会保持未签名——Gradle 不会报错，但手机装不上，
// 所以 scripts/build-apk.sh 在打 release 前会先检查这个文件。
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}
val hasReleaseKey = keystoreProperties.getProperty("storeFile") != null

android {
    compileSdk = 37
    namespace = "com.syjun.pointra.mobile"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.syjun.pointra.mobile"
        // 目标设备：Android 8.0 (API 26) 及以上
        minSdk = 26
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (hasReleaseKey) {
            create("release") {
                storeFile = file(keystoreProperties.getProperty("storeFile"))
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            // 没配 keystore.properties 时保持未签名（仍能出包，但装不了）
            if (hasReleaseKey) {
                signingConfig = signingConfigs.getByName("release")
            }
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

// ---------------------------------------------------------------------------
// Windows 上 `tauri android build/dev` 会把 cargo 产物以「符号链接」放进
// jniLibs，而创建文件符号链接需要开发者模式或管理员权限。
// 当前机器两者都没有，于是改为：
//   scripts/build-apk.sh 用 ASCII 的 CARGO_TARGET_DIR 编译 .so → 拷贝到
//   app/src/main/jniLibs/<abi>/ → 直接跑 Gradle 打包。
// 因此这里关掉 Gradle 内置的 rust 构建任务（它只会重复触发符号链接并失败）。
// 若你在已开启开发者模式的机器上工作，可删掉这个 afterEvaluate 块，
// 恢复 `pnpm tauri android build` 的原生流程。
// ---------------------------------------------------------------------------
afterEvaluate {
    tasks.matching { it.name.startsWith("rustBuild") }.configureEach {
        enabled = false
    }
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
