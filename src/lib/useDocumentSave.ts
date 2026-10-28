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
import { onDocumentChanged } from "./documentEvents";

/** A save that failed and can be retried without retyping anything. */
export interface SaveFailure {
  error: CommandError;
  kind: SaveKind;
  request: SaveDocumentRequest;
}

/** A save waiting for the user to accept unencrypted output (ADR-0007). */
export interface SaveConfirmation {
  kind: SaveKind;
  request: SaveDocumentRequest;
}

/** Callbacks the workspace supplies to the save flow. */
export interface DocumentSaveOptions {
  /**
   * A save was refused because these required fields are still empty. Called
   * once per blocked attempt, in tab order.
   */
  onMissingRequired?: (fields: FormFieldView[]) => void;
}

/** Everything the save UI needs for the open document. */
export interface DocumentSave {
  /** The document differs from the last written revision. */
  dirty: boolean;
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
  /** Saves to the Output Document, falling back to Save As on the first save. */
  saveChanges: () => void;
  /** Chooses a target path and writes a new Output Document. */
  saveAs: () => void;
  /** Continues a pending save after accepting the unencrypted-output warning. */
  confirmUnencrypted: () => void;
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
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<SaveProgress | null>(null);
  const [error, setError] = useState<SaveFailure | null>(null);
  const [confirmation, setConfirmation] = useState<SaveConfirmation | null>(null);
  const [requiredFields, setRequiredFields] = useState<FormFieldView[] | null>(
    null,
  );
  const [lastSavedPath, setLastSavedPath] = useState<string | null>(null);

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

  // A new document starts with fresh save state.
  useEffect(() => {
    setHistory(null);
    setBusy(false);
    setProgress(null);
    setError(null);
    setConfirmation(null);
    setRequiredFields(null);
    setLastSavedPath(null);
    hasOutputRef.current = false;
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
    async (kind: SaveKind, request: SaveDocumentRequest) => {
      const current = documentRef.current;
      if (!current || busyRef.current) {
        return;
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
        setHistory(result.history);
        setLastSavedPath(result.outputPath);
        setRequiredFields(null);
      } catch (saveError) {
        setError({ error: toCommandError(saveError), kind, request });
      } finally {
        busyRef.current = false;
        setBusy(false);
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
    (kind: SaveKind, request: SaveDocumentRequest) => {
      const current = documentRef.current;
      if (!current) {
        return;
      }
      // ADR-0007: never write an unencrypted Output Document without an
      // explicit confirmation for this save.
      if (current.encrypted && !request.confirmUnencryptedOutput) {
        setConfirmation({ kind, request });
        return;
      }
      void runSave(kind, request);
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

  const confirmUnencrypted = useCallback(() => {
    if (!confirmation) {
      return;
    }
    const pending = confirmation;
    setConfirmation(null);
    void runSave(pending.kind, {
      ...pending.request,
      confirmUnencryptedOutput: true,
    });
  }, [confirmation, runSave]);

  const cancelConfirmation = useCallback(() => {
    setConfirmation(null);
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
    void runSave(failure.kind, failure.request);
  }, [error, runSave, saveAs]);

  const dismissError = useCallback(() => {
    setError(null);
  }, []);

  const dismissRequired = useCallback(() => {
    setRequiredFields(null);
  }, []);

  return {
    dirty: history?.dirty ?? false,
    canSaveChanges: !(document?.readOnly ?? false),
    busy,
    progress,
    error,
    confirmation,
    requiredFields,
    lastSavedPath,
    saveChanges,
    saveAs,
    confirmUnencrypted,
    cancelConfirmation,
    retry,
    dismissError,
    dismissRequired,
  };
}
