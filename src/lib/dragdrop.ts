// Drag and drop. A file or link dropped on the window must never navigate the WebView or open anything.
// Only elements marked `data-drop-zone` (the future file tree) accept drops; text fields and the editor keep the
// browser's normal text drop. The terminal does not: a text drop into xterm's textarea would bypass the paste
// sanitiser.

const EDITABLE = "input, textarea, [contenteditable='true'], .cm-editor";

function accepts(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest("[data-drop-zone]")) return true;
  return target.closest(EDITABLE) !== null && target.closest(".xterm") === null;
}

/** Installs the document-level guards; returns the uninstall function. */
export function installDragDropGuard(doc: Document = document): () => void {
  const onOver = (e: DragEvent) => {
    if (accepts(e.target)) return;
    if (e.dataTransfer) e.dataTransfer.dropEffect = "none"; // no preventDefault: the drop is not allowed here
  };
  const onDrop = (e: DragEvent) => {
    if (accepts(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
  };
  const onDragStart = (e: DragEvent) => {
    if (e.target instanceof Element && e.target.closest("a, img")) e.preventDefault();
  };
  doc.addEventListener("dragenter", onOver, true);
  doc.addEventListener("dragover", onOver, true);
  doc.addEventListener("drop", onDrop, true);
  doc.addEventListener("dragstart", onDragStart, true);
  return () => {
    doc.removeEventListener("dragenter", onOver, true);
    doc.removeEventListener("dragover", onOver, true);
    doc.removeEventListener("drop", onDrop, true);
    doc.removeEventListener("dragstart", onDragStart, true);
  };
}
