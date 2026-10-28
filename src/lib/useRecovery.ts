import { useCallback, useEffect, useRef, useState } from "react";

import {
  discardRecovery,
  listRecoveryCandidates,
  recoverySnapshot,
  restoreRecovery,
  toCommandError,
  type CommandError,
  type DocumentView,
  type RecoverySnapshotView,
} from "./backend";

/**
 * How often an open, dirty Document asks the backend for a throttled
 * Recovery File tick. The backend owns the debounce and maximum interval, so
 * this only needs to be frequent enough for those rules to be observable.
 */
export const RECOVERY_TICK_MS = 2_000;

/**
 * Requests a throttled Recovery File tick for the open Document.
 *
 * The backend decides whether a tick writes anything: it debounces revisions
 * and caps how long continuous editing may postpone a snapshot. This hook only
 * provides the clock. Ticks run while the Document is dirty; one immediate tick
 * accompanies every dirty-state change (including returning to the saved
 * revision), letting the backend remove a now-stale snapshot.
 *
 * A failed tick is ignored: recovery must never interrupt editing.
 */
export function useRecoverySnapshots(
  documentId: string | null,
  dirty: boolean,
  intervalMs: number = RECOVERY_TICK_MS,
): void {
  useEffect(() => {
    if (!documentId) {
      return;
    }
    // Serialization can outlast a tick; never queue overlapping requests.
    let inFlight = false;
    const tick = () => {
      if (inFlight) {
        return;
      }
      inFlight = true;
      recoverySnapshot(documentId)
        .catch(() => {
          // Recovery is an enhancement; editing continues without it.
        })
        .finally(() => {
          inFlight = false;
        });
    };
    tick();
    if (!dirty) {
      return;
    }
    const timer = window.setInterval(tick, intervalMs);
    return () => window.clearInterval(timer);
  }, [documentId, dirty, intervalMs]);
}

/** The start-up recovery prompt's state and actions. */
export interface RecoveryStartup {
  /** Recovery Files found when this window started, newest first. */
  candidates: RecoverySnapshotView[];
  /** The prompt is still waiting for the user's decision. */
  open: boolean;
  /** Session whose restore or discard is running, if any. */
  busySessionId: string | null;
  /** Why the last restore or discard failed. */
  error: CommandError | null;
  /** Opens the snapshot as the Document; null when the action failed. */
  restore: (sessionId: string) => Promise<DocumentView | null>;
  /** Deletes the snapshot without opening it. */
  discard: (sessionId: string) => void;
  /** Leaves the snapshots on disk for the next start ("稍后处理"). */
  dismiss: () => void;
}

/**
 * Finds Recovery Files left by an abnormal exit and offers to restore them.
 *
 * Startup only lists candidates: it never opens a Document. The list is read
 * once per window, and only the main window enables the hook — windows opened
 * for a specific `?open=` Document belong to that Document, not to recovery.
 */
export function useRecoveryStartup(enabled = true): RecoveryStartup {
  const [candidates, setCandidates] = useState<RecoverySnapshotView[]>([]);
  const [open, setOpen] = useState(false);
  const [busySessionId, setBusySessionId] = useState<string | null>(null);
  const [error, setError] = useState<CommandError | null>(null);
  const busyRef = useRef(false);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    let cancelled = false;
    void listRecoveryCandidates()
      .then((found) => {
        // A mocked or degraded IPC boundary can answer with a non-list; the
        // start-up prompt must never depend on that answer.
        if (cancelled || !Array.isArray(found) || found.length === 0) {
          return;
        }
        setCandidates(found);
        setOpen(true);
      })
      .catch(() => {
        // Recovery is best-effort. A failed scan must not block startup.
      });
    return () => {
      cancelled = true;
    };
  }, [enabled]);

  // Restoring or discarding the last candidate closes the prompt.
  useEffect(() => {
    if (open && candidates.length === 0) {
      setOpen(false);
    }
  }, [candidates.length, open]);

  const restore = useCallback(
    async (sessionId: string): Promise<DocumentView | null> => {
      if (busyRef.current) {
        return null;
      }
      busyRef.current = true;
      setBusySessionId(sessionId);
      setError(null);
      try {
        const document = await restoreRecovery(sessionId);
        setCandidates((current) =>
          current.filter((candidate) => candidate.sessionId !== sessionId),
        );
        return document;
      } catch (caught) {
        setError(toCommandError(caught));
        return null;
      } finally {
        busyRef.current = false;
        setBusySessionId(null);
      }
    },
    [],
  );

  const discard = useCallback((sessionId: string) => {
    if (busyRef.current) {
      return;
    }
    busyRef.current = true;
    setBusySessionId(sessionId);
    setError(null);
    discardRecovery(sessionId)
      .then(() => {
        setCandidates((current) =>
          current.filter((candidate) => candidate.sessionId !== sessionId),
        );
      })
      .catch((caught) => {
        setError(toCommandError(caught));
      })
      .finally(() => {
        busyRef.current = false;
        setBusySessionId(null);
      });
  }, []);

  const dismiss = useCallback(() => {
    if (busyRef.current) {
      return;
    }
    setOpen(false);
    setError(null);
  }, []);

  return {
    candidates,
    open,
    busySessionId,
    error,
    restore,
    discard,
    dismiss,
  };
}
