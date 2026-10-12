# Android 环境搭建清单（P0）

> 目标：让 `pnpm tauri android dev` 能把空白 App 跑进 Android 模拟器。
> 探测基线（2026-10-11，你的机器）：
> - ✅ JDK 17.0.1（`JAVA_HOME` **未设置**）
> - ✅ Rust 1.98.1 / cargo 1.98.1
> - ✅ Node v22.23.2
> - ❌ Android Studio / Android SDK / NDK / adb / platform-tools 全部缺失
> - ❌ `rustup target` 仅有 `x86_64-pc-windows-msvc`
>
> 预计耗时 30–60 分钟（含下载），磁盘占用 8–12 GB。

---

## 1. 安装 Android Studio

下载：<https://developer.android.com/studio>（Windows 64-bit installer，约 1.2 GB）

安装时勾选：
- ☑ Android Studio
- ☑ Android Virtual Device（**必须**，Q12 用模拟器）

首次启动选 **Custom** 安装类型，确认 SDK 路径为：

```
C:\Users\houakun\AppData\Local\Android\Sdk
```

（默认路径即可，后文用 `%LOCALAPPDATA%\Android\Sdk` 指代）

---

## 2. 通过 SDK Manager 安装组件

Android Studio → **Settings → Languages & Frameworks → Android SDK**：

### 2.1 SDK Platforms 页
- ☑ **Android 14.0 (API 34)** — 编译目标
- ☑ **Android 8.0 (API 26)** — 可选，仅用于本地核对最低版本行为

### 2.2 SDK Tools 页
- ☑ Android SDK Build-Tools **34.0.0**
- ☑ Android SDK Platform-Tools（提供 `adb`）
- ☑ Android SDK Command-line Tools (latest)（**必须**，Tauri CLI 依赖）
- ☑ **NDK (Side by side)** → 安装后在下方选 **26.1.10909125**（或 26.x / 27.x 任一稳定版）
- ☑ CMake（NDK 编译用）
- ☑ Android Emulator

> ⚠️ **NDK 版本必须记下来**，后面 `NDK_HOME` 要用完整版本号目录名。

---

## 3. 配置环境变量

Windows 设置 → 系统 → 系统信息 → 高级系统设置 → 环境变量 → **用户变量**（或系统变量）：

```
JAVA_HOME   = C:\Program Files\Java\jdk-17          （按你的实际 JDK 17 安装路径修改）
ANDROID_HOME= C:\Users\houakun\AppData\Local\Android\Sdk
NDK_HOME    = C:\Users\houakun\AppData\Local\Android\Sdk\ndk\26.1.10909125
```

在 `Path` 中追加：

```
%JAVA_HOME%\bin
%ANDROID_HOME%\platform-tools
%ANDROID_HOME%\cmdline-tools\latest\bin
%ANDROID_HOME%\emulator
```

> `JAVA_HOME` 目前为空，但 `java -version` 能跑，说明 JDK 在 `Path` 里。**必须显式设置 `JAVA_HOME`**，否则 Gradle 会报错。

设置完**重开终端**，验证：

```powershell
echo $env:JAVA_HOME
echo $env:ANDROID_HOME
echo $env:NDK_HOME
adb version
sdkmanager --version
java -version
emulator -list-avds
```

---

## 4. 添加 Rust 交叉编译 target

```powershell
rustup target add aarch64-linux-android
rustup target add armv7-linux-androideabi
rustup target add i686-linux-android
rustup target add x86_64-linux-android
```

验证：

```powershell
rustup target list --installed
```

预期看到 4 个 `*-linux-android`。
> **模拟器（Q12）用 `x86_64-linux-android`**（Windows + Intel/AMD CPU，构建最快）；
> `aarch64-linux-android` 留给以后装真机用。

---

## 5. 创建模拟器（AVD）

Android Studio → **Device Manager → Create Device**：

| 项 | 建议值 |
|---|---|
| 设备 | Pixel 6 |
| 系统镜像 | **API 34**，**x86_64**，Google APIs（`system-images;android-34;google_apis;x86_64`） |
| 名称 | `pointra_api34` |
| 内存 | 2 GB 以上 |
| 存储 | 4 GB 以上 |

命令行等价方式（可选）：

```powershell
sdkmanager "system-images;android-34;google_apis;x86_64"
avdmanager create avd -n pointra_api34 -k "system-images;android-34;google_apis;x86_64" -d pixel_6
emulator -avd pointra_api34
```

---

## 6. 检查 Windows 虚拟化（模拟器性能关键）

模拟器需要硬件加速，否则会慢到不可用。

1. **任务管理器 → 性能 → CPU**，确认「虚拟化: 已启用」
2. 若未启用 → 进 BIOS 打开 **Intel VT-x** / **AMD-V (SVM)**
3. **Windows 功能**里启用：
   - ☑ Windows 虚拟机监控程序平台（Windows Hypervisor Platform, WHPX）
   - ☑ 虚拟机平台（Virtual Machine Platform）

> 若同时装了 VMware/VirtualBox 且模拟器启动报错，属 WHPX/Hyper-V 冲突，需二选一。

---

## 7. 验证 P0 完成（用一个最小 Tauri 工程）

```powershell
# 在临时目录建最小工程
cd $env:TEMP
pnpm create tauri-app pointra-smoke --template react-ts --manager pnpm
cd pointra-smoke

pnpm install

# 初始化 Android 工程（会生成 src-tauri/gen/android）
pnpm tauri android init

# 启动模拟器并跑起来
pnpm tauri android dev
```

**P0 通过标准**：模拟器里出现空白 Tauri App 窗口（或 WebView 加载了 React 模板页）。

常见报错对照：

| 报错 | 原因 | 处理 |
|---|---|---|
| `Failed to find NDK` / `NDK_HOME is not set` | NDK_HOME 未设或版本号目录名写错 | 核对 `%ANDROID_HOME%\ndk\` 下的实际目录名 |
| `JAVA_HOME is not set` | 未设 | 见 §3 |
| `error: linker 'aarch64-linux-android-clang' not found` | target 装了但 NDK 没配好 | 检查 `NDK_HOME` |
| Gradle 卡在下载 | 网络问题 | 配置 Gradle 镜像，或耐心等待首次 10–30 分钟 |
| 模拟器启动黑屏 / 极慢 | 虚拟化未开 | 见 §6 |
| `cleartext HTTP not permitted`（后续阶段） | Android 默认禁明文 HTTP | 后续在 manifest 加 `usesCleartextTraffic` |

验证完成后可删除该临时工程（`$env:TEMP\pointra-smoke`）。

---

## 8. 与本项目的关系

P0 通过后：

| 阶段 | 内容 |
|---|---|
| P1 | PC 端生词库 + `Alt+S` 收藏（**不需要 Android 环境**，可先做） |
| P2 | PC 端 HTTP/WS 同步服务（**不需要 Android 环境**，可先做） |
| P3 | Android 端数据层：模拟器里访问 PC 用 `10.0.2.2:31415` |

> 建议：**立刻开始 P1，同时后台下载 Android 组件**。两者互不阻塞。

---

## 9. P0 回执（已核对通过）

```
JAVA_HOME    = D:\devople\Java\JDK
ANDROID_HOME = C:\Users\houakun\AppData\Local\Android\Sdk
NDK_HOME     = %LOCALAPPDATA%\Android\Sdk\ndk\26.1.10909125
adb          = 1.0.41 / 37.0.1
rustup targets = aarch64 / armv7 / i686 / x86_64-linux-android + x86_64-pc-windows-msvc
AVD          = pointra_api34（API 34，x86_64）
platforms    = android-34 / android-36 / android-37.0
build-tools  = 34.0.0 / 36.0.0 / 37.0.0
```

> 补充安装：AGP 9.3.1 实际要求 **compileSdk 37 / Build-Tools 37**（模板默认），
> 因此又装了 `platforms;android-37.0` 与 `build-tools;37.0.0`（Gradle 构建时还会
> 自动拉 `android-36` 与 `build-tools;36.0.0`）。

---

## 10. 本机 Windows 构建踩的坑（已解决）

工程路径含中文（`D:\智能体\pointra\...`），叠加 Windows 符号链接权限，共 5 个问题：

| # | 现象 | 处理 |
|---|---|---|
| 1 | NDK 链接器报 `cannot find version script D:\<乱码>...` / `unspecified system_category error` | cargo 产物放到**纯 ASCII** 目录：`CARGO_TARGET_DIR=~/.pointra-mobile-target` |
| 2 | Gradle 下载极慢（~57KB/s）、Maven 超时 | 换国内镜像：Gradle 用 `mirrors.cloud.tencent.com/gradle`，Maven 用 `maven.aliyun.com/repository/{google,public,gradle-plugin}` |
| 3 | AGP 报 `Your project path contains non-ASCII characters` | `gen/android/gradle.properties` 加 `android.overridePathCheck=true` |
| 4 | Kotlin 报 `Daemon compilation failed` / `different roots`（C: 与 D: 跨盘 + 中文路径） | 加 `kotlin.incremental=false`、`kotlin.compiler.execution.strategy=in-process`、`org.gradle.configuration-cache=false` |
| 5 | `tauri android build` 要把 `.so` 以**符号链接**放进 jniLibs，但本机无开发者模式/管理员权限 | 关闭 Gradle 里的 `rustBuild*` 任务，改为「cargo 产物手抄进 jniLibs + 直接调 Gradle」，封装为 **`Pointra-Mobile/scripts/build-apk.sh`** |

### 日常构建命令

```bash
cd Pointra-Mobile
bash scripts/build-apk.sh x86_64        # 产出 debug APK

# 安装并启动（模拟器）
export MSYS_NO_PATHCONV=1               # git bash 下必须，否则 Android 路径被改写
ADB="$ANDROID_HOME/platform-tools/adb.exe"
"$ADB" install -r -t 'D:/智能体/pointra/Pointra-Mobile/src-tauri/gen/android/app/build/outputs/apk/x86_64/debug/app-x86_64-debug.apk'
"$ADB" shell am start -n com.syjun.pointra.mobile/.MainActivity
```

> 若你的机器已开启 Windows **开发者模式**，可删掉 `gen/android/app/build.gradle.kts`
> 里的 `afterEvaluate { tasks.matching { it.name.startsWith("rustBuild") } ... }`，
> 恢复 `pnpm tauri android build --debug` 的原生流程。

---

## 11. 开发联调用的 PC 侧工具

`Pointra-Pub/src-tauri/examples/sync_dev_server.rs` 是一个**仅用于本机联调**的
headless 服务（不参与 App 打包）：

```bash
cd Pointra-Pub/src-tauri
cargo run --example sync_dev_server -- D:/pointra-dev-server 30000 50
# 打印：PC 端计数 / 监听端口 / 配对码

# 模拟「PC 刚刚收藏了一个词」（会走真实的 add_entry + broadcast_add 广播）
echo scout >> D:/pointra-dev-server/dev-add.txt
```

它让 P3–P6 的模拟器验证无需人工在 PC GUI 上操作；正式产品仍只用 App 自身的
`vocab_add_current` / `SyncManager`。
