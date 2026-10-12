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
#   scripts/build-apk.sh [x86_64|aarch64|armv7|i686]   (默认 x86_64)
set -euo pipefail

ABI="${1:-x86_64}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# ASCII 目标目录（默认落在用户目录下，避免中文路径）
export CARGO_TARGET_DIR="${POINTRA_TARGET_DIR:-$HOME/.pointra-mobile-target}"

case "$ABI" in
  x86_64)  FLAVOR="X86_64" ;;
  aarch64) FLAVOR="Arm64" ;;
  armv7)   FLAVOR="Arm" ;;
  i686)    FLAVOR="X86" ;;
  *) echo "未知 ABI: $ABI" >&2; exit 1 ;;
esac

echo "==> 1/3 构建前端"
pnpm build

echo "==> 2/3 交叉编译 Rust（符号链接步骤会失败，属预期；.so 已产出）"
npx tauri android build --debug --target "$ABI" --apk || true

SO="$CARGO_TARGET_DIR/$ABI-linux-android/debug/libpointra_mobile.so"
if [[ ! -f "$SO" ]]; then
  echo "未找到 $SO，Rust 编译可能真的失败了" >&2
  exit 1
fi

DEST="src-tauri/gen/android/app/src/main/jniLibs/$ABI"
mkdir -p "$DEST"
cp "$SO" "$DEST/libpointra_mobile.so"

echo "==> 3/3 Gradle 打包"
cd src-tauri/gen/android
./gradlew ":app:assemble${FLAVOR}Debug" --console=plain

echo
echo "APK: $ROOT/src-tauri/gen/android/app/build/outputs/apk/$ABI/debug/app-$ABI-debug.apk"
