import { useEffect, useRef } from "react";
import { onBackButtonPress, exit } from "@tauri-apps/api/app";

/**
 * Android 返回键 / 全面屏向内滑手势的统一处理。
 *
 * ## 为什么要自己维护一个栈
 *
 * Tauri 的 `AppPlugin`（`tauri-2.12.3/mobile/android/.../AppPlugin.kt`）逻辑是：
 *
 * ```kotlin
 * if (!hasListener("back-button")) {
 *   if (webView.canGoBack()) webView.goBack() else activity.onBackPressed()  // → 退出
 * } else {
 *   trigger("back-button", { canGoBack })   // 完全交给 JS，不 goBack、不退出
 * }
 * ```
 *
 * 也就是说：**只要注册了 `onBackButtonPress`，返回键就由我们说了算**。
 *
 * 之前那版是「压一条 history + 监听 popstate」的写法，踩了一个真机上的坑：
 * 第一次向内滑把唯一那条占位历史消费掉后，`goBack()` 的 `popstate` 是**异步**派发的；
 * 用户快速连着滑第二次时，WebView 的指针已经回退、补压还没执行 →
 * `canGoBack()` 已经是 false → `activity.onBackPressed()` → **整个 App 被退掉**，
 * 而不是回到文章列表。所以现在不再借助 history，改成内存里的层栈，
 * 由"谁打开了覆盖层"自己登记，返回键从栈顶往下问。
 */

/** 当前打开的覆盖层（后进先出）。返回键从栈顶问起。 */
const backStack: Array<() => void> = [];

/**
 * 覆盖层打开时登记一个返回键处理器；关闭时自动摘掉。
 *
 * `onBack` 放 ref 里、依赖只留 `[active]`：调用方写内联函数也不会每次渲染都重登记。
 */
export function useBackGuard(active: boolean, onBack: () => void) {
  const cb = useRef(onBack);
  cb.current = onBack;

  useEffect(() => {
    if (!active) return;
    const handler = () => cb.current();
    backStack.push(handler);
    return () => {
      const i = backStack.lastIndexOf(handler);
      if (i >= 0) backStack.splice(i, 1);
    };
  }, [active]);
}

/**
 * 在根组件调用一次：把 Android 返回键接到 `backStack` 上。
 *
 * - 栈里有覆盖层 → 交给它处理（不退出 App）
 * - 栈空 → 说明已经在根页面（今日/阅读列表/…），按系统惯例退出 App
 */
export function useAndroidBackButton() {
  useEffect(() => {
    let unregister: (() => void) | undefined;
    let disposed = false;

    onBackButtonPress(() => {
      // 不用 `backStack.at(-1)`：tsconfig 的 lib 还没到 ES2022，`Array.prototype.at` 不存在。
      const top = backStack.length > 0 ? backStack[backStack.length - 1] : undefined;
      if (top) {
        top();
        return;
      }
      // 根页面：退出 App。AppPlugin 的 exit 走 `finishAffinity()`，
      // 是"正常关掉任务"而不是杀进程（比 history 方案干净）。
      void exit(0);
    })
      .then((listener) => {
        // StrictMode 下 effect 会立刻被卸载重挂：晚到的 listener 必须自己注销，
        // 否则会留下两个监听，一次返回键触发两遍。
        if (disposed) {
          void listener.unregister();
          return;
        }
        unregister = () => void listener.unregister();
      })
      .catch((e) => {
        // 权限没配（capabilities 缺 core:app:allow-register-listener）或非 Android 环境。
        // 这时返回键会退回 Tauri 默认行为：能回退就回退，否则退出 App。
        console.error("返回键接管失败，将使用系统默认行为：", e);
      });

    return () => {
      disposed = true;
      unregister?.();
    };
  }, []);
}
