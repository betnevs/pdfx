import { useCallback, useEffect, useRef, useState } from "react";

import {
  listAnnotations,
  toCommandError,
  type AnnotationView,
  type CommandError,
} from "./backend";
import { onDocumentChanged } from "./documentEvents";

type AnnotationListState =
  | { status: "idle"; annotations: AnnotationView[]; error: null }
  | { status: "loading"; annotations: AnnotationView[]; error: null }
  | { status: "ready"; annotations: AnnotationView[]; error: null }
  | { status: "error"; annotations: AnnotationView[]; error: CommandError };

/**
 * Loads the document's native annotations and keeps them current.
 *
 * Creating, restyling, deleting, undo and redo all announce a page change on
 * the in-window event bus; this hook refetches the list without dropping the
 * previous entries, so the panel does not flash while a refresh is in flight.
 * A stale response for a previous document is discarded.
 */
export function useAnnotations(documentId: string | null): {
  annotations: AnnotationView[];
  status: AnnotationListState["status"];
  error: CommandError | null;
  reload: () => Promise<void>;
} {
  const [state, setState] = useState<AnnotationListState>({
    status: "idle",
    annotations: [],
    error: null,
  });
  const generation = useRef(0);

  const load = useCallback(async () => {
    if (!documentId) {
      return;
    }
    const request = generation.current + 1;
    generation.current = request;
    setState((current) => ({
      status: "loading",
      annotations: current.annotations,
      error: null,
    }));
    try {
      const annotations = await listAnnotations(documentId);
      if (generation.current === request) {
        setState({ status: "ready", annotations, error: null });
      }
    } catch (error) {
      if (generation.current === request) {
        setState((current) => ({
          status: "error",
          annotations: current.annotations,
          error: toCommandError(error),
        }));
      }
    }
  }, [documentId]);

  useEffect(() => {
    generation.current += 1;
    setState({ status: documentId ? "loading" : "idle", annotations: [], error: null });
    if (documentId) {
      void load();
    }
    return () => {
      generation.current += 1;
    };
  }, [documentId, load]);

  useEffect(() => {
    if (!documentId) {
      return;
    }
    return onDocumentChanged((detail) => {
      if (detail.documentId === documentId) {
        void load();
      }
    });
  }, [documentId, load]);

  return {
    annotations: state.annotations,
    status: state.status,
    error: state.error,
    reload: load,
  };
}
