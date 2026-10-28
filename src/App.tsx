import { useEffect, useState } from "react";

import { getAppInfo, type AppInfo } from "./lib/backend";

type AppInfoState =
  | { status: "loading" }
  | { status: "ready"; info: AppInfo }
  | { status: "unavailable" };

function statusText(state: AppInfoState): string {
  switch (state.status) {
    case "loading":
      return "正在载入…";
    case "ready":
      return `${state.info.name} ${state.info.version}`;
    case "unavailable":
      return "无法获取应用信息";
  }
}

export function App() {
  const [appInfo, setAppInfo] = useState<AppInfoState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    getAppInfo()
      .then((info) => {
        if (!cancelled) {
          setAppInfo({ status: "ready", info });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setAppInfo({ status: "unavailable" });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            P
          </span>
          <h1>PDFX</h1>
        </div>
        <span className="app-mode">本地 PDF 编辑器</span>
      </header>

      <main className="workspace" aria-label="PDFX 工作区">
        <section className="empty-workspace">
          <h2>空工作区</h2>
          <p>打开一个 PDF 文件开始编辑。文件只在本机处理。</p>
        </section>
      </main>

      <footer className="status-bar">
        <span role="status">{statusText(appInfo)}</span>
        <span>离线 · 无遥测</span>
      </footer>
    </div>
  );
}
