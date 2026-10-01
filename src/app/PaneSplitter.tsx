import { useRef, useState } from 'react';

interface PaneSplitterProps {
  value: number;
  min: number;
  max: number;
  defaultValue: number;
  label: string;
  className?: string;
  onChange: (value: number) => void;
  onCommit: (value: number) => void;
}

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/// Accessible draggable divider between two panes. Pointer Events cover mouse,
/// touch and pen; keyboard users step with the arrow keys, jump with Home/End
/// and restore the default with Enter or a double click. The parent owns the
/// width state: `onChange` streams live updates during the gesture, `onCommit`
/// fires once when it ends so the final value can be persisted.
export function PaneSplitter({ value, min, max, defaultValue, label, className, onChange, onCommit }: PaneSplitterProps) {
  const origin = useRef<{ x: number; width: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    origin.current = { x: event.clientX, width: value };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
  };
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!origin.current) return;
    onChange(clamp(origin.current.width + event.clientX - origin.current.x, min, max));
  };
  const endDrag = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!origin.current) return;
    origin.current = null;
    setDragging(false);
    event.currentTarget.releasePointerCapture(event.pointerId);
    onCommit(value);
  };
  const step = (delta: number) => { const next = clamp(value + delta, min, max); if (next !== value) { onChange(next); onCommit(next); } };
  const jump = (target: number) => { onChange(target); onCommit(target); };
  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') { step(-16); event.preventDefault(); }
    else if (event.key === 'ArrowRight' || event.key === 'ArrowDown') { step(16); event.preventDefault(); }
    else if (event.key === 'Home') { jump(min); event.preventDefault(); }
    else if (event.key === 'End') { jump(max); event.preventDefault(); }
    else if (event.key === 'Enter' || event.key === ' ') { jump(defaultValue); event.preventDefault(); }
  };

  return <div
    role="separator"
    aria-orientation="vertical"
    aria-label={label}
    aria-valuenow={Math.round(value)}
    aria-valuemin={min}
    aria-valuemax={max}
    tabIndex={0}
    className={`pane-splitter${dragging ? ' dragging' : ''}${className ? ` ${className}` : ''}`}
    onPointerDown={onPointerDown}
    onPointerMove={onPointerMove}
    onPointerUp={endDrag}
    onPointerCancel={endDrag}
    onKeyDown={onKeyDown}
    onDoubleClick={() => jump(defaultValue)}
  />;
}
