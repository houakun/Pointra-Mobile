import { useEffect, useRef } from "react";

/**
 * 让 Android 返回键 / 全面屏边缘手势先「关掉当前覆盖页」，而不是直接退出整个 App。
 *
 * 原理：Tauri 的 Android 端在没有 `back-button` 监听时，返回键会走 WebView 的
 * history —— 能 `goBack()` 就 goBack，不能才调 Activity.onBackPressed 退出。
 * 所以覆盖页打开时压入一条历史，返回手势就会 goBack 并触发 `popstate`，
 * 我们在这里把覆盖页关掉；根页面没有历史可回，则按系统惯例退出 App。
 *
 * 注意：`onBack` 放在 ref 里，依赖只留 `[active]`。否则调用方写内联函数会导致
 * 每次渲染都重跑 effect、每次渲染都 pushState，历史瞬间堆爆。
 */
export function useBackGuard(active: boolean, onBack: () => void) {
  const cb = useRef(onBack);
  cb.current = onBack;

  useEffect(() => {
    if (!active) return;
    let pushed = false;
    try {
      window.history.pushState({ pointraOverlay: true }, "");
      pushed = true;
    } catch {
      // 个别 WebView 在自定义 scheme 下不允许 pushState：
      // 退化为「只能点页面上的返回按钮」，至少不会误退出 App。
    }
    const onPop = () => cb.current();
    window.addEventListener("popstate", onPop);
    return () => {
      window.removeEventListener("popstate", onPop);
      // 不是被返回手势关掉的（例如学习页走完队列自然结束）→ 把压入的那条历史弹掉，
      // 否则历史越堆越多，返回键要按很多次才有反应。
      const overlayState = window.history.state as { pointraOverlay?: boolean } | null;
      if (pushed && overlayState?.pointraOverlay) {
        window.history.back();
      }
    };
  }, [active]);
}
