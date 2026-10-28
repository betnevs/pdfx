/**
 * Keyboard ownership helpers shared by the workspace and the page viewer.
 *
 * Page-navigation keys (Home/End, PageUp/PageDown) and application undo/redo
 * must never fire while a text control owns the keyboard: there the platform
 * keeps its native caret, scrolling and undo behaviour.
 */

/**
 * Whether the event target is a text-editing control.
 *
 * Covers inputs of every type, textareas, selects and contentEditable hosts
 * (including descendants). A focused checkbox is treated as editable too:
 * page keys there would move the document out from under the control.
 */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!target || typeof target !== "object") {
    return false;
  }
  if (!(target instanceof Element)) {
    return false;
  }
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    return true;
  }
  if ((target as HTMLElement).isContentEditable) {
    return true;
  }
  return target.closest("[contenteditable]") !== null;
}

/**
 * Whether the platform is Windows, where Ctrl+Y is the conventional redo key.
 *
 * The native menu carries one accelerator per item, so Ctrl+Y is handled by
 * the webview's focused-keydown path instead of a second menu accelerator.
 */
export function isWindowsPlatform(): boolean {
  if (typeof navigator === "undefined") {
    return false;
  }
  return /windows/i.test(navigator.userAgent);
}
