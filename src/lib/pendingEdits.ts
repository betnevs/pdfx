/** In-memory inline drafts must be committed before save or changing pages. */
interface PendingEdit {
  flush: () => Promise<boolean>;
  dirty: () => boolean;
}
const drafts = new Map<string, Map<symbol, PendingEdit>>();
const flushing = new Map<string, Promise<boolean>>();
export const PENDING_EDIT_CHANGED = "pdfx:pending-edit-changed";
export function notifyPendingEditChanged(): void {
  window.dispatchEvent(new Event(PENDING_EDIT_CHANGED));
}
export function registerPendingEdit(
  documentId: string,
  flush: () => Promise<boolean>,
  dirty: () => boolean = () => true,
): () => void {
  const key = Symbol();
  let entries = drafts.get(documentId);
  if (!entries) {
    entries = new Map();
    drafts.set(documentId, entries);
  }
  entries.set(key, { flush, dirty });
  notifyPendingEditChanged();
  return () => {
    entries.delete(key);
    if (!entries.size && drafts.get(documentId) === entries)
      drafts.delete(documentId);
    notifyPendingEditChanged();
  };
}
export async function flushPendingEdits(documentId?: string): Promise<boolean> {
  if (!documentId) {
    for (const id of drafts.keys())
      if (!(await flushPendingEdits(id))) return false;
    return true;
  }
  const underway = flushing.get(documentId);
  if (underway) return underway;
  const run = (async () => {
    const entries = drafts.get(documentId);
    for (const [key, edit] of [...(entries?.entries() ?? [])]) {
      try {
        if (!(await edit.flush())) return false;
      } catch {
        return false;
      }
      entries?.delete(key);
    }
    if (entries && !entries.size && drafts.get(documentId) === entries)
      drafts.delete(documentId);
    notifyPendingEditChanged();
    return true;
  })();
  flushing.set(documentId, run);
  try {
    return await run;
  } finally {
    flushing.delete(documentId);
  }
}
export function hasPendingEdits(documentId?: string): boolean {
  return documentId ? (drafts.get(documentId)?.size ?? 0) > 0 : drafts.size > 0;
}
export function hasDirtyPendingEdits(documentId: string): boolean {
  return [...(drafts.get(documentId)?.values() ?? [])].some((edit) =>
    edit.dirty(),
  );
}
