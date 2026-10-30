import { useCallback, useEffect, useRef, useState } from "react";

import {
  chooseSaveFile,
  editHistory,
  missingRequiredFields,
  onSaveProgress,
  saveDocument,
  toCommandError,
  type CommandError,
  type DocumentView,
  type EditHistoryView,
  type FormFieldView,
  type SaveDocumentRequest,
  type SaveKind,
  type SaveProgress,
  type SaveResultView,
} from "./backend";
import { flushPendingEdits, hasDirtyPendingEdits, PENDING_EDIT_CHANGED } from "./pendingEdits";
import { onDocumentChanged } from "./documentEvents";

/** A save that failed and can be retried. */
export interface SaveFailure {
  error: CommandError;
  kind: SaveKind;
  /** The original request with any opening password removed. */
  request: SaveDocumentRequest;
  /**
   * The failed request carried an opening password. Retrying must ask for it
   * again instead of keeping the secret in memory.
   */
  usedPassword: boolean;
}

/** A save waiting for the user's opening-password or unencrypted choice. */
export interface SaveConfirmation {
  kind: SaveKind;
  /** The request without an opening password; the dialog supplies it. */
  request: SaveDocumentRequest;
  /** Saving without a password would remove the current output's protection. */
  outputEncrypted: boolean;
  /** Re-open with the opening-password fields already shown (retry). */
  preferPassword: boolean;
}

/**
 * How a save that gates closing the window ended.
 *
 * Only `"saved"` means the Output Document is on disk; every other outcome
 * must leave the window open. `"cancelled"` covers a dismissed Save As dialog
 * or unencrypted-output confirmation, `"blocked"` a missing required field,
 * and `"failed"` a write or dialog error.
 */
export type SaveForCloseResult = "saved" | "cancelled" | "failed" | "blocked";

/** The outcome of one actual write attempt. */
type SaveOutcome = "saved" | "failed";

/** Callbacks the workspace supplies to the save flow. */
export interface DocumentSaveOptions {
  /**
   * A save was refused because these required fields are still empty. Called
   * once per blocked attempt, in tab order.
   */
  onMissingRequired?: (fields: FormFieldView[]) => void;
  /**
   * A save finished writing its Output Document. The workspace refreshes the
   * recent-files list from this notification.
   */
  onSaved?: (outputPath: string) => void;
}

/** Everything the save UI needs for the open document. */
export interface DocumentSave {
  /** The document differs from the last written revision. */
  dirty: boolean;
  /** The Edit Session can undo (and the document is editable). */
  canUndo: boolean;
  /** The Edit Session can redo (and the document is editable). */
  canRedo: boolean;
  /** Save Changes is unavailable for a Permission-Restricted document. */
  canSaveChanges: boolean;
  busy: boolean;
  progress: SaveProgress | null;
  error: SaveFailure | null;
  confirmation: SaveConfirmation | null;
  /** Required fields that blocked the latest save attempt, in tab order. */
  requiredFields: FormFieldView[] | null;
  /** Where the document was last written in this session. */
  lastSavedPath: string | null;
  /**
   * The latest successful save wrote a document whose actual signatures are
   * now invalid. The signature warning switches to its invalidated state.
   */
  signatureInvalidated: boolean;
  /**
   * Increments after every successful save. The workspace uses it to re-show a
   * dismissed signature warning once a new save has invalidated signatures.
   */
  saveGeneration: number;
  /** Saves to the Output Document, falling back to Save As on the first save. */
  saveChanges: () => void;
  /** Chooses a target path and writes a new Output Document. */
  saveAs: () => void;
  /**
   * Runs the normal save flow for a window close and resolves how it ended.
   * The window may close only when the result is `"saved"`.
   */
  saveForClose: () => Promise<SaveForCloseResult>;
  /** Resolves once no save is running (immediately when idle). */
  waitForIdle: () => Promise<void>;
  /**
   * Adopts the history returned by an undo/redo command, so the dirty marker
   * and the native menu availability change without another backend read.
   */
  acceptHistory: (history: EditHistoryView) => void;
  /** Continues a pending save with the chosen opening password (null = none). */
  confirmSaveOptions: (password: string | null) => void;
  cancelConfirmation: () => void;
  retry: () => void;
  dismissError: () => void;
  dismissRequired: () => void;
}

/**
 * Owns the save flow for one open Document.
 *
 * Save As picks a path through the native dialog; Save Changes overwrites the
 * Output Document chosen by the first successful save. The hook tracks dirty
 * state from the backend's edit history, so saving leaves undo/redo intact and
 * undoing past the saved revision marks the document dirty again.
 */
export function useDocumentSave(
  document: DocumentView | null,
  options: DocumentSaveOptions = {},
): DocumentSave {
  const documentId = document?.documentId ?? null;
  const [pendingDirty, setPendingDirty] = useState(false);
  useEffect(() => {
    const update = () => setPendingDirty(documentId ? hasDirtyPendingEdits(documentId) : false);
    update(); window.addEventListener(PENDING_EDIT_CHANGED, update);
    return () => window.removeEventListener(PENDING_EDIT_CHANGED, update);
  }, [documentId]);
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<SaveProgress | null>(null);
  const [error, setError] = useState<SaveFailure | null>(null);
  const [confirmation, setConfirmation] = useState<SaveConfirmation | null>(null);
  const [requiredFields, setRequiredFields] = useState<FormFieldView[] | null>(
    null,
  );
  const [lastSavedPath, setLastSavedPath] = useState<string | null>(null);
  const [signatureInvalidated, setSignatureInvalidated] = useState(false);
  const [saveGeneration, setSaveGeneration] = useState(0);

  const documentRef = useRef(document);
  documentRef.current = document;
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const requiredRef = useRef(requiredFields);
  requiredRef.current = requiredFields;
  const busyRef = useRef(false);
  // The backend remembers the Output Document; this mirrors whether a Save As
  // has happened in this window so Save Changes can fall back to Save As.
  const hasOutputRef = useRef(false);
  // Whether the last written Output Document requires an opening password.
  // Save Changes must not replace it with an unencrypted file without asking.
  const outputEncryptedRef = useRef(false);
  // Resolves the promise returned by `startSave` once the user answers an
  // unencrypted-output confirmation. Only one confirmation can be pending.
  const confirmationResolverRef = useRef<
    ((result: SaveForCloseResult) => void) | null
  >(null);
  // Close requests wait here while a save is already in flight, so the fresh
  // history read happens after the backend has settled the write.
  const idleWaitersRef = useRef<Array<() => void>>([]);

  // A new document starts with fresh save state.
  useEffect(() => {
    const pendingConfirmation = confirmationResolverRef.current;
    confirmationResolverRef.current = null;
    pendingConfirmation?.("cancelled");
    setHistory(null);
    setBusy(false);
    setProgress(null);
    setError(null);
    setConfirmation(null);
    setRequiredFields(null);
    setLastSavedPath(documentRef.current?.outputPath ?? null);
    setSignatureInvalidated(false);
    setSaveGeneration(0);
    // A restored Document may already have an Output Document from before the
    // crash; Save Changes overwrites it instead of forcing another Save As.
    hasOutputRef.current = Boolean(documentRef.current?.outputPath);
    // A restored protected output must not be silently replaced either; the
    // backend detects it from the file because the password is never stored.
    outputEncryptedRef.current = Boolean(documentRef.current?.outputEncrypted);
    busyRef.current = false;

    if (!documentId) {
      return;
    }
    let cancelled = false;
    editHistory(documentId)
      .then((next) => {
        if (!cancelled) {
          setHistory(next);
        }
      })
      .catch(() => {
        // Dirty tracking is an enhancement; a missing history call must not
        // block opening or saving.
      });
    return () => {
      cancelled = true;
    };
  }, [documentId]);

  // Every edit, undo or redo announces itself; re-read the clean baseline.
  useEffect(() => {
    if (!documentId) {
      return;
    }
    let cancelled = false;
    return onDocumentChanged((detail) => {
      if (detail.documentId !== documentId) {
        return;
      }
      editHistory(documentId)
        .then((next) => {
          if (!cancelled) {
            setHistory(next);
          }
        })
        .catch(() => {});
      // While a required-field warning is visible, keep it in step with the
      // fields the user is filling.
      if (requiredRef.current) {
        missingRequiredFields(documentId)
          .then((missing) => {
            if (!cancelled) {
              setRequiredFields(missing.length > 0 ? missing : null);
            }
          })
          .catch(() => {});
      }
    });
  }, [documentId]);

  useEffect(() => {
    if (!documentId) {
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    onSaveProgress((update) => {
      if (update.documentId === documentId) {
        setProgress(update);
      }
    })
      .then((dispose) => {
        if (disposed) {
          dispose();
        } else {
          unlisten = dispose;
        }
      })
      .catch(() => {
        // Outside the Tauri shell there is no event bridge.
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [documentId]);

  const runSave = useCallback(
    async (kind: SaveKind, request: SaveDocumentRequest): Promise<SaveOutcome> => {
      const current = documentRef.current;
      if (!current || busyRef.current) {
        return "failed";
      }
      busyRef.current = true;
      setBusy(true);
      setProgress(null);
      setError(null);
      try {
        const result: SaveResultView = await saveDocument(
          current.documentId,
          request,
        );
        hasOutputRef.current = true;
        outputEncryptedRef.current = result.encrypted;
        setHistory(result.history);
        setLastSavedPath(result.outputPath);
        setRequiredFields(null);
        setSignatureInvalidated(Boolean(result.signatureInvalidated));
        setSaveGeneration((generation) => generation + 1);
        optionsRef.current.onSaved?.(result.outputPath);
        return "saved";
      } catch (saveError) {
        // The password is never part of retryable state: copy only the
        // non-secret fields so a failed save cannot leak it into the UI or a
        // later request. Retrying re-opens the dialog when one was used.
        const safeRequest: SaveDocumentRequest = {
          saveAsPath: request.saveAsPath,
          confirmUnencryptedOutput: request.confirmUnencryptedOutput,
        };
        setError({
          error: toCommandError(saveError),
          kind,
          request: safeRequest,
          usedPassword: request.password !== undefined,
        });
        return "failed";
      } finally {
        busyRef.current = false;
        setBusy(false);
        const waiters = idleWaitersRef.current;
        idleWaitersRef.current = [];
        for (const resolve of waiters) {
          resolve();
        }
      }
    },
    [],
  );

  /**
   * Required-field check run before every save attempt (ticket 30).
   *
   * The backend is the source of truth for which fields are mandatory and
   * which are still empty. A failure to read them must not block saving, so
   * only a successful, non-empty answer refuses the attempt.
   */
  const checkRequired = useCallback(async (): Promise<boolean> => {
    const current = documentRef.current;
    if (!current) {
      return false;
    }
    if (!(await flushPendingEdits(current.documentId))) return false;
    try {
      const missing = await missingRequiredFields(current.documentId);
      if (missing.length > 0) {
        setRequiredFields(missing);
        optionsRef.current.onMissingRequired?.(missing);
        return false;
      }
    } catch {
      // Validation is best-effort; saving is still possible without it.
    }
    setRequiredFields(null);
    return true;
  }, []);

  const startSave = useCallback(
    (
      kind: SaveKind,
      request: SaveDocumentRequest,
      preferPassword = false,
    ): Promise<SaveForCloseResult> => {
      const current = documentRef.current;
      if (!current) {
        return Promise.resolve("cancelled");
      }
      // Save As always offers the opening-password option. Save Changes shows
      // the dialog only when the write needs a decision: an encrypted Source
      // Document (ADR-0007), a protected Output Document whose password would
      // otherwise be silently dropped, or a document whose signature
      // structures would be invalidated by writing it.
      const needsDecision =
        current.encrypted ||
        outputEncryptedRef.current ||
        Boolean(current.signature);
      if (kind === "save_as" || needsDecision) {
        // A newer attempt supersedes an older, unanswered dialog.
        const previous = confirmationResolverRef.current;
        previous?.("cancelled");
        return new Promise((resolve) => {
          confirmationResolverRef.current = resolve;
          setConfirmation({
            kind,
            request,
            outputEncrypted: outputEncryptedRef.current,
            preferPassword,
          });
        });
      }
      return runSave(kind, request);
    },
    [runSave],
  );

  const saveAs = useCallback(() => {
    const current = documentRef.current;
    if (!current || busyRef.current) {
      return;
    }
    void (async () => {
      // Refuse before choosing a target so an incomplete form never produces
      // a pointless file dialog.
      if (!(await checkRequired())) {
        return;
      }
      let path: string | null;
      try {
        path = await chooseSaveFile(current.name);
      } catch (dialogError) {
        console.warn("[pdfx] could not show the Save As dialog", dialogError);
        return;
      }
      if (!path) {
        return;
      }
      startSave("save_as", {
        saveAsPath: path,
        confirmUnencryptedOutput: false,
      });
    })();
  }, [checkRequired, startSave]);

  const saveChanges = useCallback(() => {
    const current = documentRef.current;
    if (!current || busyRef.current || current.readOnly) {
      return;
    }
    void (async () => {
      if (!(await checkRequired())) {
        return;
      }
      if (!hasOutputRef.current) {
        // The first save is always Save As (the backend has no Output Document).
        saveAs();
        return;
      }
      startSave("save_changes", {
        saveAsPath: null,
        confirmUnencryptedOutput: false,
      });
    })();
  }, [checkRequired, saveAs, startSave]);

  /**
   * The close guard's save path.
   *
   * It is the same flow as the Save buttons — required fields, Save As when no
   * Output Document exists, the unencrypted-output confirmation and the
   * retryable write error — but it resolves an outcome so the caller can keep
   * the window open unless the document is actually on disk.
   */
  const saveForClose = useCallback(async (): Promise<SaveForCloseResult> => {
    const current = documentRef.current;
    if (!current || busyRef.current) {
      return "failed";
    }
    if (!(await checkRequired())) {
      return "blocked";
    }
    // A confirmation left over from an earlier save attempt must not swallow
    // this close: answer it as cancelled and let this save raise its own.
    if (confirmationResolverRef.current) {
      const previous = confirmationResolverRef.current;
      confirmationResolverRef.current = null;
      setConfirmation(null);
      previous("cancelled");
    }
    // A read-only document cannot Save Changes, but Save As still works.
    if (current.readOnly || !hasOutputRef.current) {
      let path: string | null;
      try {
        path = await chooseSaveFile(current.name);
      } catch (dialogError) {
        console.warn("[pdfx] could not show the Save As dialog", dialogError);
        return "failed";
      }
      if (!path) {
        return "cancelled";
      }
      return startSave("save_as", {
        saveAsPath: path,
        confirmUnencryptedOutput: false,
      });
    }
    return startSave("save_changes", {
      saveAsPath: null,
      confirmUnencryptedOutput: false,
    });
  }, [checkRequired, startSave]);

  const waitForIdle = useCallback((): Promise<void> => {
    if (!busyRef.current) {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      idleWaitersRef.current.push(resolve);
    });
  }, []);

  const confirmSaveOptions = useCallback(
    (password: string | null) => {
      if (!confirmation) {
        return;
      }
      const pending = confirmation;
      const resolve = confirmationResolverRef.current;
      confirmationResolverRef.current = null;
      setConfirmation(null);
      const request: SaveDocumentRequest = {
        saveAsPath: pending.request.saveAsPath,
        confirmUnencryptedOutput: pending.request.confirmUnencryptedOutput,
      };
      if (password !== null && password.length > 0) {
        // The secret travels only in this in-flight request; the dialog has
        // already cleared its fields and no retryable state keeps it.
        request.password = password;
      } else if (documentRef.current?.encrypted || pending.outputEncrypted) {
        // Clicking the explicit unencrypted action is the ADR-0007 acceptance.
        request.confirmUnencryptedOutput = true;
      }
      if (documentRef.current?.signature) {
        // The dialog showed the signature warning and the user chose the
        // explicit save action; the confirmation is per-save and not retained.
        request.confirmSignatureInvalidation = true;
      }
      void runSave(pending.kind, request).then((outcome) => resolve?.(outcome));
    },
    [confirmation, runSave],
  );

  const cancelConfirmation = useCallback(() => {
    const resolve = confirmationResolverRef.current;
    confirmationResolverRef.current = null;
    setConfirmation(null);
    resolve?.("cancelled");
  }, []);

  const retry = useCallback(() => {
    if (!error) {
      return;
    }
    const failure = error;
    setError(null);
    if (
      failure.error.code === "save_target_is_source" ||
      failure.error.code === "no_output_document"
    ) {
      // The target itself was wrong; choose a new one instead of repeating it.
      saveAs();
      return;
    }
    if (failure.usedPassword) {
      // The password was intentionally not retained: ask for it again with
      // the password option already selected, but with empty fields.
      void startSave(failure.kind, failure.request, true);
      return;
    }
    if (documentRef.current?.signature) {
      // The confirmation is never retained either: every retry is a new save
      // attempt, so it goes back through the dialog and asks again.
      void startSave(failure.kind, failure.request, false);
      return;
    }
    void runSave(failure.kind, failure.request);
  }, [error, runSave, saveAs, startSave]);

  const dismissError = useCallback(() => {
    setError(null);
  }, []);

  const dismissRequired = useCallback(() => {
    setRequiredFields(null);
  }, []);

  const acceptHistory = useCallback((next: EditHistoryView) => {
    setHistory(next);
  }, []);

  return {
    dirty: pendingDirty || (history?.dirty ?? false),
    canUndo: (pendingDirty || (history?.canUndo ?? false)) && !(document?.readOnly ?? false),
    canRedo: !pendingDirty && (history?.canRedo ?? false) && !(document?.readOnly ?? false),
    canSaveChanges: !(document?.readOnly ?? false),
    busy,
    progress,
    error,
    confirmation,
    requiredFields,
    lastSavedPath,
    signatureInvalidated,
    saveGeneration,
    saveChanges,
    saveAs,
    saveForClose,
    waitForIdle,
    acceptHistory,
    confirmSaveOptions,
    cancelConfirmation,
    retry,
    dismissError,
    dismissRequired,
  };
}
