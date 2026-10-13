import { useCallback, useEffect, useState, type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";

type Size = { width: number; height: number };

function readSize(key: string, fallback: Size, min: Size): Size {
  if (typeof window === "undefined") return fallback;
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as Partial<Size>;
    if (typeof parsed.width !== "number" || typeof parsed.height !== "number") return fallback;
    return {
      width: Math.max(min.width, parsed.width),
      height: Math.max(min.height, parsed.height),
    };
  } catch {
    return fallback;
  }
}

/**
 * Make an anchored popover resizable from its bottom-left corner and remember
 * the size per `name`. The popover is right-anchored, so dragging the left edge
 * outwards grows it.
 */
export function useResizablePopover(
  name: string,
  initial: Size,
  min: Size = { width: 320, height: 240 },
): { style: CSSProperties; handle: ReactNode } {
  const storageKey = `riga.popover.${name}`;
  const [size, setSize] = useState<Size>(() => readSize(storageKey, initial, min));

  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(size));
    } catch {
      // A blocked storage backend should not break the popover.
    }
  }, [storageKey, size]);

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLSpanElement>) => {
      event.preventDefault();
      event.stopPropagation();
      const startX = event.clientX;
      const startY = event.clientY;
      const start = size;
      const onMove = (move: PointerEvent) => {
        setSize({
          width: Math.max(min.width, start.width + (startX - move.clientX)),
          height: Math.max(min.height, start.height + (move.clientY - startY)),
        });
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [size, min.width, min.height],
  );

  return {
    style: { width: size.width, height: size.height },
    handle: (
      <span
        className="popover-resize-handle"
        role="separator"
        aria-label="Resize panel"
        onPointerDown={onPointerDown}
      />
    ),
  };
}
