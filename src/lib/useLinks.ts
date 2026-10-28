import { useCallback, useEffect, useRef, useState } from "react";

import {
  listLinks,
  toCommandError,
  type CommandError,
  type LinkView,
} from "./backend";
import { onDocumentChanged } from "./documentEvents";

type LinkListState =
  | { status: "idle"; links: LinkView[]; error: null }
  | { status: "loading"; links: LinkView[]; error: null }
  | { status: "ready"; links: LinkView[]; error: null }
  | { status: "error"; links: LinkView[]; error: CommandError };

/**
 * Loads the document's native links and keeps them current.
 *
 * Link creation, target edits, deletion, undo and redo all announce a page
 * change on the in-window event bus; this hook refetches the list without
 * dropping the previous entries, so the overlay does not flash while a
 * refresh is in flight. A stale response for a previous document is
 * discarded.
 */
export function useLinks(documentId: string | null): {
  links: LinkView[];
  status: LinkListState["status"];
  error: CommandError | null;
  reload: () => Promise<void>;
} {
  const [state, setState] = useState<LinkListState>({
    status: "idle",
    links: [],
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
      links: current.links,
      error: null,
    }));
    try {
      const links = await listLinks(documentId);
      if (generation.current === request) {
        setState({ status: "ready", links, error: null });
      }
    } catch (error) {
      if (generation.current === request) {
        setState((current) => ({
          status: "error",
          links: current.links,
          error: toCommandError(error),
        }));
      }
    }
  }, [documentId]);

  useEffect(() => {
    generation.current += 1;
    setState({ status: documentId ? "loading" : "idle", links: [], error: null });
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
    links: state.links,
    status: state.status,
    error: state.error,
    reload: load,
  };
}
