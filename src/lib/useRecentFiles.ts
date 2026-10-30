import { useCallback, useEffect, useRef, useState } from "react";

import { clearRecentFiles, forgetRecentFile, listRecentFiles, type RecentFileView } from "./backend";

export interface RecentFiles {
  files: RecentFileView[];
  busy: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  forget: (path: string) => void;
  clear: () => void;
}

/** Mutations affect only the local history index, never the documents. */
export function useRecentFiles(enabled = true): RecentFiles {
  const [files, setFiles] = useState<RecentFileView[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const mutating = useRef(false);

  const refresh = useCallback(async () => {
    const id = ++request.current;
    try {
      const found = await listRecentFiles();
      if (id === request.current && Array.isArray(found)) setFiles(found);
    } catch {
      // A history read failure does not prevent opening a document.
    }
  }, []);

  useEffect(() => {
    if (enabled) void refresh();
    return () => { request.current += 1; };
  }, [enabled, refresh]);

  const mutate = useCallback(async (operation: () => Promise<RecentFileView[]>) => {
    if (mutating.current) return;
    mutating.current = true;
    const id = ++request.current;
    setBusy(true);
    setError(null);
    try {
      const next = await operation();
      if (id === request.current && Array.isArray(next)) setFiles(next);
    } catch {
      setError("无法更新最近文件记录，请重试。");
    } finally {
      mutating.current = false;
      setBusy(false);
    }
  }, []);

  const forget = useCallback((path: string) => {
    void mutate(() => forgetRecentFile(path));
  }, [mutate]);
  const clear = useCallback(() => { void mutate(clearRecentFiles); }, [mutate]);

  return { files, busy, error, refresh, forget, clear };
}
