import { useCallback, useEffect, useRef, useState } from "react";

import {
  listAttachments,
  toCommandError,
  type AttachmentView,
  type CommandError,
} from "./backend";
import { onDocumentChanged } from "./documentEvents";

type AttachmentListState =
  | { status: "idle"; attachments: AttachmentView[]; error: null }
  | { status: "loading"; attachments: AttachmentView[]; error: null }
  | { status: "ready"; attachments: AttachmentView[]; error: null }
  | { status: "error"; attachments: AttachmentView[]; error: CommandError };

/**
 * Loads the document's attachments and keeps them current.
 *
 * Adding, deleting, undo and redo all announce a document change on the
 * in-window event bus; this hook refetches the list without dropping the
 * previous entries, so the panel does not flash while a refresh is in flight.
 * A stale response for a previous document is discarded.
 */
export function useAttachments(documentId: string | null): {
  attachments: AttachmentView[];
  status: AttachmentListState["status"];
  error: CommandError | null;
  reload: () => Promise<void>;
} {
  const [state, setState] = useState<AttachmentListState>({
    status: "idle",
    attachments: [],
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
      attachments: current.attachments,
      error: null,
    }));
    try {
      const attachments = await listAttachments(documentId);
      if (generation.current === request) {
        setState({ status: "ready", attachments, error: null });
      }
    } catch (error) {
      if (generation.current === request) {
        setState((current) => ({
          status: "error",
          attachments: current.attachments,
          error: toCommandError(error),
        }));
      }
    }
  }, [documentId]);

  useEffect(() => {
    generation.current += 1;
    setState({
      status: documentId ? "loading" : "idle",
      attachments: [],
      error: null,
    });
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
    attachments: state.attachments,
    status: state.status,
    error: state.error,
    reload: load,
  };
}
