import { useCallback, useEffect, useState } from "react";

import {
  forgetRecentFile,
  listRecentFiles,
  type RecentFileView,
} from "./backend";

/** The locally persisted recent-files list and its mutations. */
export interface RecentFiles {
  /** Newest first; missing files have already been pruned by the backend. */
  files: RecentFileView[];
  /** Re-reads the list (after an open, save or restore). */
  refresh: () => Promise<void>;
  /** Removes a path, optimistically and then from the stored list. */
  forget: (path: string) => void;
}

/**
 * Loads the recent-files list for the start screen.
 *
 * Displaying the list never opens a document: the user picks an entry
 * explicitly. Loading is best-effort, so a missing IPC boundary or a corrupt
 * list leaves the start screen without recent files rather than failing it.
 */
export function useRecentFiles(enabled = true): RecentFiles {
  const [files, setFiles] = useState<RecentFileView[]>([]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    let cancelled = false;
    listRecentFiles()
      .then((found) => {
        // A degraded boundary can answer with a non-list; never let that
        // reach the render path.
        if (!cancelled && Array.isArray(found)) {
          setFiles(found);
        }
      })
      .catch(() => {
        // Recent files are a convenience; opening files still works.
      });
    return () => {
      cancelled = true;
    };
  }, [enabled]);

  const refresh = useCallback(async () => {
    try {
      const found = await listRecentFiles();
      if (Array.isArray(found)) {
        setFiles(found);
      }
    } catch {
      // Keep the previous list when the refresh cannot be answered.
    }
  }, []);

  const forget = useCallback((path: string) => {
    // A missing file should disappear immediately; the backend prunes the
    // persisted list in the background.
    setFiles((current) => current.filter((file) => file.path !== path));
    forgetRecentFile(path)
      .then((next) => {
        if (Array.isArray(next)) {
          setFiles(next);
        }
      })
      .catch(() => {
        // The in-memory removal still reflects what the user just learned.
      });
  }, []);

  return { files, refresh, forget };
}
