#!/usr/bin/env bash
# 构建 Pointra Mobile 的 Android debug APK。
#
# 为什么不用 `pnpm tauri android build` 一把梭：
#   1) 本机工程路径含中文（D:\智能体\...），Android NDK 的链接器会读不了
#      中文路径下的 .o / version-script，所以必须把 cargo 产物放到纯 ASCII 目录；
#   2) tauri CLI 随后要把 .so **以符号链接**放进 jniLibs，而 Windows 创建文件
#      符号链接需要开发者模式或管理员权限，本机两者都没有。
# 因此这里拆成三步：cargo 产物 → 手抄进 jniLibs → 直接调 Gradle 打包。
# 在已开启开发者模式的机器上，直接 `pnpm tauri android build --debug` 即可。
#
# 用法：
#   scripts/build-apk.sh [x86_64|aarch64|armv7|i686]
#     x86_64  → 模拟器（默认）
#     aarch64 → 真机（几乎全部 Android 手机）
set -euo pipefail

ABI="${1:-x86_64}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# ASCII 目标目录（默认落在用户目录下，避免中文路径）
export CARGO_TARGET_DIR="${POINTRA_TARGET_DIR:-$HOME/.pointra-mobile-target}"

# 【易错点】jniLibs 的子目录名是 **Android ABI 名**，不是 Rust 三 元组名：
#   写成 jniLibs/aarch64/ 会被 AGP 拒绝：
#   "out extracted from path ...\jniLibs\aarch64\libpointra_mobile.so is not an ABI"
# Flavor 名则来自 tauri 的 archList（arm64/arm/x86/x86_64）。
case "$ABI" in
  x86_64)  FLAVOR="X86_64"; JNI_DIR="x86_64" ;;
  aarch64) FLAVOR="Arm64";  JNI_DIR="arm64-v8a" ;;
  armv7)   FLAVOR="Arm";    JNI_DIR="armeabi-v7a" ;;
  i686)    FLAVOR="X86";    JNI_DIR="x86" ;;
  *) echo "未知 ABI: $ABI" >&2; exit 1 ;;
esac
FLAVOR_LC="$(echo "$FLAVOR" | tr '[:upper:]' '[:lower:]')"

echo "==> 1/3 构建前端"
pnpm build

echo "==> 2/3 交叉编译 Rust（符号链接步骤会失败，属预期；.so 已产出）"
npx tauri android build --debug --target "$ABI" --apk || true

SO="$CARGO_TARGET_DIR/$ABI-linux-android/debug/libpointra_mobile.so"
if [[ ! -f "$SO" ]]; then
  echo "未找到 $SO，Rust 编译可能真的失败了" >&2
  exit 1
fi

JNI_ROOT="src-tauri/gen/android/app/src/main/jniLibs"
DEST="$JNI_ROOT/$JNI_DIR"
mkdir -p "$DEST"
cp "$SO" "$DEST/libpointra_mobile.so"

# jniLibs 下若残留 ABI 名不合法的目录（例如手误建的 aarch64/），AGP 会直接报错，
# 这里提示一下，由人确认后手动清理（脚本不替用户批量删目录）。
for d in "$JNI_ROOT"/*/; do
  name="$(basename "$d")"
  case "$name" in
    arm64-v8a|armeabi-v7a|x86|x86_64) ;;
    *) echo "⚠ 发现非 ABI 目录：$JNI_ROOT/$name —— 会让 Gradle 报 'is not an ABI'，请手动清理" >&2 ;;
  esac
done

echo "==> 3/3 Gradle 打包"
cd src-tauri/gen/android
./gradlew ":app:assemble${FLAVOR}Debug" --console=plain

APK="$ROOT/src-tauri/gen/android/app/build/outputs/apk/$FLAVOR_LC/debug/app-$FLAVOR_LC-debug.apk"
echo
if [[ -f "$APK" ]]; then
  echo "APK: $APK ($(( $(stat -c%s "$APK") / 1048576 )) MB)"
else
  echo "APK 未在预期位置：$APK" >&2
  find "$ROOT/src-tauri/gen/android/app/build/outputs/apk" -name "*.apk" >&2
  exit 1
fi
