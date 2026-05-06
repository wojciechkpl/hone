import { onCleanup, type Component } from "solid-js";
import { applySplitRatio } from "../lib/preview";

/** A 6px draggable gutter sitting between two grid columns. The parent
 * grid uses `--split-ratio` as its first track size; this component
 * just rewrites that variable as the user drags. */
export const SplitGutter: Component<{ containerRef: () => HTMLElement | null }> = (props) => {
  let dragging = false;

  function onDown(e: PointerEvent) {
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onMove(e: PointerEvent) {
    if (!dragging) return;
    const container = props.containerRef();
    if (!container) return;
    const rect = container.getBoundingClientRect();
    if (rect.width <= 0) return;
    const next = (e.clientX - rect.left) / rect.width;
    applySplitRatio(next);
  }

  function onUp(e: PointerEvent) {
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  }

  // Defensive global cleanup: if the component unmounts mid-drag, the
  // capture is released by the browser, but we still want to flag the
  // local state so a stale `dragging=true` doesn't carry over.
  onCleanup(() => {
    dragging = false;
  });

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize chat / preview split"
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      style={{
        cursor: "col-resize",
        width: "6px",
        background: "var(--surface-border)",
        "user-select": "none",
        "touch-action": "none",
      }}
      title="Drag to resize"
    />
  );
};
